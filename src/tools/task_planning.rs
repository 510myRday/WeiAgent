use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    domain::{AgentToolsConfig, TaskPlan, TaskPlanItem, TaskPlanItemStatus, ToolResult},
    tools::registry::{AgentTool, ToolExecutionContext},
};

pub const TASK_PLANNING_TOOL_NAME: &str = "task_planning";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskPlanningAction {
    Create,
    SetStatus,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TaskPlanItemSelector {
    Single(usize),
    Multiple(Vec<usize>),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct TaskPlanningArgs {
    action: TaskPlanningAction,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    plan: Vec<TaskPlanItem>,
    #[serde(default, alias = "planningId")]
    planning_id: Option<String>,
    #[serde(default)]
    item: Option<TaskPlanItemSelector>,
    #[serde(default)]
    status: Option<TaskPlanItemStatus>,
}

pub struct TaskPlanningTool;

#[async_trait]
impl AgentTool for TaskPlanningTool {
    fn definition(&self, _config: &AgentToolsConfig) -> crate::llm::types::ToolDefinition {
        super::registry::tool_definition(
            TASK_PLANNING_TOOL_NAME,
            "Create and maintain the active task plan for the current conversation. Before using any other tool, create a plan when the task has multiple steps or meaningful dependencies. When creating a plan, include a clear explanation of the user's requirements, objective, completion criteria, and key constraints so it can be reused in task summaries. Keep the plan updated as work progresses.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["create", "set_status"],
                        "description": "Plan operation: create a new plan or update the status of one or more existing tasks."
                    },
                    "explanation": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 4000,
                        "description": "Required when action is create. Clearly describe the user's requirements, task objective, expected completion criteria, and key constraints. Write a specific, high-quality explanation because it is retained and reused in later task summaries."
                    },
                    "plan": {
                        "type": "array",
                        "description": "The complete ordered task list. Required for create.",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["step", "status"],
                            "properties": {
                                "step": {
                                    "type": "string",
                                    "minLength": 1,
                                    "maxLength": 500,
                                    "description": "A concise actionable task description."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Task state. At most one task may be in_progress."
                                }
                            }
                        }
                    },
                    "planning_id": {
                        "type": "string",
                        "description": "The active plan ID returned by create. Required for set_status."
                    },
                    "item": {
                        "oneOf": [
                            {
                                "type": "integer",
                                "minimum": 1,
                                "description": "A 1-based task index as a JSON number, for example 1. Do not quote it as a string."
                            },
                            {
                                "type": "array",
                                "minItems": 1,
                                "uniqueItems": true,
                                "items": {
                                    "type": "integer",
                                    "minimum": 1
                                },
                                "description": "A non-empty array of 1-based task indexes as JSON numbers, for example [1, 2]. Do not quote the indexes as strings."
                            },
                            {
                                "type": "string",
                                "description": "Compatibility form: a decimal task index such as \"1\", or a JSON array string such as \"[1, 2]\". Prefer a JSON number or array."
                            }
                        ],
                        "description": "A 1-based task index or an array of indexes. Arrays let set_status update multiple tasks in one call. Prefer a JSON integer or array; numeric strings and JSON-encoded arrays are accepted for compatibility."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "New status for the selected task."
                    }
                },
                "required": ["action"],
                "oneOf": [
                    {
                        "properties": {
                            "action": {"enum": ["create"]}
                        },
                        "required": ["action", "explanation", "plan"]
                    },
                    {
                        "properties": {
                            "action": {"enum": ["set_status"]}
                        },
                        "required": ["action", "planning_id", "item", "status"]
                    }
                ],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.task_planning.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args = match parse_task_planning_args(arguments) {
            Ok(args) => args,
            Err(message) => return ToolResult::Error { message },
        };
        match args.action {
            TaskPlanningAction::Create => {
                let Some(explanation) = args
                    .explanation
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    return ToolResult::Error {
                        message: "task_planning create requires a clear, non-empty explanation describing the user's requirements, objective, completion criteria, and key constraints".to_string(),
                    };
                };
                if let Err(message) = validate_plan(&args.plan) {
                    return ToolResult::Error { message };
                }
                if let Err(message) = validate_initial_plan(&args.plan) {
                    return ToolResult::Error { message };
                }
                match context
                    .store
                    .create_task_plan(
                        context.conversation_id,
                        Some(explanation.to_string()),
                        args.plan,
                    )
                    .await
                {
                    Ok(plan) if plan_all_completed(&plan.plan) => match context
                        .store
                        .deactivate_task_plan(context.conversation_id, Some(plan.planning_id))
                        .await
                    {
                        Ok(_) => completed_plan_result("Plan completed", &plan),
                        Err(error) => ToolResult::Error {
                            message: error.to_string(),
                        },
                    },
                    Ok(plan) => plan_result("Plan created", &plan),
                    Err(error) => ToolResult::Error {
                        message: error.to_string(),
                    },
                }
            }
            TaskPlanningAction::SetStatus => {
                let provided_planning_id = args
                    .planning_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let planning_id = parse_planning_id(provided_planning_id);
                let active_plan = match context.store.read_task_plan(context.conversation_id).await
                {
                    Ok(active_plan) => active_plan,
                    Err(error) => {
                        return ToolResult::Error {
                            message: error.to_string(),
                        };
                    }
                };
                if planning_id != active_plan.as_ref().map(|plan| plan.planning_id) {
                    let has_provided_id = provided_planning_id.is_some();
                    let (code, reason) = match active_plan.as_ref() {
                        Some(active_plan) if has_provided_id => (
                            "PlanningIdInvalid",
                            format!(
                                "本次 set_status 的 planning_id 无效，或不属于当前会话。请使用当前 active 计划的 planning_id 重试：{}",
                                active_plan.planning_id
                            ),
                        ),
                        Some(active_plan) => (
                            "PlanningRequired",
                            format!(
                                "本次 set_status 缺少 planning_id。请使用当前 active 计划的 planning_id 重试：{}",
                                active_plan.planning_id
                            ),
                        ),
                        None if has_provided_id => (
                            "PlanningIdInvalid",
                            "本次 set_status 提供的 planning_id 无效，当前会话也没有 active 任务计划。请先调用 task_planning 创建计划，再使用新返回的 planning_id 重试。"
                                .to_string(),
                        ),
                        None => (
                            "PlanningRequired",
                            "当前会话没有 active 任务计划，请先调用 task_planning 创建计划。"
                                .to_string(),
                        ),
                    };
                    return ToolResult::Blocked {
                        reason,
                        code: Some(code.to_string()),
                        details: Some(serde_json::json!({
                            "tool": TASK_PLANNING_TOOL_NAME,
                            "provided_planning_id": provided_planning_id,
                            "active_planning_id": active_plan.as_ref().map(|plan| plan.planning_id),
                            "required_argument": "planning_id"
                        })),
                    };
                }
                let planning_id = planning_id.expect("active plan identity was checked above");
                let Some(status) = args.status else {
                    return ToolResult::Error {
                        message: "task_planning set_status requires status".to_string(),
                    };
                };
                let items = match normalize_item_indexes(args.item.as_ref(), "set_status") {
                    Ok(items) => items,
                    Err(message) => return ToolResult::Error { message },
                };
                let Some(mut plan) = active_plan else {
                    return ToolResult::Blocked {
                        reason: "当前 active 任务计划已经不存在，请先调用 task_planning 创建计划。"
                            .to_string(),
                        code: Some("PlanningRequired".to_string()),
                        details: Some(serde_json::json!({
                            "tool": TASK_PLANNING_TOOL_NAME,
                            "provided_planning_id": planning_id,
                            "active_planning_id": null,
                            "required_argument": "planning_id"
                        })),
                    };
                };
                if let Err(message) = validate_item_indexes(&items, plan.plan.len()) {
                    return ToolResult::Error { message };
                }
                let last_item = items.iter().copied().max().unwrap_or_default();
                for item in &items {
                    plan.plan[*item - 1].status = status.clone();
                }
                if status == TaskPlanItemStatus::Completed
                    && !plan
                        .plan
                        .iter()
                        .any(|entry| entry.status == TaskPlanItemStatus::InProgress)
                    && let Some(next) = plan
                        .plan
                        .iter_mut()
                        .skip(last_item)
                        .find(|entry| entry.status == TaskPlanItemStatus::Pending)
                {
                    next.status = TaskPlanItemStatus::InProgress;
                }
                if let Err(message) = validate_plan(&plan.plan) {
                    return ToolResult::Error { message };
                }
                let updated = match context
                    .store
                    .update_task_plan(
                        context.conversation_id,
                        planning_id,
                        plan.explanation.clone(),
                        plan.plan.clone(),
                    )
                    .await
                {
                    Ok(Some(updated)) => plan_result("Plan item updated", &updated),
                    Ok(None) => ToolResult::Blocked {
                        reason: "当前 active 任务计划已发生变化，planning_id 不再有效。请读取当前计划后重试。"
                            .to_string(),
                        code: Some("PlanningIdInvalid".to_string()),
                        details: Some(serde_json::json!({
                            "tool": TASK_PLANNING_TOOL_NAME,
                            "provided_planning_id": planning_id,
                            "active_planning_id": context
                                .store
                                .read_task_plan(context.conversation_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|plan| plan.planning_id),
                            "required_argument": "planning_id"
                        })),
                    },
                    Err(error) => ToolResult::Error {
                        message: error.to_string(),
                    },
                };
                let ToolResult::Completed { .. } = &updated else {
                    return updated;
                };
                if plan_all_completed(&plan.plan) {
                    return match context
                        .store
                        .deactivate_task_plan(context.conversation_id, Some(planning_id))
                        .await
                    {
                        Ok(_) => completed_plan_result("Plan completed", &plan),
                        Err(error) => ToolResult::Error {
                            message: error.to_string(),
                        },
                    };
                }
                updated
            }
        }
    }
}

