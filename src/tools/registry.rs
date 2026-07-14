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
    fn definition(&self) -> ToolDefinition;
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
        self.tools
            .insert(tool.definition().function.name.clone(), Arc::new(tool));
    }

    pub fn definitions(&self, config: &AgentToolsConfig) -> Option<Vec<ToolDefinition>> {
        let definitions: Vec<_> = self
            .tools
            .values()
            .filter(|tool| tool.is_enabled(config))
            .map(|tool| tool.definition())
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
    fn definition(&self) -> ToolDefinition {
        let description = format!(
            "Execute a local terminal command. {} In request-approval mode, commands run inside an OS-enforced workspace sandbox with network access blocked. Use sandbox_permissions='require_escalated' only when the user explicitly asked to write outside the workspace, use the internet, or otherwise needs full host access, and include a short justification. If a sandboxed attempt is denied, retry with escalation instead of inventing success. In full-access mode commands run directly without approval. For the default workspace, omit cwd, pass an empty value, or use '.'. Set cwd only to a directory known to exist; never guess paths such as /tmp. Set background=true for long-running services.",
            terminal_shell_guidance()
        );
        tool_definition(
            "execute_terminal",
            &description,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "cwd": { "type": "string", "description": "Optional existing host directory. Omit it, leave it empty, or use '.' for the configured workspace root. Never invent a temporary directory." },
                    "timeout_ms": { "type": "integer", "minimum": 1, "description": "Foreground wait timeout before the command continues in background" },
                    "background": { "type": "boolean", "description": "Start immediately in background and return a process_id" },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Use the OS sandbox by default. Request escalation only for external writes, internet access, or another explicit need for full host access."
                    },
                    "justification": {
                        "type": "string",
                        "description": "Required when sandbox_permissions is require_escalated. Explain why full host access is needed."
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
    fn definition(&self) -> ToolDefinition {
        tool_definition(
            "get_terminal_process",
            "Get the latest status and captured stdout/stderr for a background terminal process.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process_id": { "type": "string" }
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
    fn definition(&self) -> ToolDefinition {
        tool_definition(
            "stop_terminal_process",
            "Stop a running background terminal process owned by this agent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "process_id": { "type": "string" }
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

fn terminal_shell_guidance() -> &'static str {
    if cfg!(windows) {
        "The host is Windows and command is interpreted by cmd.exe. Use Windows-compatible commands and paths; do not assume a POSIX shell, /tmp, sh heredocs, python3, ls, or pwd are available."
    } else {
        "The host is Unix-like and command is interpreted by a POSIX shell."
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
            "interpreted by a POSIX shell"
        };
        assert!(terminal.function.description.contains(expected_shell));
        assert!(terminal.function.description.contains("omit cwd"));
    }
}
