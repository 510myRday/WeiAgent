use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::{
    AgentTool, ToolExecutionContext, build_execute_terminal_description,
    frozen_terminal_config, normalize_optional_cwd, run_with_approval, terminal_request,
    tool_definition, unsupported_host_shell_syntax,
};
use crate::{
    domain::{AgentToolsConfig, ToolResult},
    llm::types::ToolDefinition,
    tools::terminal::{
        MAX_SESSION_WAIT_MS, SandboxPermissions, TerminalSessionAction,
        execute_terminal, process_read_to_result,
    },
};
pub(super) struct ExecuteTerminalTool;

pub(super) const MAX_TERMINAL_COMMAND_BYTES: usize = 8_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecuteTerminalArgs {
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
        let terminal_config = frozen_terminal_config(&context);
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
            &terminal_request(&command, &cwd, timeout_ms, background, tty, sandbox_permissions, false, &context),
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
                    &terminal_request(&command, &cwd, timeout_ms, background, tty, sandbox_permissions, true, &context),
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

pub(super) struct TerminalSessionTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TerminalSessionArgs {
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