fn parse_task_planning_args(arguments: Value) -> Result<TaskPlanningArgs, String> {
    let arguments = normalize_task_planning_arguments(arguments);
    match serde_json::from_value::<TaskPlanningArgs>(arguments.clone()) {
        Ok(args) => Ok(args),
        Err(_) => {
            if let Some(item) = arguments.get("item") {
                if item.is_string() {
                    return Err(
                        "task_planning item must be a JSON integer or an array of JSON integers. A string is accepted only when it contains a decimal index such as `\"1\"` or a JSON array such as `\"[1, 2]\"`.".to_string(),
                    );
                }
                if item
                    .as_array()
                    .is_some_and(|items| items.iter().any(Value::is_string))
                {
                    return Err(
                        "task_planning item must be a JSON integer or an array of JSON integers. Array entries may not contain invalid strings; prefer `\"item\": [1, 2]`.".to_string(),
                    );
                }
            }
            Err("Invalid task_planning arguments. For set_status, provide planning_id, item (a 1-based JSON integer or array of integers), and status (pending, in_progress, or completed).".to_string())
        }
    }
}

fn normalize_task_planning_arguments(mut arguments: Value) -> Value {
    let Some(object) = arguments.as_object_mut() else {
        return arguments;
    };
    let Some(item) = object.get_mut("item") else {
        return arguments;
    };
    normalize_task_planning_item(item);
    arguments
}

