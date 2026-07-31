use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    domain::{AgentToolsConfig, ApprovalDecision, PendingApproval, ToolResult},
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::Store,
    tools::{
        approval::ApprovalManager,
        multi_edit::{
            MULTI_EDIT_GRANT_COMMAND, MULTI_EDIT_TOOL_NAME, MultiEditFileArgs, MultiEditRequest,
            execute_multi_edit, parse_multi_edit_file_args,
        },
        task_planning::{TASK_PLANNING_TOOL_NAME, TaskPlanningTool},
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
    pub store: Arc<Store>,
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
        registry.register(TaskPlanningTool);
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
        if config.task_planning.enabled {
            for definition in &mut definitions {
                if definition.function.name != TASK_PLANNING_TOOL_NAME {
                    add_planning_id_parameter(definition);
                }
            }
        }
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
                code: None,
                details: None,
            };
        }
        let mut tool_arguments = arguments;
        if context.tools_config.task_planning.enabled && name != TASK_PLANNING_TOOL_NAME {
            let planning_argument = tool_arguments
                .get("planning_id")
                .or_else(|| tool_arguments.get("planningId"));
            let planning_id = planning_argument
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let active_plan = context
                .store
                .read_task_plan(context.conversation_id)
                .await
                .ok()
                .flatten();
            let valid = planning_id
                .and_then(|value| Uuid::parse_str(value).ok())
                .zip(active_plan.as_ref().map(|plan| plan.planning_id))
                .is_some_and(|(provided, active)| provided == active);
            if !valid {
                let invalid_argument = planning_argument.is_some_and(|value| {
                    !value.is_null()
                        && !value
                            .as_str()
                            .is_some_and(|planning_id| !planning_id.trim().is_empty())
                });
                let code = if planning_id.is_some() || invalid_argument {
                    "PlanningIdInvalid"
                } else {
                    "PlanningRequired"
                };
                return ToolResult::Blocked {
                    reason: "请先调用 task_planning 创建计划，并将返回的 planning_id 传入本工具。"
                        .to_string(),
                    code: Some(code.to_string()),
                    details: Some(serde_json::json!({
                        "tool": name,
                        "planning_id": planning_id,
                        "required_argument": "planning_id"
                    })),
                };
            }
            if let Some(object) = tool_arguments.as_object_mut() {
                object.remove("planning_id");
                object.remove("planningId");
            }
        }
        tool.execute(tool_arguments, context).await
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
                        "description": "Existing working directory. Omit to use the default workspace; relative paths resolve from it. If the requested directory is outside the configured workspace roots, the server automatically asks the user before execution. sandbox_permissions=require_escalated with justification may also be used to describe the requested expanded access."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 120000,
                        "description": "Foreground wait in milliseconds. If omitted, the server default is 30000 ms. Maximum is 120000. An unfinished command returns process_id when sessions are supported; the runtime may stop it on timeout."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "If true, start immediately as a managed background process and return process_id without waiting. Use only for long-lived servers/watchers/dev services. Leave false (default) for ordinary short commands."
                    },
                    "tty": {
                        "type": "boolean",
                        "description": "Request a PTY for a genuinely interactive command. Use for REPLs, interactive CLIs, or software that requires a terminal. Leave false for ordinary commands."
                    },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Execution privilege request. use_default follows the server's current policy. The server automatically requests approval for a cwd outside the configured workspace roots. Use require_escalated with a justification when expanded access is explicitly requested."
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
        let result = execute_terminal(
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
        let approval_cwd = cwd
            .clone()
            .or_else(|| Some(context.store.terminal_workspace_dir.display().to_string()));
        run_with_approval(
            &context,
            approval_id,
            command.clone(),
            approval_cwd,
            "local_shell",
            "full_access",
            justification,
            background.unwrap_or(false),
            "This command requires interactive approval; use the SSE message endpoint",
            "Command rejected by user or approval expired",
            &command,
            cwd.as_deref(),
            async |terminal_config| {
                execute_terminal(
                    &ExecuteTerminalRequest {
                        command: command.clone(),
                        cwd: cwd.clone(),
                        timeout_ms,
                        background,
                        tty,
                        sandbox_permissions,
                        approved: true,
                        agent_id: context.agent_id,
                        conversation_id: context.conversation_id,
                        tool_call_id: context.call_id.clone(),
                    },
                    &terminal_config,
                    &context.store.terminal_workspace_dir,
                    &context.terminal_manager,
                )
                .await
            },
        )
        .await
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
                code: None,
                details: None,
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
            "Edit, create, delete, or move text files in one atomic call. Prefer this over shell redirection or execute_terminal file rewrites for source, config, and documentation changes. Every operation is validated before anything is written; on failure files are left unchanged. Result warnings (for example possibly unbalanced delimiters) do not block the write; inspect them before continuing. For an edit, read the current file first when it is not already in context, keep each old_string short but unique, and put all replacements for one path in one edit operation. To fully rewrite an existing file, use a single create operation with overwrite=true instead of many edits. `operations` must be a JSON array, and each object inside it must include its own `type` field: `edit`, `create`, `delete`, or `move`; `type` is not a top-level field. Order edits so a later old_string never targets text produced by an earlier new_string; such edits are rejected. In TS/JS template strings write ${{...}} exactly; do not escape the dollar sign as \\${{...}}. In-workspace edits may require approval under request_approval. When cwd or a target path is outside the configured workspace roots, the server requests approval automatically before writing; sandbox_permissions=require_escalated with a short justification may also be supplied to make the expanded access request explicit. Do not bypass multi_edit_file via shell when an outside-root edit is needed.{}",
            configured_working_directories_guidance(config)
        );
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
                        "description": "Preferred ordered file operations. A path may appear only once per call. Pass a JSON array, not a quoted JSON string. Each array object must include its own type field; type belongs inside operations[], not at the top level.",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "description": "Apply exact text replacements inside one existing file. Include type=edit in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "path", "edits"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["edit"]},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "File to edit, relative to cwd unless absolute."},
                                        "edits": {
                                            "type": "array",
                                            "minItems": 1,
                                            "maxItems": 64,
                                            "description": "Ordered replacements for this file, validated and applied in memory before any write.",
                                            "items": {
                                                "type": "object",
                                                "additionalProperties": false,
                                                "required": ["old_string", "new_string"],
                                                "properties": {
                                                    "old_string": {"type": "string", "maxLength": 200000, "description": "Exact text copied from the current file, including indentation, without line-number prefixes. Compared after normalizing CRLF to LF."},
                                                    "new_string": {"type": "string", "maxLength": 2097152, "description": "Replacement text. Must differ from old_string."},
                                                    "replace_all": {"type": "boolean", "description": "Replace every occurrence of old_string. Defaults to false."},
                                                    "start_line": {"type": "integer", "minimum": 1, "description": "Optional 1-based line hint used only when old_string matches more than once; the occurrence closest to this line is replaced."}
                                                }
                                            }
                                        }
                                    }
                                },
                                {
                                    "type": "object",
                                    "description": "Create a new file, or fully rewrite an existing one with overwrite=true. Include type=create in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "path", "content"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["create"]},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "File to create. Parent directories are created automatically."},
                                        "content": {"type": "string", "maxLength": 2097152, "description": "Full file content to write."},
                                        "overwrite": {"type": "boolean", "description": "Defaults to false, which fails when the target already exists. Set true to replace the existing file; this is the preferred way to fully rewrite a file."}
                                    }
                                },
                                {
                                    "type": "object",
                                    "description": "Delete one file. Directories are refused. Include type=delete in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "path"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["delete"]},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "File to delete."}
                                    }
                                },
                                {
                                    "type": "object",
                                    "description": "Move or rename one file. Directories are refused. Include type=move in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "from", "to"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["move"]},
                                        "from": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "Source file."},
                                        "to": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "Target path. Parent directories are created automatically."},
                                        "overwrite": {"type": "boolean", "description": "Defaults to false, which fails when the target already exists. Set true to replace the target file."}
                                    }
                                }
                            ]
                        }
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Existing working directory for relative paths; a ~ prefix expands to the user home directory. Omit to use the default workspace. If cwd or target files are outside the configured workspace roots, the server automatically asks the user before writing."
                    },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Write privilege request. use_default follows the server policy and automatically requests approval for outside-root cwd or target paths. Use require_escalated with a short justification when expanded access should be explicit. Prefer this over rewriting files through execute_terminal."
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
        let args: MultiEditFileArgs = match parse_multi_edit_file_args(arguments) {
            Ok(args) => args,
            Err(message) => return ToolResult::Error { message },
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

        let result = execute_multi_edit(
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
        let approval_cwd = cwd
            .clone()
            .or_else(|| Some(context.store.terminal_workspace_dir.display().to_string()));
        let justification = justification
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some("Apply multi_edit_file changes".to_string()));
        run_with_approval(
            &context,
            approval_id,
            command,
            approval_cwd.clone(),
            "builtin_multi_edit",
            "file_write",
            justification,
            false,
            "This edit requires interactive approval; use the SSE message endpoint",
            "Edit rejected by user or approval expired",
            MULTI_EDIT_GRANT_COMMAND,
            approval_cwd.as_deref(),
            async |terminal_config| {
                execute_multi_edit(
                    &MultiEditRequest {
                        args,
                        approved: true,
                    },
                    &terminal_config,
                    &context.store.terminal_workspace_dir,
                )
                .await
            },
        )
        .await
    }
}

