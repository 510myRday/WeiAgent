use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    domain::{AgentToolsConfig, ApprovalDecision, PendingApproval, ToolResult},
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::JsonStore,
    tools::{
        approval::ApprovalManager,
        multi_edit::{
            MULTI_EDIT_GRANT_COMMAND, MULTI_EDIT_TOOL_NAME, MultiEditFileArgs, MultiEditRequest,
            execute_multi_edit,
        },
        terminal::{
            ExecuteTerminalRequest, MAX_SESSION_WAIT_MS, SandboxPermissions, TerminalManager,
            TerminalSessionAction, execute_terminal, process_read_to_result,
        },
        web_search::{WebSearchArgs, execute_web_search},
    },
};

#[derive(Debug, Clone)]
pub enum ToolExecutionEvent {
    ApprovalRequired {
        call_id: String,
        agent_id: Uuid,
        command: String,
        approval_id: String,
        cwd: Option<String>,
        runtime: String,
        permission: String,
        justification: Option<String>,
        background: bool,
    },
}

#[derive(Clone)]
pub struct ToolExecutionContext {
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub call_id: String,
    pub tools_config: AgentToolsConfig,
    pub store: Arc<JsonStore>,
    pub terminal_manager: Arc<TerminalManager>,
    pub approval_manager: Arc<ApprovalManager>,
    pub event_sender: Option<mpsc::Sender<ToolExecutionEvent>>,
}

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self, config: &AgentToolsConfig) -> ToolDefinition;
    fn is_enabled(&self, config: &AgentToolsConfig) -> bool;
    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult;
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn with_builtin_tools() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };
        registry.register(ExecuteTerminalTool);
        registry.register(TerminalSessionTool);
        registry.register(MultiEditFileTool);
        registry.register(WebSearchTool);
        registry
    }

    pub fn register<T: AgentTool + 'static>(&mut self, tool: T) {
        let name = tool
            .definition(&AgentToolsConfig::default())
            .function
            .name
            .clone();
        self.tools.insert(name, Arc::new(tool));
    }

    pub fn definitions(&self, config: &AgentToolsConfig) -> Option<Vec<ToolDefinition>> {
        let mut definitions: Vec<_> = self
            .tools
            .values()
            .filter(|tool| tool.is_enabled(config))
            .map(|tool| tool.definition(config))
            .collect();
        // Tool order is part of the model prompt prefix; keep it stable across
        // HashMap seeds and process restarts so provider prompt caches can match.
        definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));
        (!definitions.is_empty()).then_some(definitions)
    }

    /// Stable digest used by prompt-cache adapters. Approval grants are
    /// intentionally absent from tool schemas, while schema-affecting tool
    /// settings remain part of the serialized definitions.
    #[allow(dead_code)]
    pub fn definitions_hash(&self, config: &AgentToolsConfig) -> String {
        crate::llm::cache::tools_hash(self.definitions(config).as_deref())
    }

    pub async fn execute(
        &self,
        name: &str,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> ToolResult {
        let Some(tool) = self.tools.get(name) else {
            return ToolResult::Error {
                message: format!("Unknown tool: {name}"),
            };
        };
        if !tool.is_enabled(&context.tools_config) {
            return ToolResult::Blocked {
                reason: format!("Tool '{name}' is disabled"),
            };
        }
        tool.execute(arguments, context).await
    }
}

struct ExecuteTerminalTool;

const MAX_TERMINAL_COMMAND_BYTES: usize = 8_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteTerminalArgs {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    background: Option<bool>,
    #[serde(default)]
    tty: Option<bool>,
    #[serde(default)]
    sandbox_permissions: SandboxPermissions,
    #[serde(default)]
    justification: Option<String>,
}