fn normalize_task_planning_item(item: &mut Value) {
    match item {
        Value::String(value) => {
            if let Ok(index) = value.trim().parse::<usize>() {
                *item = json!(index);
            } else if let Ok(mut parsed) = serde_json::from_str::<Value>(value.trim())
                && parsed.is_array()
            {
                normalize_task_planning_item(&mut parsed);
                *item = parsed;
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_task_planning_item(item);
            }
        }
        _ => {}
    }
}

fn parse_planning_id(value: Option<&str>) -> Option<Uuid> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn normalize_item_indexes(
    item: Option<&TaskPlanItemSelector>,
    action: &str,
) -> Result<Vec<usize>, String> {
    let Some(item) = item else {
        return Err(format!("task_planning {action} requires item"));
    };
    let mut items = match item {
        TaskPlanItemSelector::Single(item) => vec![*item],
        TaskPlanItemSelector::Multiple(items) => items.clone(),
    };
    items.sort_unstable();
    items.dedup();
    if items.is_empty() {
        return Err(format!("task_planning {action} requires at least one item"));
    }
    Ok(items)
}

fn validate_item_indexes(items: &[usize], plan_len: usize) -> Result<(), String> {
    if let Some(item) = items.iter().find(|item| **item == 0 || **item > plan_len) {
        return Err(format!(
            "task_planning item must be between 1 and {} (received {})",
            plan_len, item
        ));
    }
    Ok(())
}

