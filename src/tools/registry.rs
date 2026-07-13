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
        execute::{ExecuteTerminalRequest, execute_terminal, process_info_to_result},
        process_manager::ProcessManager,
    },
};

#[derive(Debug, Clone)]
pub enum ToolExecutionEvent {
    ApprovalRequired {
        call_id: String,
        command: String,
        approval_id: String,
        cwd: Option<String>,
        runtime: String,
        network_enabled: bool,
        write_roots: Vec<String>,
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
}

#[async_trait]
impl AgentTool for ExecuteTerminalTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition(
            "execute_terminal",
            "Execute a command in the configured terminal sandbox. Omit cwd to use the configured workspace root; never invent or guess a cwd. Only set cwd when the user explicitly requires a known existing directory inside an allowed root. Set background=true for long-running services. If a foreground command exceeds timeout_ms it automatically continues in the background and returns a process_id. A blocked or error result means the command did not run; do not claim success or invent an exit code.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "cwd": { "type": "string", "description": "Optional existing host directory inside an allowed sandbox root. Omit this field to use the configured workspace; never guess a path." },
                    "timeout_ms": { "type": "integer", "minimum": 1, "description": "Foreground wait timeout before the command continues in background" },
                    "background": { "type": "boolean", "description": "Start immediately in background and return a process_id" }
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
        if args
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd.len() > 4_096 || cwd.trim().is_empty())
        {
            return ToolResult::Error {
                message: "cwd must be a non-empty path no longer than 4096 bytes".to_string(),
            };
        }
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
        let cwd = args.cwd;
        let timeout_ms = args.timeout_ms;
        let background = args.background;
        let mut result = execute_terminal(
            &ExecuteTerminalRequest {
                command: command.clone(),
                cwd: cwd.clone(),
                timeout_ms,
                background,
                approved: false,
                agent_id: context.agent_id,
                conversation_id: context.conversation_id,
                tool_call_id: context.call_id.clone(),
            },
            &context.tools_config.terminal,
            &context.process_manager,
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
            runtime: match context.tools_config.terminal.sandbox.mode {
                crate::domain::SandboxMode::Docker => "docker".to_string(),
                crate::domain::SandboxMode::NativeRestricted => "native_unsandboxed".to_string(),
            },
            network_enabled: match context.tools_config.terminal.sandbox.mode {
                crate::domain::SandboxMode::Docker => {
                    context.tools_config.terminal.sandbox.network_enabled
                }
                crate::domain::SandboxMode::NativeRestricted => true,
            },
            write_roots: context.tools_config.terminal.allowed_cwd_roots.clone(),
            background: background.unwrap_or(false),
        });
        let _approval_guard = ApprovalRegistrationGuard {
            manager: context.approval_manager.clone(),
            approval_id: approval_id.clone(),
        };
        if event_sender
            .send(ToolExecutionEvent::ApprovalRequired {
                call_id: context.call_id.clone(),
                command: command.clone(),
                approval_id,
                cwd: cwd.clone(),
                runtime: match context.tools_config.terminal.sandbox.mode {
                    crate::domain::SandboxMode::Docker => "docker".to_string(),
                    crate::domain::SandboxMode::NativeRestricted => {
                        "native_unsandboxed".to_string()
                    }
                },
                network_enabled: match context.tools_config.terminal.sandbox.mode {
                    crate::domain::SandboxMode::Docker => {
                        context.tools_config.terminal.sandbox.network_enabled
                    }
                    crate::domain::SandboxMode::NativeRestricted => true,
                },
                write_roots: context.tools_config.terminal.allowed_cwd_roots.clone(),
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
                    .always_allow_terminal_command(
                        context.agent_id,
                        &command,
                        cwd.as_deref(),
                        &context.tools_config.terminal.sandbox,
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
                approved: true,
                agent_id: context.agent_id,
                conversation_id: context.conversation_id,
                tool_call_id: context.call_id,
            },
            &context.tools_config.terminal,
            &context.process_manager,
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

struct ApprovalRegistrationGuard {
    manager: Arc<ApprovalManager>,
    approval_id: String,
}

impl Drop for ApprovalRegistrationGuard {
    fn drop(&mut self) {
        self.manager.cancel(&self.approval_id);
    }
}