#[async_trait]
impl AgentTool for ExecuteTerminalTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        let description = build_execute_terminal_description();
        tool_definition(
            "execute_terminal",
            &description,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 8000,
                        "description": "Command to execute using the host shell described by this tool. Prefer short focused commands; chain with shell operators when useful."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Existing working directory. Omit to use the default workspace; relative paths resolve from it. If the requested directory is outside the configured sandbox roots, set sandbox_permissions=require_escalated and provide justification so the user can approve access."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 120000,
                        "description": "Foreground wait in milliseconds. If omitted, the server default is 30000 ms. Maximum is 120000. An unfinished command returns process_id when sessions are supported; the runtime may stop it on timeout."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "If true, start immediately as a managed background process and return process_id without waiting. Use only for long-lived servers/watchers/dev services. Leave false (default) for ordinary short commands. Windows sandbox mode blocks background sessions; request approved full access for long-lived work there."
                    },
                    "tty": {
                        "type": "boolean",
                        "description": "Allocate a Codex PTY for a genuinely interactive command. Use for REPLs, interactive CLIs, or software that requires a terminal. Leave false for ordinary commands. Windows sandbox sessions are non-interactive and cannot use PTY or continued stdin; request full access when either is required."
                    },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Execution privilege request. use_default follows the server's current policy. For any cwd outside the configured sandbox roots, use require_escalated and provide a justification; the server may request approval. Example: {\"sandbox_permissions\":\"require_escalated\",\"justification\":\"access is required for the requested workspace\"}."
                    },
                    "justification": {
                        "type": "string",
                        "maxLength": 2000,
                        "description": "Short reason for elevated access. Include it when sandbox_permissions is require_escalated. Max 2000 bytes. Skip it for use_default."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args = match serde_json::from_value::<ExecuteTerminalArgs>(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid execute_terminal arguments: {error}"),
                };
            }
        };
        let cwd = match normalize_optional_cwd(args.cwd) {
            Ok(cwd) => cwd,
            Err(message) => return ToolResult::Error { message },
        };
        if args.timeout_ms.is_some_and(|timeout| {
            timeout == 0 || timeout > crate::tools::terminal::MAX_TERMINAL_TIMEOUT_MS
        }) {
            return ToolResult::Error {
                message: format!(
                    "timeout_ms must be between 1 and {}",
                    crate::tools::terminal::MAX_TERMINAL_TIMEOUT_MS
                ),
            };
        }
        let command = args.command;
        if command.trim().is_empty() {
            return ToolResult::Error {
                message: "command cannot be empty".to_string(),
            };
        }
        if command.len() > MAX_TERMINAL_COMMAND_BYTES {
            return ToolResult::Error {
                message: format!(
                    "command exceeds the {MAX_TERMINAL_COMMAND_BYTES} byte limit; split it into smaller focused commands"
                ),
            };
        }
        if let Some(message) = unsupported_host_shell_syntax(&command) {
            return ToolResult::Error {
                message: message.to_string(),
            };
        }
        let timeout_ms = args.timeout_ms;
        let background = args.background;
        let tty = args.tty;
        let sandbox_permissions = args.sandbox_permissions;
        let justification = args.justification;
        let terminal_config = match live_terminal_config(&context).await {
            Ok(config) => config,
            Err(message) => return ToolResult::Error { message },
        };
        if terminal_config.permission_mode
            == crate::domain::TerminalPermissionMode::RequestApproval
            && sandbox_permissions == SandboxPermissions::RequireEscalated
            && justification
                .as_deref()
                .is_none_or(|value| value.trim().is_empty() || value.len() > 2_000)
        {
            return ToolResult::Error {
                message:
                    "justification is required for require_escalated and must be at most 2000 bytes"
                        .to_string(),
            };
        }
        let mut result = execute_terminal(
            &ExecuteTerminalRequest {
                command: command.clone(),
                cwd: cwd.clone(),
                timeout_ms,
                background,
                tty,
                sandbox_permissions,
                approved: false,
                agent_id: context.agent_id,
                conversation_id: context.conversation_id,
                tool_call_id: context.call_id.clone(),
            },
            &terminal_config,
            &context.store.data_dir,
            &context.terminal_manager,
        )
        .await;

        let ToolResult::PendingApproval { approval_id, .. } = &result else {
            return result;
        };
        let approval_id = approval_id.clone();
        let Some(event_sender) = context.event_sender.clone() else {
            return ToolResult::Rejected {
                reason: "This command requires interactive approval; use the SSE message endpoint"
                    .to_string(),
            };
        };
        let decision_rx = context.approval_manager.register(PendingApproval {
            approval_id: approval_id.clone(),
            agent_id: context.agent_id,
            conversation_id: context.conversation_id,
            tool_call_id: context.call_id.clone(),
            command: command.clone(),
            cwd: cwd
                .clone()
                .or_else(|| {
                    context
                        .tools_config
                        .terminal
                        .allowed_cwd_roots
                        .first()
                        .cloned()
                })
                .or_else(|| Some(context.store.data_dir.display().to_string())),
            runtime: "codex_full_access".to_string(),
            permission: "full_access".to_string(),
            justification: justification.clone(),
            background: background.unwrap_or(false),
        });
        let _approval_guard = ApprovalRegistrationGuard {
            manager: context.approval_manager.clone(),
            approval_id: approval_id.clone(),
        };
        if event_sender
            .send(ToolExecutionEvent::ApprovalRequired {
                call_id: context.call_id.clone(),
                agent_id: context.agent_id,
                command: command.clone(),
                approval_id: approval_id.clone(),
                cwd: cwd.clone(),
                runtime: "codex_full_access".to_string(),
                permission: "full_access".to_string(),
                justification: justification.clone(),
                background: background.unwrap_or(false),
            })
            .await
            .is_err()
        {
            return ToolResult::Rejected {
                reason: "Approval client disconnected".to_string(),
            };
        }

        let decision = wait_for_approval_decision(
            &context,
            &approval_id,
            decision_rx,
            &event_sender,
            |tools_config| {
                tools_config.terminal.permission_mode
                    == crate::domain::TerminalPermissionMode::FullAccess
            },
        )
        .await;
        match decision {
            Some(ApprovalDecision::Approve) => {}
            Some(ApprovalDecision::AlwaysApprove) => {
                if let Err(error) = context
                    .store
                    .always_allow_terminal_command(context.agent_id, &command, cwd.as_deref())
                    .await
                {
                    return ToolResult::Error {
                        message: format!("Failed to persist always-allow rule: {error}"),
                    };
                }
            }
            Some(ApprovalDecision::Reject) => {
                return ToolResult::Rejected {
                    reason: "Command rejected by user or approval expired".to_string(),
                };
            }
            None => {
                return ToolResult::Rejected {
                    reason: "Approval client disconnected".to_string(),
                };
            }
        }

        let terminal_config = match live_terminal_config(&context).await {
            Ok(config) => config,
            Err(message) => return ToolResult::Error { message },
        };
        result = execute_terminal(
            &ExecuteTerminalRequest {
                command,
                cwd,
                timeout_ms,
                background,
                tty,
                sandbox_permissions,
                approved: true,
                agent_id: context.agent_id,
                conversation_id: context.conversation_id,
                tool_call_id: context.call_id,
            },
            &terminal_config,
            &context.store.data_dir,
            &context.terminal_manager,
        )
        .await;
        result
    }
}