struct WebSearchTool;

#[async_trait]
impl AgentTool for WebSearchTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_search",
            "Use for browsing, current information, source links, precise attribution, or information likely to have changed. Search complementary sources in parallel and return deduplicated evidence candidates, per-source diagnostics, and separately labeled AI research syntheses. Use fast for simple lookups, balanced for current facts and comparisons, and deep for comprehensive research or conflicting claims. For research, use complementary query rewrites, multiple independent source groups, and materially different retries when a search is empty. AI research syntheses are hypotheses and query directions, not verified evidence or independent sources; use web_extract on important URLs before citing them.",
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
            "List the currently configured web search sources, their roles, strengths, limitations, cost/latency tiers, evidence type, independence group, capabilities, and health. Use this before custom source selection so searches can use appropriate independent source groups; source state is dynamic and is intentionally not embedded in the stable web_search schema.",
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
            "Open one to five public HTTP(S) URLs for source verification. Prefer the most relevant original or primary pages, especially for important or disputed claims. Verify important or disputed claims with independent evidence, then cite the original page with a Markdown link next to the supported claim. Uses local extraction first, then configured URL-content providers when local content is blocked, dynamic, or low quality. Stores direct evidence, returns focused excerpts, and labels Grok SSO fallback output as AI-derived.",
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
pub(crate) fn tool_definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        kind: "function".to_string(),
        function: FunctionDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