fn validate_plan(plan: &[TaskPlanItem]) -> Result<(), String> {
    if plan.is_empty() {
        return Err("task_planning plan requires at least one item".to_string());
    }
    if plan.len() > 50 {
        return Err("task_planning plan cannot contain more than 50 items".to_string());
    }
    let in_progress = plan
        .iter()
        .filter(|item| item.status == TaskPlanItemStatus::InProgress)
        .count();
    if in_progress > 1 {
        return Err("task_planning plan can have at most one in_progress item".to_string());
    }
    for (index, item) in plan.iter().enumerate() {
        if item.step.trim().is_empty() {
            return Err(format!(
                "task_planning item {} has an empty step",
                index + 1
            ));
        }
        if item.step.chars().count() > 500 {
            return Err(format!(
                "task_planning item {} exceeds 500 characters",
                index + 1
            ));
        }
    }
    Ok(())
}

fn validate_initial_plan(plan: &[TaskPlanItem]) -> Result<(), String> {
    if plan
        .first()
        .is_none_or(|item| item.status != TaskPlanItemStatus::InProgress)
    {
        return Err(
            "task_planning create requires the first item to have status in_progress".to_string(),
        );
    }
    if plan
        .iter()
        .skip(1)
        .any(|item| item.status != TaskPlanItemStatus::Pending)
    {
        return Err(
            "task_planning create requires all items after the first to have status pending"
                .to_string(),
        );
    }
    Ok(())
}

fn plan_all_completed(plan: &[TaskPlanItem]) -> bool {
    !plan.is_empty()
        && plan
            .iter()
            .all(|item| item.status == TaskPlanItemStatus::Completed)
}

fn plan_result(message: &str, plan: &TaskPlan) -> ToolResult {
    plan_json_result(message, plan, true)
}

fn completed_plan_result(message: &str, plan: &TaskPlan) -> ToolResult {
    plan_json_result(message, plan, false)
}