struct TerminalSessionTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalSessionArgs {
    process_id: String,
    action: TerminalSessionAction,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    cursor: Option<u64>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    cols: Option<u16>,
}

#[async_trait]
impl AgentTool for TerminalSessionTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "terminal_session",
            "Continue a running process returned by execute_terminal. Pass the previous output_cursor as cursor to receive only new output. Sessions are limited to the current agent and conversation.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process_id": {
                        "type": "string",
                        "description": "UUID process_id returned by execute_terminal."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["poll", "write", "submit", "interrupt", "eof", "terminate", "resize"],
                        "description": "Session operation. poll only reads; write sends raw input; submit appends Enter; interrupt sends Ctrl+C in a PTY and terminates a pipe session; eof closes stdin; terminate reliably stops the process; resize changes PTY rows/columns."
                    },
                    "input": {
                        "type": "string",
                        "description": "Used only by write and submit. Required for write; submit may use an empty string to press Enter."
                    },
                    "cursor": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Previous output_cursor. The response contains only output after this cursor. Omit or use 0 to read retained output from the beginning."
                    },
                    "wait_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 30000,
                        "description": "Long-poll wait for new output or process completion. Default 5000 for poll and 250 after input/control actions."
                    },
                    "rows": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "description": "Required with cols only for action=resize."
                    },
                    "cols": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 1000,
                        "description": "Required with rows only for action=resize."
                    }
                },
                "required": ["process_id", "action"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args = match serde_json::from_value::<TerminalSessionArgs>(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid terminal_session arguments: {error}"),
                };
            }
        };
        let process_id = args.process_id;
        if Uuid::parse_str(&process_id).is_err() {
            return ToolResult::Error {
                message: "process_id must be a UUID".to_string(),
            };
        }
        let Some(existing) = context.terminal_manager.get(&process_id).await else {
            return ToolResult::Error {
                message: format!("Terminal process '{process_id}' was not found"),
            };
        };
        if existing.agent_id != context.agent_id
            || existing.conversation_id != context.conversation_id
        {
            return ToolResult::Blocked {
                reason: "The process belongs to another agent run".to_string(),
            };
        }
        let cursor = args.cursor.unwrap_or(0);
        let wait_ms = args
            .wait_ms
            .unwrap_or(if args.action == TerminalSessionAction::Poll {
                5_000
            } else {
                250
            });
        if wait_ms > MAX_SESSION_WAIT_MS {
            return ToolResult::Error {
                message: format!("wait_ms must be at most {MAX_SESSION_WAIT_MS}"),
            };
        }
        let operation = match args.action {
            TerminalSessionAction::Poll => Ok(()),
            TerminalSessionAction::Write => {
                let Some(input) = args.input.as_deref() else {
                    return ToolResult::Error {
                        message: "input is required for action=write".to_string(),
                    };
                };
                context
                    .terminal_manager
                    .write(&process_id, input.as_bytes(), false)
                    .await
                    .map(|_| ())
            }
            TerminalSessionAction::Submit => {
                let mut input = args.input.unwrap_or_default();
                input.push_str(if cfg!(windows) && existing.tty {
                    "\r"
                } else {
                    "\n"
                });
                context
                    .terminal_manager
                    .write(&process_id, input.as_bytes(), false)
                    .await
                    .map(|_| ())
            }
            TerminalSessionAction::Interrupt => context
                .terminal_manager
                .interrupt(&process_id)
                .await
                .map(|_| ()),
            TerminalSessionAction::Eof => context
                .terminal_manager
                .write(&process_id, &[], true)
                .await
                .map(|_| ()),
            TerminalSessionAction::Terminate => context
                .terminal_manager
                .terminate(&process_id)
                .await
                .map(|_| ()),
            TerminalSessionAction::Resize => {
                let (Some(rows), Some(cols)) = (args.rows, args.cols) else {
                    return ToolResult::Error {
                        message: "rows and cols are required for action=resize".to_string(),
                    };
                };
                context
                    .terminal_manager
                    .resize(&process_id, rows, cols)
                    .await
                    .map(|_| ())
            }
        };
        if let Err(error) = operation {
            return ToolResult::Error {
                message: error.to_string(),
            };
        }
        match context
            .terminal_manager
            .read_since(
                &process_id,
                cursor,
                std::time::Duration::from_millis(wait_ms),
            )
            .await
        {
            Some((info, stdout, stderr, next_cursor)) => {
                process_read_to_result(info, stdout, stderr, next_cursor)
            }
            None => ToolResult::Error {
                message: format!("Terminal process '{process_id}' was not found"),
            },
        }
    }
}

