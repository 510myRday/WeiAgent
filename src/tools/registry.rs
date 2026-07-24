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
        web_search::{
            WebSearchArgs, describe_web_search_sources,
            evidence::{
                WebExtractArgs, WebSearchProvenanceCache, execute_web_extract_with_status_updates,
                record_web_search_provenance,
            },
            execute_web_search_with_status_updates,
        },
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
    pub web_search_provenance: WebSearchProvenanceCache,
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
        registry.register(WebExtractTool);
        registry.register(WebSearchTool);
        registry.register(WebSearchSourcesTool);
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
    fn definition(&self, config: &AgentToolsConfig) -> ToolDefinition {
        let description = build_execute_terminal_description(config);
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
        if terminal_config.permission_mode == crate::domain::TerminalPermissionMode::RequestApproval
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
            &context.store.terminal_workspace_dir,
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
        let approval_cwd = cwd
            .clone()
            .or_else(|| Some(context.store.terminal_workspace_dir.display().to_string()));
        let decision_rx = context.approval_manager.register(PendingApproval {
            approval_id: approval_id.clone(),
            agent_id: context.agent_id,
            conversation_id: context.conversation_id,
            tool_call_id: context.call_id.clone(),
            command: command.clone(),
            cwd: approval_cwd.clone(),
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
                cwd: approval_cwd,
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
            &context.store.terminal_workspace_dir,
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
                        "maximum": MAX_SESSION_WAIT_MS,
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
    fn definition(&self, config: &AgentToolsConfig) -> ToolDefinition {
        let description = format!(
            "Edit, create, delete, or move text files in one atomic call. Prefer this over shell redirection or execute_terminal file rewrites for source, config, and documentation changes. Every operation is validated by the server; failure leaves files unchanged. Prefer one input mode: operations (preferred), path+edits, or files. For an edit, read the current file first when it is not already in context, copy old_string exactly including indentation, keep it short and unique, and put all replacements for one path in one operation. operations, files, and edits must be JSON arrays, never quoted JSON text. Do not include line-number prefixes in old_string. In-workspace edits may require approval under request_approval. Paths outside the configured workspace roots must set sandbox_permissions=require_escalated with a short justification so the user can approve; do not bypass multi_edit_file via shell when an outside-root edit is needed. Exact edits compare LF-normalized text; use replace_all or startLine only when needed.{}",
            configured_working_directories_guidance(config)
        );
        tool_definition(
            MULTI_EDIT_TOOL_NAME,
            &description,
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 4096,
                        "description": "Legacy single-file edit path, relative to cwd unless absolute. Use with top-level edits."
                    },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 64,
                        "description": "Legacy ordered exact replacements for path. Do not pass this array as a quoted JSON string.",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["old_string", "new_string"],
                            "properties": {
                                "old_string": {"type": "string", "maxLength": 200000, "description": "Exact current text to replace, including indentation."},
                                "new_string": {"type": "string", "maxLength": 2097152},
                                "replace_all": {"type": "boolean", "description": "Replace every occurrence. Defaults to false."},
                                "startLine": {"type": "integer", "minimum": 1, "description": "1-based hint for resolving an ambiguous match."}
                            }
                        }
                    },
                    "files": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 32,
                        "description": "Multiple existing files. Each item has path and edits. Do not combine with path or operations.",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["path", "edits"],
                            "properties": {
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
                        }
                    },
                    "operations": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 32,
                        "description": "Preferred ordered file operations. A path may appear only once per call. Pass a JSON array, not a quoted JSON string.",
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
                "required": []
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
        if terminal_config.permission_mode == crate::domain::TerminalPermissionMode::RequestApproval
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
            &context.store.terminal_workspace_dir,
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
            .or_else(|| Some(context.store.terminal_workspace_dir.display().to_string()));
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
            &context.store.terminal_workspace_dir,
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
            "Search one or more complementary web sources in parallel. Returns deduplicated evidence candidates, per-source diagnostics, and separately labeled AI research syntheses. Use web_extract on important URLs before citing them.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 500,
                        "description": "Search query. Prefer concrete keywords or a short question."
                    },
                    "additional_queries": {
                        "type": "array",
                        "maxItems": 3,
                        "items": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "description": "Optional complementary query rewrites for parallel retrieval. Use for deep research, ambiguous wording, or likely vocabulary differences."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["auto", "fast", "balanced", "deep", "custom"],
                        "description": "Routing depth. fast uses one source at a time with fallback, balanced uses up to three independent sources in parallel, deep uses all eligible independent source groups, and custom follows source_ids/source_roles."
                    },
                    "source_roles": {
                        "type": "array",
                        "uniqueItems": true,
                        "items": {
                            "type": "string",
                            "enum": [
                                "general_search",
                                "semantic_research",
                                "current_news",
                                "official_docs",
                                "technical_research",
                                "academic_research",
                                "fact_checking",
                                "market_research",
                                "product_research",
                                "multilingual_search",
                                "ai_research",
                                "social_search"
                            ]
                        },
                        "description": "Optional capability roles used to constrain server-side routing."
                    },
                    "source_ids": {
                        "type": "array",
                        "uniqueItems": true,
                        "items": { "type": "string", "minLength": 1, "maxLength": 80 },
                        "description": "Optional source IDs from web_search_sources. Prefer automatic routing unless a source characteristic materially fits the task."
                    },
                    "max_results_per_source": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum native results requested from each selected source."
                    },
                    "allowed_domains": {
                        "type": "array",
                        "maxItems": 20,
                        "uniqueItems": true,
                        "items": { "type": "string", "minLength": 1, "maxLength": 253 },
                        "description": "Only return results from these domains or their subdomains."
                    },
                    "blocked_domains": {
                        "type": "array",
                        "maxItems": 20,
                        "uniqueItems": true,
                        "items": { "type": "string", "minLength": 1, "maxLength": 253 },
                        "description": "Exclude results from these domains and their subdomains."
                    },
                    "recency_days": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 3650,
                        "description": "Prefer or require results published within this many days when supported by the source."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled
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
        let execution =
            execute_web_search_with_status_updates(args, &context.tools_config.web_search).await;
        record_web_search_provenance(&context.web_search_provenance, &execution.result);
        if let Err(error) = context
            .store
            .update_web_search_key_statuses(&execution.status_updates)
            .await
        {
            tracing::warn!(%error, "failed to persist web search key statuses");
        }
        execution.result
    }
}

