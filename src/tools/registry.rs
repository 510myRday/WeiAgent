use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
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
    pub event_sender: Option<mpsc::UnboundedSender<ToolExecutionEvent>>,
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

#[async_trait]
impl AgentTool for ExecuteTerminalTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition(
            "execute_terminal",
            "Execute a command in the configured terminal sandbox. Set background=true for long-running services. If a foreground command exceeds timeout_ms it automatically continues in the background and returns a process_id.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "cwd": { "type": "string", "description": "Optional host working directory inside an allowed sandbox root" },
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
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let cwd = arguments
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string);
        let timeout_ms = arguments.get("timeout_ms").and_then(Value::as_u64);
        let background = arguments.get("background").and_then(Value::as_bool);
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
            command: command.clone(),
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
            })
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
                    .always_allow_terminal_command(context.agent_id, &command)
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
        let process_id = arguments
            .get("process_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(info) = context.process_manager.get(process_id).await else {
            return ToolResult::Error {
                message: format!("Terminal process '{process_id}' was not found"),
            };
        };
        if info.agent_id != context.agent_id {
            return ToolResult::Blocked {
                reason: "The process belongs to another agent".to_string(),
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
        let process_id = arguments
            .get("process_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(existing) = context.process_manager.get(process_id).await else {
            return ToolResult::Error {
                message: format!("Terminal process '{process_id}' was not found"),
            };
        };
        if existing.agent_id != context.agent_id {
            return ToolResult::Blocked {
                reason: "The process belongs to another agent".to_string(),
            };
        }
        if existing.status != ProcessStatus::Running {
            return process_info_to_result(existing);
        }
        match context.process_manager.stop(process_id).await {
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
