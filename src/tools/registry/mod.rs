use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    domain::{AgentToolsConfig, ApprovalDecision, PendingApproval, ToolResult},
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::Store,
    tools::{
        approval::ApprovalManager,
        multi_edit::{LEGACY_MULTI_EDIT_TOOL_NAME, MULTI_FILE_OPERATION_TOOL_NAME},
        task_planning::{TASK_PLANNING_TOOL_NAME, TaskPlanningTool},
        terminal::{ExecuteTerminalRequest, SandboxPermissions, TerminalManager},
        web_search::evidence::WebSearchProvenanceCache,
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
    pub supports_multimodal: bool,
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
        registry.register(ReadImagesTool);
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
        let canonical_name = match name {
            LEGACY_MULTI_EDIT_TOOL_NAME => MULTI_FILE_OPERATION_TOOL_NAME,
            other => other,
        };
        let Some(tool) = self.tools.get(canonical_name) else {
            return ToolResult::Error {
                message: format!("Unknown tool: {name}"),
            };
        };
        if canonical_name == READ_IMAGES_TOOL_NAME && !context.supports_multimodal {
            return ToolResult::Blocked {
                reason: "The selected model is not configured for multimodal image input"
                    .to_string(),
                code: Some("MultimodalRequired".to_string()),
                details: None,
            };
        }
        if !tool.is_enabled(&context.tools_config) {
            return ToolResult::Blocked {
                reason: format!("Tool '{name}' is disabled"),
                code: None,
                details: None,
            };
        }
        let mut tool_arguments = arguments;
        if context.tools_config.task_planning.enabled && canonical_name != TASK_PLANNING_TOOL_NAME {
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
                .zip(active_plan.as_ref().map(|plan| plan.public_id()))
                .is_some_and(|(provided, active)| provided == active);
            if !valid {
                let invalid_argument = planning_argument.is_some_and(|value| {
                    !value.is_null()
                        && !value
                            .as_str()
                            .is_some_and(|planning_id| !planning_id.trim().is_empty())
                });
                let has_provided_id = planning_id.is_some() || invalid_argument;
                let (code, reason) = match active_plan.as_ref() {
                    Some(active_plan) if has_provided_id => (
                        "PlanningIdInvalid",
                        format!(
                            "本次工具调用的 planning_id 无效，或不属于当前会话。请使用当前 active 计划的 planning_id 重试：{}",
                            active_plan.public_id()
                        ),
                    ),
                    Some(active_plan) => (
                        "PlanningRequired",
                        format!(
                            "当前会话已有 active 任务计划，但本次工具调用缺少 planning_id。请使用下面的 planning_id 重试：{}",
                            active_plan.public_id()
                        ),
                    ),
                    None if has_provided_id => (
                        "PlanningIdInvalid",
                        "本次工具调用提供的 planning_id 无效，当前会话也没有 active 任务计划。请先调用 task_planning 创建计划，再使用新返回的 planning_id 重试。"
                            .to_string(),
                    ),
                    None => (
                        "PlanningRequired",
                        "当前会话没有 active 任务计划，请先调用 task_planning 创建计划，再重试本次工具调用。"
                            .to_string(),
                    ),
                };
                return ToolResult::Blocked {
                    reason,
                    code: Some(code.to_string()),
                    details: Some(serde_json::json!({
                        "tool": canonical_name,
                        "planning_id": planning_id,
                        "provided_planning_id": planning_id,
                        "active_planning_id": active_plan.as_ref().map(|plan| plan.public_id()),
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

mod image;
mod multi_edit;
mod terminal;
mod web_search;

use image::{READ_IMAGES_TOOL_NAME, ReadImagesTool};
use multi_edit::MultiEditFileTool;
use terminal::{ExecuteTerminalTool, TerminalSessionTool};
use web_search::{WebExtractTool, WebSearchSourcesTool, WebSearchTool};

pub(super) fn tool_definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        kind: "function".to_string(),
        function: FunctionDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

pub(super) fn add_planning_id_parameter(definition: &mut ToolDefinition) {
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
            "pattern": "^[0-9a-z]{6}$",
            "description": "Required when task planning is enabled. Use the six-character lowercase alphanumeric planning_id returned by task_planning."
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

#[allow(clippy::too_many_arguments)]
pub(super) fn terminal_request(
    command: &str,
    cwd: &Option<String>,
    timeout_ms: Option<u64>,
    background: Option<bool>,
    tty: Option<bool>,
    sandbox_permissions: SandboxPermissions,
    approved: bool,
    context: &ToolExecutionContext,
) -> ExecuteTerminalRequest {
    ExecuteTerminalRequest {
        command: command.to_string(),
        cwd: cwd.clone(),
        timeout_ms,
        background,
        tty,
        sandbox_permissions,
        approved,
        agent_id: context.agent_id,
        conversation_id: context.conversation_id,
        tool_call_id: context.call_id.clone(),
    }
}

pub(super) fn normalize_optional_cwd(cwd: Option<String>) -> Result<Option<String>, String> {
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

pub(super) fn unsupported_host_shell_syntax(command: &str) -> Option<String> {
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

pub(super) fn build_execute_terminal_description(_config: &AgentToolsConfig) -> String {
    let shell = crate::tools::shell::selected_host_shell();
    let host_guidance = shell.tool_guidance();
    let examples = shell.command_examples();

    format!(
        "Execute a local command and return stdout, stderr, and exit_code, or process_id when it continues running. Use it to inspect files, search repositories, run builds/tests, and manage local processes; prefer `rg` for text search and `rg --files` for file discovery. WeiAgent includes a private ripgrep binary in released builds and adds it to the command PATH; if `rg` is unavailable, use the selected shell's built-in file search or grep fallback. Prefer multi_file_operation for source/config edits (including outside-root paths via sandbox_permissions=require_escalated + justification) instead of rewriting files through shell. Selected shell: {}. {host_guidance} {examples} {} The server enforces its current execution policy; request elevated access with sandbox_permissions=require_escalated and a justification when needed. Run ordinary commands in the foreground; use background only for long-lived services and tty only for interactive programs. Continue a returned process_id with terminal_session instead of starting the command again. Omit cwd to use the WeiAgent default workspace; relative paths resolve from it. The server checks requested cwd and target paths against its configured permission roots and requests approval when required.",
        shell.display_name(),
        crate::tools::prompt::TERMINAL_FILE_DISCOVERY_POLICY
    )
}

pub(super) fn frozen_terminal_config(
    context: &ToolExecutionContext,
) -> crate::domain::TerminalToolConfig {
    context.tools_config.terminal.clone()
}

pub(super) async fn wait_for_approval_decision(
    context: &ToolExecutionContext,
    approval_id: &str,
    mut decision_rx: tokio::sync::oneshot::Receiver<ApprovalDecision>,
    event_sender: &mpsc::Sender<ToolExecutionEvent>,
) -> Option<ApprovalDecision> {
    tokio::select! {
        decision = &mut decision_rx => decision.ok(),
        _ = event_sender.closed() => {
            context.approval_manager.cancel(approval_id);
            None
        }
    }
}

/// Shared interactive-approval flow: register the pending approval, notify the
/// SSE client, wait for a decision (auto-approving if the agent is switched to
/// full access), persist an always-allow grant when requested, then re-execute
/// with `approved = true` under a freshly reloaded terminal config.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_with_approval(
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

    let decision =
        wait_for_approval_decision(context, &approval_id, decision_rx, &event_sender).await;
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

    let terminal_config = frozen_terminal_config(context);
    re_execute(terminal_config).await
}

pub(super) struct ApprovalRegistrationGuard {
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
    use super::terminal::MAX_TERMINAL_COMMAND_BYTES;
    use super::{ToolRegistry, normalize_optional_cwd, unsupported_host_shell_syntax};
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
        assert!(
            terminal
                .function
                .description
                .contains("File Discovery and Reading Policy")
        );
        assert!(terminal.function.description.contains("node_modules"));
        assert!(terminal.function.description.contains("target"));
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
    fn terminal_tool_schemas_do_not_expose_configured_working_directories() {
        let mut config = AgentToolsConfig::default();
        config.terminal.enabled = true;
        config.terminal.allowed_cwd_roots = vec![
            "D:/Projects/alpha".to_string(),
            "D:/Clients/beta".to_string(),
        ];
        let registry = ToolRegistry::with_builtin_tools();
        let default_definitions = registry.definitions(&AgentToolsConfig::default()).unwrap();
        let configured_definitions = registry.definitions(&config).unwrap();

        for tool_name in ["execute_terminal", "multi_file_operation"] {
            let default_description = &default_definitions
                .iter()
                .find(|definition| definition.function.name == tool_name)
                .unwrap()
                .function
                .description;
            let configured_description = &configured_definitions
                .iter()
                .find(|definition| definition.function.name == tool_name)
                .unwrap()
                .function
                .description;
            assert_eq!(default_description, configured_description);
            assert!(!configured_description.contains("D:/Projects/alpha"));
            assert!(!configured_description.contains("D:/Clients/beta"));
            assert!(!configured_description.contains("known candidate locations"));
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
        assert!(names.contains(&"multi_file_operation"));
        assert!(names.contains(&"task_planning"));
        assert_eq!(names.len(), 8);
        assert!(names.contains(&"read_images"));

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
                "multi_file_operation",
                "read_images",
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
            .find(|definition| definition.function.name == "multi_file_operation")
            .expect("multi_file_operation should be registered");
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
            supports_multimodal: false,
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
        assert!(reason.contains("没有 active 任务计划"));
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
        let result = registry
            .execute("web_search_sources", json!({}), context(conversation_id))
            .await;
        let ToolResult::Blocked {
            reason,
            code,
            details,
        } = result
        else {
            panic!("expected an active plan to require its planning id");
        };
        assert!(reason.contains(plan.public_id()));
        assert_eq!(code.as_deref(), Some("PlanningRequired"));
        assert_eq!(details.unwrap()["active_planning_id"], plan.public_id());

        let other_conversation = Uuid::new_v4();
        let result = registry
            .execute(
                "web_search_sources",
                json!({"planning_id": plan.public_id()}),
                context(other_conversation),
            )
            .await;
        let ToolResult::Blocked { code, details, .. } = result else {
            panic!("expected cross-conversation planning id to be blocked");
        };
        assert_eq!(code.as_deref(), Some("PlanningIdInvalid"));
        assert_eq!(details.unwrap()["provided_planning_id"], plan.public_id());

        let mut disabled_context = context(conversation_id);
        disabled_context.tools_config.task_planning.enabled = false;
        let result = registry
            .execute("web_search_sources", json!({}), disabled_context)
            .await;
        assert!(matches!(result, ToolResult::Completed { .. }));
    }
}