struct WebSearchSourcesTool;

#[async_trait]
impl AgentTool for WebSearchSourcesTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_search_sources",
            "List the currently configured web search sources, their roles, strengths, limitations, cost/latency tiers, evidence type, independence group, capabilities, and health. Use before custom source selection; source state is dynamic and is intentionally not embedded in the stable web_search schema.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        if !arguments.as_object().is_some_and(serde_json::Map::is_empty) {
            return ToolResult::Error {
                message: "web_search_sources accepts no arguments".to_string(),
            };
        }
        let output = describe_web_search_sources(&context.tools_config.web_search);
        match serde_json::to_string_pretty(&output) {
            Ok(stdout) => ToolResult::Completed {
                stdout,
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 0,
                runtime: Some("builtin_web_search_sources".to_string()),
                output_cursor: 0,
            },
            Err(error) => ToolResult::Error {
                message: format!("failed to serialize web search sources: {error}"),
            },
        }
    }
}

struct WebExtractTool;

#[async_trait]
impl AgentTool for WebExtractTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_extract",
            "Open one to five public HTTP(S) URLs. Uses local extraction first, then configured URL-content providers when local content is blocked, dynamic, or low quality. Stores direct evidence, returns focused excerpts, and labels Grok SSO fallback output as AI-derived.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 5,
                        "uniqueItems": true,
                        "items": { "type": "string", "format": "uri", "minLength": 8, "maxLength": 4096 },
                        "description": "Original result URLs to open and verify. Prefer primary sources."
                    },
                    "query": {
                        "type": "string",
                        "maxLength": 500,
                        "description": "Optional research question or keywords used to rank relevant excerpts."
                    },
                    "max_chars_per_url": {
                        "type": "integer",
                        "minimum": 1000,
                        "maximum": 20000,
                        "description": "Maximum excerpt characters returned per URL. Full extracted text remains in the evidence store."
                    }
                },
                "required": ["urls"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args: WebExtractArgs = match serde_json::from_value(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid web_extract arguments: {error}"),
                };
            }
        };
        let execution = execute_web_extract_with_status_updates(
            args,
            &context.store.data_dir,
            &context.tools_config.evidence_storage,
            &context.tools_config.web_search,
            &context.web_search_provenance,
        )
        .await;
        if let Err(error) = context
            .store
            .update_web_search_key_statuses(&execution.status_updates)
            .await
        {
            tracing::warn!(%error, "failed to persist web extraction key statuses");
        }
        execution.result
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