fn plan_json_result(message: &str, plan: &TaskPlan, active: bool) -> ToolResult {
    ToolResult::Completed {
        stdout: json!({
            "status": "completed",
            "tool": TASK_PLANNING_TOOL_NAME,
            "message": message,
            "active": active,
            "planning_id": plan.planning_id,
            "explanation": plan.explanation,
            "plan": plan.plan,
            "created_at": plan.created_at,
            "updated_at": plan.updated_at,
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
        duration_ms: 0,
        runtime: Some("builtin_task_planning".to_string()),
        output_cursor: 0,
        structured_content: None,
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::{
        storage::Store,
        tools::{approval::ApprovalManager, registry::AgentTool, terminal::TerminalManager},
    };

    fn context(store: Arc<Store>, conversation_id: Uuid) -> ToolExecutionContext {
        ToolExecutionContext {
            agent_id: Uuid::new_v4(),
            conversation_id,
            call_id: "task-planning-test".to_string(),
            tools_config: AgentToolsConfig::default(),
            store,
            terminal_manager: TerminalManager::new_for_tests(),
            approval_manager: Arc::new(ApprovalManager::new()),
            event_sender: None,
            web_search_provenance: Default::default(),
        }
    }

    #[test]
    fn task_planning_definition_requires_create_explanation_only() {
        let definition = TaskPlanningTool.definition(&AgentToolsConfig::default());
        let required = definition.function.parameters["required"]
            .as_array()
            .unwrap();
        assert!(required.iter().any(|value| value == "action"));
        assert!(
            definition.function.parameters["properties"]["explanation"]["description"]
                .as_str()
                .unwrap()
                .contains("action is create")
        );
        assert!(
            definition.function.parameters["properties"]["item"]["description"]
                .as_str()
                .unwrap()
                .contains("accepted for compatibility")
        );
        assert!(
            definition.function.parameters["properties"]["item"]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .any(|variant| variant["type"] == "string")
        );
        let branches = definition.function.parameters["oneOf"].as_array().unwrap();
        assert!(branches.iter().any(|branch| {
            branch["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|value| value == "explanation"))
        }));
        assert!(branches.iter().any(|branch| {
            branch["required"]
                .as_array()
                .is_some_and(|required| !required.iter().any(|value| value == "explanation"))
        }));
        assert!(branches.iter().any(|branch| {
            branch["required"].as_array().is_some_and(|required| {
                ["planning_id", "item", "status"]
                    .iter()
                    .all(|name| required.iter().any(|value| value == name))
            })
        }));
    }

    #[tokio::test]
    async fn task_planning_accepts_a_numeric_string_item_index() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;
        let created = tool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "explanation": "Test numeric string compatibility",
                    "plan": [{"step": "First", "status": "in_progress"}]
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = created else {
            panic!("expected plan creation to succeed");
        };
        let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let result = tool
            .execute(
                serde_json::json!({
                    "action": "set_status",
                    "planning_id": created["planning_id"],
                    "item": "1",
                    "status": "completed"
                }),
                context(store, conversation_id),
            )
            .await;

        let ToolResult::Completed { stdout, .. } = result else {
            panic!("expected a numeric string item index to be normalized");
        };
        let result: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(result["plan"][0]["status"], "completed");
    }

    #[tokio::test]
    async fn task_planning_accepts_a_stringified_item_array() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;
        let created = tool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "explanation": "Test stringified item array compatibility",
                    "plan": [
                        {"step": "First", "status": "in_progress"},
                        {"step": "Second", "status": "pending"}
                    ]
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = created else {
            panic!("expected plan creation to succeed");
        };
        let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let result = tool
            .execute(
                serde_json::json!({
                    "action": "set_status",
                    "planning_id": created["planning_id"],
                    "item": "[1, 2]",
                    "status": "completed"
                }),
                context(store, conversation_id),
            )
            .await;

        let ToolResult::Completed { stdout, .. } = result else {
            panic!("expected a stringified item array to be normalized");
        };
        let result: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(result["plan"][0]["status"], "completed");
        assert_eq!(result["plan"][1]["status"], "completed");
    }

    #[tokio::test]
    async fn task_planning_explains_that_non_numeric_item_is_invalid() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let created = TaskPlanningTool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "explanation": "Validate task item parsing",
                    "plan": [{"step": "First", "status": "in_progress"}]
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = created else {
            panic!("expected plan creation to succeed");
        };
        let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let result = TaskPlanningTool
            .execute(
                serde_json::json!({
                    "action": "set_status",
                    "planning_id": created["planning_id"],
                    "item": "one",
                    "status": "completed"
                }),
                context(store, conversation_id),
            )
            .await;

        let ToolResult::Error { message } = result else {
            panic!("expected a non-numeric item index to be rejected");
        };
        assert!(message.contains("JSON integer"));
        assert!(message.contains("accepted only when it contains"));
        assert!(!message.contains("untagged enum"));
    }

    #[tokio::test]
    async fn task_planning_rejects_missing_or_blank_explanation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;

        for explanation in [None, Some("   ")] {
            let mut arguments = serde_json::json!({
                "action": "create",
                "plan": [{"step": "First", "status": "in_progress"}]
            });
            if let Some(explanation) = explanation {
                arguments["explanation"] = serde_json::Value::String(explanation.to_string());
            }
            let result = tool
                .execute(arguments, context(store.clone(), conversation_id))
                .await;
            let ToolResult::Error { message } = result else {
                panic!("expected missing or blank explanation to be rejected");
            };
            assert!(message.contains("explanation"));
        }
    }

    #[tokio::test]
    async fn task_planning_create_requires_a_current_item() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();

        let result = TaskPlanningTool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "explanation": "Test plan",
                    "plan": [
                        {"step": "First", "status": "pending"},
                        {"step": "Second", "status": "pending"}
                    ]
                }),
                context(store, conversation_id),
            )
            .await;

        let ToolResult::Error { message } = result else {
            panic!("expected a plan without an in_progress item to be rejected");
        };
        assert!(message.contains("first item"));
        assert!(message.contains("in_progress"));
    }

    #[tokio::test]
    async fn task_planning_advances_items_and_retains_completed_plan() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;

        let created = tool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "explanation": "Test plan",
                    "plan": [
                        {"step": "First", "status": "in_progress"},
                        {"step": "Second", "status": "pending"}
                    ]
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = created else {
            panic!("expected plan creation to succeed");
        };
        let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let planning_id = created["planning_id"].as_str().unwrap().to_string();

        let updated = tool
            .execute(
                serde_json::json!({
                    "action": "set_status",
                    "planning_id": planning_id,
                    "item": 1,
                    "status": "completed"
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = updated else {
            panic!("expected item update to succeed");
        };
        let updated: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(updated["plan"][1]["status"], "in_progress");

        let completed = tool
            .execute(
                serde_json::json!({
                    "action": "set_status",
                    "planning_id": planning_id,
                    "item": 2,
                    "status": "completed"
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = completed else {
            panic!("expected plan completion to succeed");
        };
        let completed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(completed["active"], false);
        assert_eq!(store.read_task_plan(conversation_id).await.unwrap(), None);
        let retained: i64 = store
            .database()
            .call({
                let planning_id = Uuid::parse_str(planning_id.as_str()).unwrap();
                move |conn| {
                    Ok(conn.query_row(
                        "SELECT COUNT(*) FROM task_plans WHERE planning_id = ?1",
                        [planning_id.to_string()],
                        |row| row.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(retained, 1);
    }

    #[tokio::test]
    async fn set_status_accepts_multiple_items_in_one_call() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;

        let created = tool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "explanation": "Complete the planned implementation and validation",
                    "plan": [
                        {"step": "First", "status": "in_progress"},
                        {"step": "Second", "status": "pending"},
                        {"step": "Third", "status": "pending"}
                    ]
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = created else {
            panic!("expected plan creation to succeed");
        };
        let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let planning_id = created["planning_id"].as_str().unwrap();

        let updated = tool
            .execute(
                serde_json::json!({
                    "action": "set_status",
                    "planning_id": planning_id,
                    "item": [1, 2],
                    "status": "completed"
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = updated else {
            panic!("expected multiple item update to succeed");
        };
        let updated: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(updated["plan"][0]["status"], "completed");
        assert_eq!(updated["plan"][1]["status"], "completed");
        assert_eq!(updated["plan"][2]["status"], "in_progress");
    }
}
