use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    domain::{AgentToolsConfig, ApprovalDecision, PendingApproval, ProcessStatus, ToolResult},
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::JsonStore,
    tools::{
        approval::ApprovalManager,
        execute::{
            ExecuteTerminalRequest, SandboxPermissions, execute_terminal, process_info_to_result,
        },
        process_manager::ProcessManager,
        runtime::SandboxRuntimeManager,
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
    pub process_manager: Arc<ProcessManager>,
    pub approval_manager: Arc<ApprovalManager>,
    pub sandbox_runtime: Arc<SandboxRuntimeManager>,
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
        registry.register(GetTerminalProcessTool);
        registry.register(StopTerminalProcessTool);
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
        let definitions: Vec<_> = self
            .tools
            .values()
            .filter(|tool| tool.is_enabled(config))
            .map(|tool| tool.definition(config))
            .collect();
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
    sandbox_permissions: SandboxPermissions,
    #[serde(default)]
    justification: Option<String>,
}

#[async_trait]
impl AgentTool for ExecuteTerminalTool {
    fn definition(&self, config: &AgentToolsConfig) -> ToolDefinition {
        let description = build_execute_terminal_description(config);
        let default_timeout = config.terminal.default_timeout_ms;
        let permission = match config.terminal.permission_mode {
            crate::domain::TerminalPermissionMode::RequestApproval => "request_approval",
            crate::domain::TerminalPermissionMode::FullAccess => "full_access",
        };
        tool_definition(
            "execute_terminal",
            &description,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": format!(
                            "Shell script to run in the host default shell. {}. Max length 8000 bytes. Non-interactive only (stdin closed). Prefer short focused commands; chain with shell operators when needed.",
                            crate::tools::prompt::host_shell_label()
                        )
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Optional existing working directory. Omit it, leave it empty, or use '.' to use the configured default workspace root. Relative paths resolve against that root. Absolute paths must already exist. Never invent paths such as /tmp when the directory is unknown."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 120000,
                        "description": format!(
                            "Foreground wait budget in milliseconds before an unfinished command is promoted to background and returns process_id. Default {default_timeout}. Max 120000."
                        )
                    },
                    "background": {
                        "type": "boolean",
                        "description": "If true, start immediately as a managed background process and return process_id without waiting. Use for servers/watchers. Default false."
                    },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": format!(
                            "Execution privilege request. Current agent mode is {permission}. use_default follows that mode (OS sandbox in request_approval, direct host in full_access). require_escalated asks for full host access and, in request_approval mode, needs user approval plus justification."
                        )
                    },
                    "justification": {
                        "type": "string",
                        "description": "Short reason for full host access. Required in request_approval mode when sandbox_permissions is require_escalated. Max 2000 bytes. Skip it for use_default."
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
            timeout == 0 || timeout > crate::tools::execute::MAX_TERMINAL_TIMEOUT_MS
        }) {
            return ToolResult::Error {
                message: format!(
                    "timeout_ms must be between 1 and {}",
                    crate::tools::execute::MAX_TERMINAL_TIMEOUT_MS
                ),
            };
        }
        let command = args.command;
        let timeout_ms = args.timeout_ms;
        let background = args.background;
        let sandbox_permissions = args.sandbox_permissions;
        let justification = args.justification;
        if context.tools_config.terminal.permission_mode
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
                sandbox_permissions,
                approved: false,
                agent_id: context.agent_id,
                conversation_id: context.conversation_id,
                tool_call_id: context.call_id.clone(),
            },
            &context.tools_config.terminal,
            &context.process_manager,
            &context.sandbox_runtime,
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
            cwd: cwd.clone().or_else(|| {
                context
                    .tools_config
                    .terminal
                    .allowed_cwd_roots
                    .first()
                    .cloned()
            }),
            runtime: "full_access".to_string(),
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
                approval_id,
                cwd: cwd.clone(),
                runtime: "full_access".to_string(),
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

        let decision = tokio::select! {
            decision = decision_rx => decision.ok(),
            _ = event_sender.closed() => None,
        };
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

        result = execute_terminal(
            &ExecuteTerminalRequest {
                command,
                cwd,
                timeout_ms,
                background,
                sandbox_permissions,
                approved: true,
                agent_id: context.agent_id,
                conversation_id: context.conversation_id,
                tool_call_id: context.call_id,
            },
            &context.tools_config.terminal,
            &context.process_manager,
            &context.sandbox_runtime,
        )
        .await;
        result
    }
}

struct GetTerminalProcessTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessToolArgs {
    process_id: String,
}

#[async_trait]
impl AgentTool for GetTerminalProcessTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "get_terminal_process",
            "Read status and captured stdout/stderr for a terminal process started by this agent in this conversation. Use after execute_terminal returns process_id (background=true or foreground timeout promotion). Returns running/completed/failed/stopped state, exit_code when finished, duration_ms, and truncated output tails. Only processes owned by the current agent and conversation are visible.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process_id": {
                        "type": "string",
                        "description": "UUID process_id returned by execute_terminal when the command is still managed in the background."
                    }
                },
                "required": ["process_id"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args = match serde_json::from_value::<ProcessToolArgs>(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid get_terminal_process arguments: {error}"),
                };
            }
        };
        let process_id = args.process_id;
        if Uuid::parse_str(&process_id).is_err() {
            return ToolResult::Error {
                message: "process_id must be a UUID".to_string(),
            };
        }
        let Some(info) = context.process_manager.get(&process_id).await else {
            return ToolResult::Error {
                message: format!("Terminal process '{process_id}' was not found"),
            };
        };
        if info.agent_id != context.agent_id || info.conversation_id != context.conversation_id {
            return ToolResult::Blocked {
                reason: "The process belongs to another agent run".to_string(),
            };
        }
        process_info_to_result(info)
    }
}