fn add_planning_id_parameter(definition: &mut ToolDefinition) {
    let Some(schema) = definition.function.parameters.as_object_mut() else {
        return;
    };
    let Some(properties) = schema
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
    else {
        return;
    };
    properties.insert(
        "planning_id".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Required when task planning is enabled. Use the planning_id returned by task_planning."
        }),
    );
    let required = schema
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(required) = required.as_array_mut()
        && !required
            .iter()
            .any(|value| value.as_str() == Some("planning_id"))
    {
        required.push(Value::String("planning_id".to_string()));
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
        "Execute a local command and return stdout, stderr, and exit_code, or process_id when it continues running. Use it to inspect files, search repositories, run builds/tests, and manage local processes; prefer `rg` for text search and `rg --files` for file discovery. WeiAgent includes a private ripgrep binary in released builds and adds it to the command PATH; if `rg` is unavailable, use the selected shell's built-in file search or grep fallback. Prefer multi_edit_file for source/config edits (including outside-root paths via sandbox_permissions=require_escalated + justification) instead of rewriting files through shell. Selected shell: {}. {host_guidance} {examples} The server enforces its current execution policy; request elevated access with sandbox_permissions=require_escalated and a justification when needed. Run ordinary commands in the foreground; use background only for long-lived services and tty only for interactive programs. Continue a returned process_id with terminal_session instead of starting the command again.{working_directories}",
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

/// Shared interactive-approval flow: register the pending approval, notify the
/// SSE client, wait for a decision (auto-approving if the agent is switched to
/// full access), persist an always-allow grant when requested, then re-execute
/// with `approved = true` under a freshly reloaded terminal config.
#[allow(clippy::too_many_arguments)]
async fn run_with_approval(
    context: &ToolExecutionContext,
    approval_id: String,
    command: String,
    approval_cwd: Option<String>,
    runtime: &str,
    permission: &str,
    justification: Option<String>,
    background: bool,
    interactive_required_reason: &str,
    reject_reason: &str,
    grant_command: &str,
    grant_cwd: Option<&str>,
    re_execute: impl AsyncFnOnce(crate::domain::TerminalToolConfig) -> ToolResult,
) -> ToolResult {
    let Some(event_sender) = context.event_sender.clone() else {
        return ToolResult::Rejected {
            reason: interactive_required_reason.to_string(),
        };
    };
    let decision_rx = context.approval_manager.register(PendingApproval {
        approval_id: approval_id.clone(),
        agent_id: context.agent_id,
        conversation_id: context.conversation_id,
        tool_call_id: context.call_id.clone(),
        command: command.clone(),
        cwd: approval_cwd.clone(),
        runtime: runtime.to_string(),
        permission: permission.to_string(),
        justification: justification.clone(),
        background,
    });
    let _approval_guard = ApprovalRegistrationGuard {
        manager: context.approval_manager.clone(),
        approval_id: approval_id.clone(),
    };
    if event_sender
        .send(ToolExecutionEvent::ApprovalRequired {
            call_id: context.call_id.clone(),
            agent_id: context.agent_id,
            command,
            approval_id: approval_id.clone(),
            cwd: approval_cwd,
            runtime: runtime.to_string(),
            permission: permission.to_string(),
            justification,
            background,
        })
        .await
        .is_err()
    {
        return ToolResult::Rejected {
            reason: "Approval client disconnected".to_string(),
        };
    }

    let decision = wait_for_approval_decision(
        context,
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
                .always_allow_terminal_command(context.agent_id, grant_command, grant_cwd)
                .await
            {
                return ToolResult::Error {
                    message: format!("Failed to persist always-allow rule: {error}"),
                };
            }
        }
        Some(ApprovalDecision::Reject) => {
            return ToolResult::Rejected {
                reason: reject_reason.to_string(),
            };
        }
        None => {
            return ToolResult::Rejected {
                reason: "Approval client disconnected".to_string(),
            };
        }
    }

    let terminal_config = match live_terminal_config(context).await {
        Ok(config) => config,
        Err(message) => return ToolResult::Error { message },
    };
    re_execute(terminal_config).await
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
    use crate::domain::{AgentToolsConfig, TaskPlanItem, TaskPlanItemStatus, ToolResult};
    use crate::tools::terminal::MAX_SESSION_WAIT_MS;
    use serde_json::json;
    use std::sync::Arc;
    use uuid::Uuid;

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
                .contains("PTY")
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
        assert!(names.contains(&"task_planning"));
        assert_eq!(names.len(), 7);

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
            vec![
                "task_planning",
                "web_extract",
                "web_search",
                "web_search_sources"
            ]
        );

        config.task_planning.enabled = false;
        let disabled = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        assert!(
            !disabled
                .iter()
                .any(|definition| definition.function.name == "task_planning")
        );
        assert!(disabled.iter().all(|definition| {
            definition.function.parameters["properties"]
                .get("planning_id")
                .is_none()
        }));
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
    fn web_search_schemas_carry_the_research_workflow_guidance() {
        let mut config = AgentToolsConfig::default();
        config.web_search.enabled = true;
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let definition = |name: &str| {
            definitions
                .iter()
                .find(|item| item.function.name == name)
                .unwrap()
        };

        let search = definition("web_search");
        assert!(search.function.description.contains("current information"));
        assert!(
            search
                .function
                .description
                .contains("multiple independent source groups")
        );
        assert!(
            search
                .function
                .description
                .contains("AI research syntheses are hypotheses")
        );
        assert!(search.function.description.contains("query directions"));
        assert!(
            search
                .function
                .description
                .contains("use web_extract on important URLs")
        );
        assert!(
            search.function.parameters["properties"]["mode"]["description"]
                .as_str()
                .unwrap()
                .contains("deep")
        );

        let sources = definition("web_search_sources");
        assert!(
            sources
                .function
                .description
                .contains("custom source selection")
        );
        assert!(
            sources
                .function
                .description
                .contains("independent source groups")
        );

        let extract = definition("web_extract");
        assert!(extract.function.description.contains("source verification"));
        assert!(
            extract
                .function
                .description
                .contains("original or primary pages")
        );
        assert!(
            extract
                .function
                .description
                .contains("independent evidence")
        );
        assert!(extract.function.description.contains("Markdown link"));
        assert!(
            extract.function.parameters["properties"]["urls"]["description"]
                .as_str()
                .unwrap()
                .contains("Prefer primary sources")
        );
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
                "task_planning",
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
                .is_none()
        );
        assert_eq!(
            multi.function.parameters["required"],
            serde_json::json!(["operations", "planning_id"])
        );
    }

    #[test]
    fn planning_is_injected_into_non_planning_tool_schemas() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();

        for definition in definitions {
            let name = definition.function.name;
            let properties = &definition.function.parameters["properties"];
            let required = definition.function.parameters["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if name == "task_planning" {
                assert!(
                    definition
                        .function
                        .description
                        .contains("Before using any other tool")
                );
                assert!(
                    serde_json::to_string(&definition.function.parameters)
                        .unwrap()
                        .is_ascii()
                );
                let item_variants = definition.function.parameters["properties"]["item"]["oneOf"]
                    .as_array()
                    .unwrap();
                assert_eq!(item_variants.len(), 3);
                assert!(
                    item_variants
                        .iter()
                        .any(|variant| variant["type"] == "integer")
                );
                assert!(
                    item_variants
                        .iter()
                        .any(|variant| variant["type"] == "array")
                );
                assert!(
                    item_variants
                        .iter()
                        .any(|variant| variant["type"] == "string")
                );
                assert!(!required.iter().any(|value| value == "planning_id"));
            } else {
                assert!(properties.get("planning_id").is_some(), "{name}");
                assert!(
                    required.iter().any(|value| value == "planning_id"),
                    "{name}"
                );
            }
        }
    }

    #[tokio::test]
    async fn planning_gate_returns_stable_codes_for_missing_and_invalid_ids() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::storage::Store::new(temp_dir.path()).await.unwrap());
        let context = |conversation_id| super::ToolExecutionContext {
            agent_id: Uuid::new_v4(),
            conversation_id,
            call_id: "test-call".to_string(),
            tools_config: AgentToolsConfig::default(),
            store: store.clone(),
            terminal_manager: crate::tools::terminal::TerminalManager::new_for_tests(),
            approval_manager: Arc::new(crate::tools::approval::ApprovalManager::new()),
            event_sender: None,
            web_search_provenance: Default::default(),
        };
        let registry = ToolRegistry::with_builtin_tools();
        let conversation_id = Uuid::new_v4();

        let result = registry
            .execute("web_search_sources", json!({}), context(conversation_id))
            .await;
        let ToolResult::Blocked {
            reason,
            code,
            details,
        } = result
        else {
            panic!("expected missing planning id to be blocked");
        };
        assert_eq!(
            reason,
            "请先调用 task_planning 创建计划，并将返回的 planning_id 传入本工具。"
        );
        assert_eq!(code.as_deref(), Some("PlanningRequired"));
        assert_eq!(details.unwrap()["required_argument"], "planning_id");

        let plan = store
            .create_task_plan(
                conversation_id,
                None,
                vec![TaskPlanItem {
                    step: "Use the tool".to_string(),
                    status: TaskPlanItemStatus::InProgress,
                }],
            )
            .await
            .unwrap();
        let other_conversation = Uuid::new_v4();
        let result = registry
            .execute(
                "web_search_sources",
                json!({"planning_id": plan.planning_id}),
                context(other_conversation),
            )
            .await;
        let ToolResult::Blocked { code, .. } = result else {
            panic!("expected cross-conversation planning id to be blocked");
        };
        assert_eq!(code.as_deref(), Some("PlanningIdInvalid"));

        let mut disabled_context = context(conversation_id);
        disabled_context.tools_config.task_planning.enabled = false;
        let result = registry
            .execute("web_search_sources", json!({}), disabled_context)
            .await;
        assert!(matches!(result, ToolResult::Completed { .. }));
    }
}