struct MultiEditFileTool;

#[async_trait]
impl AgentTool for MultiEditFileTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        let description = "Edit, create, delete, or move text files in one atomic call. Prefer this over shell redirection or execute_terminal file rewrites for source, config, and documentation changes. Every operation is validated by the server; failure leaves files unchanged. In-workspace edits may require approval under request_approval. Paths outside the configured workspace roots must set sandbox_permissions=require_escalated with a short justification so the user can approve; do not bypass multi_edit_file via shell when an outside-root edit is needed. Exact edits compare LF-normalized text; use replace_all or startLine only when needed.";
        tool_definition(
            MULTI_EDIT_TOOL_NAME,
            &description,
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "operations": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 32,
                        "description": "Ordered file operations. A path may appear only once per call.",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["type", "path", "edits"],
                                    "properties": {
                                        "type": {"const": "edit"},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096},
                                        "edits": {
                                            "type": "array",
                                            "minItems": 1,
                                            "maxItems": 64,
                                            "items": {
                                                "type": "object",
                                                "additionalProperties": false,
                                                "required": ["old_string", "new_string"],
                                                "properties": {
                                                    "old_string": {"type": "string", "maxLength": 200000},
                                                    "new_string": {"type": "string", "maxLength": 2097152},
                                                    "replace_all": {"type": "boolean"},
                                                    "startLine": {"type": "integer", "minimum": 1}
                                                }
                                            }
                                        }
                                    }
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["type", "path", "content"],
                                    "properties": {
                                        "type": {"const": "create"},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096},
                                        "content": {"type": "string", "maxLength": 2097152},
                                        "overwrite": {"type": "boolean"}
                                    }
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["type", "path"],
                                    "properties": {
                                        "type": {"const": "delete"},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096}
                                    }
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["type", "from", "to"],
                                    "properties": {
                                        "type": {"const": "move"},
                                        "from": {"type": "string", "minLength": 1, "maxLength": 4096},
                                        "to": {"type": "string", "minLength": 1, "maxLength": 4096},
                                        "overwrite": {"type": "boolean"}
                                    }
                                }
                            ]
                        }
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Existing working directory for relative paths. Omit to use the default workspace. If target files are outside the configured workspace roots, set sandbox_permissions=require_escalated and provide justification so the user can approve access."
                    },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Write privilege request. use_default only allows paths under configured workspace roots. For any path outside those roots, use require_escalated and provide a justification; the server may request approval. Prefer this over rewriting files through execute_terminal."
                    },
                    "justification": {
                        "type": "string",
                        "maxLength": 2000,
                        "description": "Short reason for elevated write access. Include it when sandbox_permissions is require_escalated. Max 2000 bytes. Skip it for use_default."
                    }
                },
                "required": ["operations"]
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args: MultiEditFileArgs = match serde_json::from_value(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid multi_edit_file arguments: {error}"),
                };
            }
        };
        let cwd = match normalize_optional_cwd(args.cwd.clone()) {
            Ok(cwd) => cwd,
            Err(message) => return ToolResult::Error { message },
        };
        let terminal_config = match live_terminal_config(&context).await {
            Ok(config) => config,
            Err(message) => return ToolResult::Error { message },
        };
        if terminal_config.permission_mode
            == crate::domain::TerminalPermissionMode::RequestApproval
            && args.sandbox_permissions == SandboxPermissions::RequireEscalated
        {
            let justification = args
                .justification
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if justification.is_none() || justification.is_some_and(|value| value.len() > 2000) {
                return ToolResult::Error {
                    message: "justification is required for require_escalated and must be at most 2000 bytes"
                        .to_string(),
                };
            }
        }
        let justification = args.justification.clone();
        let args = MultiEditFileArgs {
            cwd: cwd.clone(),
            ..args
        };

        let mut result = execute_multi_edit(
            &MultiEditRequest {
                args: args.clone(),
                approved: false,
            },
            &terminal_config,
            &context.store.data_dir,
        )
        .await;

        let ToolResult::PendingApproval {
            approval_id,
            command,
        } = &result
        else {
            return result;
        };
        let approval_id = approval_id.clone();
        let command = command.clone();
        let Some(event_sender) = context.event_sender.clone() else {
            return ToolResult::Rejected {
                reason: "This edit requires interactive approval; use the SSE message endpoint"
                    .to_string(),
            };
        };

        let approval_cwd = cwd
            .clone()
            .or_else(|| {
                context
                    .tools_config
                    .terminal
                    .allowed_cwd_roots
                    .first()
                    .cloned()
            })
            .or_else(|| Some(context.store.data_dir.display().to_string()));
        let decision_rx = context.approval_manager.register(PendingApproval {
            approval_id: approval_id.clone(),
            agent_id: context.agent_id,
            conversation_id: context.conversation_id,
            tool_call_id: context.call_id.clone(),
            command: command.clone(),
            cwd: approval_cwd.clone(),
            runtime: "builtin_multi_edit".to_string(),
            permission: "file_write".to_string(),
            justification: justification
                .clone()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| Some("Apply multi_edit_file changes".to_string())),
            background: false,
        });
        let _approval_guard = ApprovalRegistrationGuard {
            manager: context.approval_manager.clone(),
            approval_id: approval_id.clone(),
        };
        if event_sender
            .send(ToolExecutionEvent::ApprovalRequired {
                call_id: context.call_id.clone(),
                agent_id: context.agent_id,
                command: command.clone(),
                approval_id: approval_id.clone(),
                cwd: approval_cwd.clone(),
                runtime: "builtin_multi_edit".to_string(),
                permission: "file_write".to_string(),
                justification: justification
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| Some("Apply multi_edit_file changes".to_string())),
                background: false,
            })
            .await
            .is_err()
        {
            return ToolResult::Rejected {
                reason: "Approval client disconnected".to_string(),
            };
        }

        let decision = wait_for_approval_decision(
            &context,
            &approval_id,
            decision_rx,
            &event_sender,
            |tools_config| {
                tools_config.terminal.permission_mode
                    == crate::domain::TerminalPermissionMode::FullAccess
            },
        )
        .await;
        match decision {
            Some(ApprovalDecision::Approve) => {}
            Some(ApprovalDecision::AlwaysApprove) => {
                if let Err(error) = context
                    .store
                    .always_allow_terminal_command(
                        context.agent_id,
                        MULTI_EDIT_GRANT_COMMAND,
                        approval_cwd.as_deref(),
                    )
                    .await
                {
                    return ToolResult::Error {
                        message: format!("Failed to persist always-allow rule: {error}"),
                    };
                }
            }
            Some(ApprovalDecision::Reject) => {
                return ToolResult::Rejected {
                    reason: "Edit rejected by user or approval expired".to_string(),
                };
            }
            None => {
                return ToolResult::Rejected {
                    reason: "Approval client disconnected".to_string(),
                };
            }
        }

        let terminal_config = match live_terminal_config(&context).await {
            Ok(config) => config,
            Err(message) => return ToolResult::Error { message },
        };
        result = execute_multi_edit(
            &MultiEditRequest {
                args,
                approved: true,
            },
            &terminal_config,
            &context.store.data_dir,
        )
        .await;
        result
    }
}