fn configured_working_directories_guidance(config: &AgentToolsConfig) -> String {
    if config.terminal.allowed_cwd_roots.is_empty() {
        return String::new();
    }
    let directories = serde_json::to_string(&config.terminal.allowed_cwd_roots)
        .unwrap_or_else(|_| "[]".to_string());
    format!(
        " The user configured these additional working directories: {directories}. Treat them as known candidate locations when relevant to the request. They do not replace the WeiAgent default workspace and are not an exhaustive list of paths this tool may access; approved or full-access operations can use other locations. Set cwd explicitly to make one of these directories the base for commands or relative paths; absolute paths may refer to them directly. Omitting cwd always uses the WeiAgent default workspace."
    )
}

fn build_execute_terminal_description(config: &AgentToolsConfig) -> String {
    let shell = crate::tools::shell::selected_host_shell();
    let host_guidance = shell.tool_guidance();
    let examples = shell.command_examples();
    let working_directories = configured_working_directories_guidance(config);

    format!(
        "Execute a local command and return stdout, stderr, and exit_code, or process_id when it continues running. Use it to inspect files, search repositories, run builds/tests, and manage local processes; prefer `rg` for text search and `rg --files` for file discovery. Prefer multi_edit_file for source/config edits (including outside-root paths via sandbox_permissions=require_escalated + justification) instead of rewriting files through shell. Selected shell: {}. {host_guidance} {examples} The server enforces its current execution policy; request elevated access with sandbox_permissions=require_escalated and a justification when needed. Run ordinary commands in the foreground; use background only for long-lived services and tty only for interactive programs. Continue a returned process_id with terminal_session instead of starting the command again.{working_directories}",
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
    use crate::tools::terminal::MAX_SESSION_WAIT_MS;

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
        let description = super::build_execute_terminal_description(&AgentToolsConfig::default());
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
        assert_eq!(
            session_tool.function.parameters["properties"]["wait_ms"]["maximum"],
            serde_json::json!(MAX_SESSION_WAIT_MS)
        );
        assert!(
            terminal.function.parameters["properties"]["tty"]["description"]
                .as_str()
                .unwrap()
                .contains("Codex PTY")
        );
    }

    #[test]
    fn terminal_tool_schemas_include_configured_working_directories_as_guidance() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        config.terminal.allowed_cwd_roots = vec![
            "D:/Projects/alpha".to_string(),
            "D:/Clients/beta".to_string(),
        ];
        let encoded_directories =
            serde_json::to_string(&config.terminal.allowed_cwd_roots).unwrap();
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();

        for tool_name in ["execute_terminal", "multi_edit_file"] {
            let description = &definitions
                .iter()
                .find(|definition| definition.function.name == tool_name)
                .unwrap()
                .function
                .description;
            assert!(description.contains(&encoded_directories));
            assert!(description.contains("known candidate locations"));
            assert!(description.contains("do not replace the WeiAgent default workspace"));
            assert!(description.contains("not an exhaustive list"));
            assert!(description.contains("Omitting cwd always uses"));
        }
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
        assert_eq!(names.len(), 6);

        config.terminal.enabled = false;
        let terminal_disabled = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let names = terminal_disabled
            .iter()
            .map(|definition| definition.function.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["web_extract", "web_search", "web_search_sources"]
        );
    }

    #[test]
    fn web_search_group_is_stable_across_key_and_health_changes() {
        let mut config = AgentToolsConfig::default();
        config.web_search.enabled = true;
        let registry = ToolRegistry::with_builtin_tools();
        let before = registry.definitions(&config).unwrap();
        let names = before
            .iter()
            .filter_map(|item| {
                item.function
                    .name
                    .starts_with("web_")
                    .then_some(item.function.name.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["web_extract", "web_search", "web_search_sources"]
        );
        let before_hash = crate::llm::cache::tools_hash(Some(&before));

        config.web_search.sources[0].api_keys = vec!["sk-test".to_string()];
        config.web_search.sources[0].key_statuses =
            vec![Some(crate::domain::WebSearchKeyStatusRecord {
                status: "rate_limited".to_string(),
                detail: "dynamic".to_string(),
                checked_at: chrono::Utc::now().to_rfc3339(),
            })];
        config.web_search.sources[0].priority = 999;
        for source in &mut config.web_search.sources {
            source.enabled = false;
        }
        let after = registry.definitions(&config).unwrap();
        assert_eq!(before_hash, crate::llm::cache::tools_hash(Some(&after)));
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
            vec![
                "execute_terminal",
                "multi_edit_file",
                "terminal_session",
                "web_extract",
                "web_search",
                "web_search_sources",
            ]
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
                .is_some()
        );
        assert!(
            multi.function.parameters["properties"]
                .get("edits")
                .is_some()
        );
        assert!(
            multi.function.parameters["properties"]
                .get("files")
                .is_some()
        );
        assert_eq!(multi.function.parameters["required"], serde_json::json!([]));
    }
}