struct StopTerminalProcessTool;

#[async_trait]
impl AgentTool for StopTerminalProcessTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "stop_terminal_process",
            "Stop a running background terminal process owned by this agent in this conversation. Use when a long-running command should end early. If the process already finished, returns its final captured status instead of failing. Only processes owned by the current agent and conversation can be stopped.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process_id": {
                        "type": "string",
                        "description": "UUID process_id returned by execute_terminal for a managed background process."
                    }
                },
                "required": ["process_id"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args = match serde_json::from_value::<ProcessToolArgs>(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid stop_terminal_process arguments: {error}"),
                };
            }
        };
        let process_id = args.process_id;
        if Uuid::parse_str(&process_id).is_err() {
            return ToolResult::Error {
                message: "process_id must be a UUID".to_string(),
            };
        }
        let Some(existing) = context.process_manager.get(&process_id).await else {
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
        if existing.status != ProcessStatus::Running {
            return process_info_to_result(existing);
        }
        match context.process_manager.stop(&process_id).await {
            Ok(Some(info)) => process_info_to_result(info),
            Ok(None) => ToolResult::Error {
                message: format!("Terminal process '{process_id}' was not found"),
            },
            Err(error) => ToolResult::Error {
                message: format!("Failed to stop process: {error}"),
            },
        }
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

fn build_execute_terminal_description(config: &AgentToolsConfig) -> String {
    let shell = crate::tools::prompt::host_shell_guidance();
    let default_timeout = config.terminal.default_timeout_ms;
    let permission = match config.terminal.permission_mode {
        crate::domain::TerminalPermissionMode::RequestApproval => {
            "Current permission mode is request_approval: default runs use the OS workspace sandbox (network denied; outside-workspace writes denied). Request sandbox_permissions=\"require_escalated\" only for explicit full-host needs and include justification; the user must approve unless an always-allow grant already matches."
        }
        crate::domain::TerminalPermissionMode::FullAccess => {
            "Current permission mode is full_access: commands run directly on the host without sandboxing or approval prompts. Still prefer the configured workspace unless the user asked for another path."
        }
    };
    let examples = if cfg!(windows) {
        "Examples of valid command strings: `dir`, `type README.md`, `where python`, `cd /d C:\\path\\to\\project && cargo test -q`, `echo hello> out.txt`."
    } else {
        "Examples of valid command strings: `ls -la`, `sed -n '1,40p' README.md`, `rg --files`, `cd /path/to/project && cargo test -q`, `printf 'hello\\n' > out.txt`."
    };

    format!(
        "Execute a local terminal command and return stdout/stderr/exit_code (or process_id if still running). {shell} {permission} \
For the default workspace, omit cwd, pass an empty value, or use '.'. Set cwd only to a directory known to exist; never substitute a guessed path. \
Foreground commands wait up to timeout_ms (default {default_timeout}, max 120000) and then continue in background with a process_id. Set background=true for services/watchers. \
Ordinary completed foreground commands do not stay listed as background processes. Use get_terminal_process / stop_terminal_process with returned process_ids. \
Returned stdout/stderr tails are limited to the last 64KB. Commands are non-interactive. If a sandboxed attempt is denied, report the failure and escalate only when user intent requires it; never invent success. {examples}"
    )
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
    use super::{ToolRegistry, normalize_optional_cwd};
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
        let expected_shell = if cfg!(windows) {
            "interpreted by cmd.exe"
        } else {
            "POSIX shell"
        };
        assert!(terminal.function.description.contains(expected_shell));
        assert!(terminal.function.description.contains("omit cwd"));
        assert!(terminal.function.description.contains("background=true"));
        assert!(terminal.function.parameters["properties"]["timeout_ms"]["description"]
            .as_str()
            .unwrap()
            .contains("30000"));
        assert!(
            definitions
                .iter()
                .any(|definition| definition.function.name == "get_terminal_process")
        );
        assert!(
            definitions
                .iter()
                .any(|definition| definition.function.name == "stop_terminal_process")
        );
    }

    #[test]
    fn terminal_definition_reflects_full_access_mode() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        config.terminal.permission_mode = crate::domain::TerminalPermissionMode::FullAccess;
        config.terminal.default_timeout_ms = 45_000;
        let definitions = ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .unwrap();
        let terminal = definitions
            .iter()
            .find(|definition| definition.function.name == "execute_terminal")
            .unwrap();
        assert!(terminal.function.description.contains("full_access"));
        assert!(terminal.function.parameters["properties"]["timeout_ms"]["description"]
            .as_str()
            .unwrap()
            .contains("45000"));
    }
}