struct WebSearchTool;

#[async_trait]
impl AgentTool for WebSearchTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_search",
            "Search the public web and return ranked results with titles, URLs, and snippets. Use for up-to-date facts, docs, news, and references that are not already known from the conversation.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 500,
                        "description": "Search query. Prefer concrete keywords or a short question."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum number of results to return. Defaults to the tool configuration when omitted."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled && config.web_search.has_usable_source()
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args: WebSearchArgs = match serde_json::from_value(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid web_search arguments: {error}"),
                };
            }
        };
        execute_web_search(args, &context.tools_config.web_search).await
    }
}
fn tool_definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        kind: "function".to_string(),
        function: FunctionDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

fn normalize_optional_cwd(cwd: Option<String>) -> Result<Option<String>, String> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return Ok(None);
    }
    if cwd.len() > 4_096 {
        return Err("cwd must be no longer than 4096 bytes".to_string());
    }
    Ok(Some(cwd.to_string()))
}

fn unsupported_host_shell_syntax(command: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let lower = command.to_ascii_lowercase();
        if lower.contains("python - <<")
            || lower.contains("python3 - <<")
            || command.contains("<<'")
            || command.contains("<<\"")
        {
            let shell = crate::tools::shell::selected_host_shell();
            return Some(format!(
                "Windows commands are interpreted using {} syntax, which does not support POSIX heredocs (`<<EOF` or `python - <<'PY'`). Use `python -c`, a PowerShell here-string when PowerShell is selected, or split the command into smaller Windows-compatible commands.",
                shell.syntax_name()
            ));
        }
    }
    None
}

fn build_execute_terminal_description() -> String {
    let shell = crate::tools::shell::selected_host_shell();
    let host_guidance = shell.tool_guidance();
    let examples = shell.command_examples();

    format!(
        "Execute a local command and return stdout, stderr, and exit_code, or process_id when it continues running. Use it to inspect files, search repositories, run builds/tests, and manage local processes; prefer `rg` for text search and `rg --files` for file discovery. Prefer multi_edit_file for source/config edits (including outside-root paths via sandbox_permissions=require_escalated + justification) instead of rewriting files through shell. Selected shell: {}. {host_guidance} {examples} The server enforces its current execution policy; request elevated access with sandbox_permissions=require_escalated and a justification when needed. Run ordinary commands in the foreground; use background only for long-lived services and tty only for interactive programs. Continue a returned process_id with terminal_session instead of starting the command again.",
        shell.display_name()
    )
}


async fn live_tools_config(context: &ToolExecutionContext) -> Result<AgentToolsConfig, String> {
    context
        .store
        .read_agent_tools_config(context.agent_id)
        .await
        .map_err(|error| format!("Failed to reload tools config: {error}"))
}

async fn live_terminal_config(
    context: &ToolExecutionContext,
) -> Result<crate::domain::TerminalToolConfig, String> {
    Ok(live_tools_config(context).await?.terminal)
}

async fn wait_for_approval_decision(
    context: &ToolExecutionContext,
    approval_id: &str,
    mut decision_rx: tokio::sync::oneshot::Receiver<ApprovalDecision>,
    event_sender: &mpsc::Sender<ToolExecutionEvent>,
    auto_approve: impl Fn(&AgentToolsConfig) -> bool,
) -> Option<ApprovalDecision> {
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the immediate first tick so we wait for a real interval first.
    poll.tick().await;
    loop {
        tokio::select! {
            decision = &mut decision_rx => return decision.ok(),
            _ = event_sender.closed() => return None,
            _ = poll.tick() => {
                match live_tools_config(context).await {
                    Ok(tools_config) if auto_approve(&tools_config) => {
                        // Drop the pending approval so the UI can clear it.
                        context.approval_manager.cancel(approval_id);
                        return Some(ApprovalDecision::Approve);
                    }
                    _ => {}
                }
            }
        }
    }
}

struct ApprovalRegistrationGuard {
    manager: Arc<ApprovalManager>,
    approval_id: String,
}

impl Drop for ApprovalRegistrationGuard {
    fn drop(&mut self) {
        self.manager.cancel(&self.approval_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TERMINAL_COMMAND_BYTES, ToolRegistry, normalize_optional_cwd,
        unsupported_host_shell_syntax,
    };
    use crate::domain::AgentToolsConfig;

    #[test]
    fn blank_cwd_uses_the_default_workspace() {
        assert_eq!(normalize_optional_cwd(None).unwrap(), None);
        assert_eq!(normalize_optional_cwd(Some(String::new())).unwrap(), None);
        assert_eq!(
            normalize_optional_cwd(Some(" \r\n\t ".to_string())).unwrap(),
            None
        );
    }

    #[test]
    fn cwd_is_trimmed_and_length_limited() {
        assert_eq!(
            normalize_optional_cwd(Some("  .  ".to_string())).unwrap(),
            Some(".".to_string())
        );
        assert!(normalize_optional_cwd(Some("x".repeat(4_097))).is_err());
    }

    #[test]
    fn terminal_command_limit_matches_the_tool_contract() {
        assert_eq!(MAX_TERMINAL_COMMAND_BYTES, 8_000);
    }

    #[test]
    fn terminal_description_explains_stable_escalation_contract() {
        let description = super::build_execute_terminal_description();
        assert!(description.contains("sandbox_permissions=require_escalated"));
        assert!(description.contains("justification"));
        assert!(description.contains("server enforces its current execution policy"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_posix_python_heredocs_with_actionable_guidance() {
        let message = unsupported_host_shell_syntax("python - <<'PY'\nprint('ok')\nPY")
            .expect("POSIX heredoc should be rejected on Windows");
        assert!(message.contains(crate::tools::shell::selected_host_shell().syntax_name()));
        assert!(message.contains("python -c"));
    }

    #[test]
    fn terminal_definition_describes_the_host_shell() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let terminal = definitions
            .iter()
            .find(|definition| definition.function.name == "execute_terminal")
            .unwrap();
        let selected_shell = crate::tools::shell::selected_host_shell();
        let expected_shell = format!("Selected shell: {}", selected_shell.display_name());
        assert!(terminal.function.description.contains(&expected_shell));
        assert!(terminal.function.description.contains("ordinary commands"));
        assert!(terminal.function.description.contains("process_id"));
        assert!(terminal.function.description.contains("prefer `rg`"));
        assert!(terminal.function.description.contains("`rg --files`"));
        if cfg!(windows) {
            assert!(
                terminal
                    .function
                    .description
                    .contains(selected_shell.syntax_name())
            );
            assert!(terminal.function.description.contains("Examples:"));
        } else {
            assert!(terminal.function.description.contains("Unix paths"));
        }
        assert!(
            terminal.function.parameters["properties"]["timeout_ms"]["description"]
                .as_str()
                .unwrap()
                .contains("30000")
        );
        assert!(
            terminal.function.parameters["properties"]["background"]["description"]
                .as_str()
                .unwrap()
                .contains("only for long-lived")
        );
        let session_tool = definitions
            .iter()
            .find(|definition| definition.function.name == "terminal_session")
            .unwrap();
        assert!(session_tool.function.description.contains("output_cursor"));
        assert!(
            terminal.function.parameters["properties"]["tty"]["description"]
                .as_str()
                .unwrap()
                .contains("Codex PTY")
        );
    }

    #[test]
    fn terminal_definition_is_stable_across_permission_modes() {
        let mut request_approval = AgentToolsConfig::default();
        request_approval.terminal.enabled = true;
        let mut full_access = request_approval.clone();
        full_access.terminal.permission_mode = crate::domain::TerminalPermissionMode::FullAccess;
        full_access.terminal.default_timeout_ms = 45_000;

        let approval_definitions = ToolRegistry::with_builtin_tools()
            .definitions(&request_approval)
            .unwrap();
        let full_access_definitions = ToolRegistry::with_builtin_tools()
            .definitions(&full_access)
            .unwrap();

        assert_eq!(approval_definitions, full_access_definitions);
        let terminal = approval_definitions
            .iter()
            .find(|definition| definition.function.name == "execute_terminal")
            .unwrap();
        assert!(!terminal.function.description.contains("request_approval"));
        assert!(!terminal.function.description.contains("full_access"));
        assert!(
            terminal.function.parameters["properties"]["timeout_ms"]["description"]
                .as_str()
                .unwrap()
                .contains("server default")
        );
    }

    #[test]
    fn terminal_enabled_switch_controls_both_terminal_tools() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        let enabled = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let names: Vec<_> = enabled
            .iter()
            .map(|definition| definition.function.name.as_str())
            .collect();
        assert!(names.contains(&"execute_terminal"));
        assert!(names.contains(&"terminal_session"));
        assert!(names.contains(&"multi_edit_file"));
        assert_eq!(names.len(), 3);

        config.terminal.enabled = false;
        assert!(
            ToolRegistry::with_builtin_tools()
                .definitions(&config)
                .is_none()
        );
    }

    #[test]
    fn web_search_enabled_only_with_keys() {
        let mut config = AgentToolsConfig::default();
        config.web_search.enabled = true;
        let disabled = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap_or_default()
            .into_iter()
            .any(|item| item.function.name == "web_search");
        assert!(!disabled);

        config.web_search.sources[0].api_keys = vec!["sk-test".to_string()];
        let enabled = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap_or_default()
            .into_iter()
            .any(|item| item.function.name == "web_search");
        assert!(enabled);
    }

    #[test]
    fn tool_definitions_are_sorted_by_name() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let names: Vec<_> = definitions
            .iter()
            .map(|definition| definition.function.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["execute_terminal", "multi_edit_file", "terminal_session"]
        );
    }

    #[test]
    fn multi_edit_is_registered_with_terminal_group() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let multi = definitions
            .iter()
            .find(|definition| definition.function.name == "multi_edit_file")
            .expect("multi_edit_file should be registered");
        let parameters = &multi.function.parameters;
        assert_eq!(
            parameters["properties"]["sandbox_permissions"]["enum"],
            serde_json::json!(["use_default", "require_escalated"])
        );
        assert!(parameters["properties"]["justification"].is_object());
        assert!(
            multi
                .function
                .description
                .contains("sandbox_permissions=require_escalated")
        );
        assert!(multi.function.description.contains("may require approval"));
        assert_eq!(
            multi.function.parameters["properties"]["operations"]["type"],
            "array"
        );
        assert_eq!(
            multi.function.parameters["properties"]["operations"]["items"]["oneOf"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert!(
            multi.function.parameters["properties"]
                .get("path")
                .is_none()
        );
        assert!(
            multi.function.parameters["properties"]
                .get("edits")
                .is_none()
        );
        assert!(
            multi.function.parameters["properties"]
                .get("files")
                .is_none()
        );
    }
}


