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
    Replace,
    SetStatus,
    RemoveItem,
    Clear,
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
            "Create and maintain the active task plan for the current conversation. Before using any other tool, create a plan and use the returned planning_id for that tool call. Keep the plan updated as work progresses.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["create", "replace", "set_status", "remove_item", "clear"],
                        "description": "Plan operation: create a new plan, replace the full plan, update one item status, remove one item, or clear the active plan."
                    },
                    "explanation": {
                        "type": "string",
                        "description": "Optional explanation of the plan. Use with create or replace."
                    },
                    "plan": {
                        "type": "array",
                        "description": "The complete ordered task list. Required for create and replace.",
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
                        "description": "The active plan ID returned by create. Required for replace, set_status, remove_item, and clear."
                    },
                    "item": {
                        "oneOf": [
                            {
                                "type": "integer",
                                "minimum": 1
                            },
                            {
                                "type": "array",
                                "minItems": 1,
                                "uniqueItems": true,
                                "items": {
                                    "type": "integer",
                                    "minimum": 1
                                }
                            }
                        ],
                        "description": "A 1-based task index or an array of indexes. Arrays let set_status update multiple tasks in one call. Required for set_status and remove_item."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "New status for the selected task."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.task_planning.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args = match serde_json::from_value::<TaskPlanningArgs>(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid task_planning arguments: {error}"),
                };
            }
        };

        match args.action {
            TaskPlanningAction::Create => {
                if let Err(message) = validate_plan(&args.plan) {
                    return ToolResult::Error { message };
                }
                match context
                    .store
                    .create_task_plan(context.conversation_id, args.explanation, args.plan)
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
            TaskPlanningAction::Replace => {
                let Some(planning_id) = parse_planning_id(args.planning_id.as_deref()) else {
                    return ToolResult::Error {
                        message: "task_planning replace requires a valid planning_id".to_string(),
                    };
                };
                if let Err(message) = validate_plan(&args.plan) {
                    return ToolResult::Error { message };
                }
                match context
                    .store
                    .update_task_plan(
                        context.conversation_id,
                        planning_id,
                        args.explanation,
                        args.plan,
                    )
                    .await
                {
                    Ok(Some(plan)) if plan_all_completed(&plan.plan) => match context
                        .store
                        .deactivate_task_plan(context.conversation_id, Some(plan.planning_id))
                        .await
                    {
                        Ok(_) => completed_plan_result("Plan completed", &plan),
                        Err(error) => ToolResult::Error {
                            message: error.to_string(),
                        },
                    },
                    Ok(Some(plan)) => plan_result("Plan replaced", &plan),
                    Ok(None) => ToolResult::Error {
                        message: "task_planning planning_id is not active in this conversation"
                            .to_string(),
                    },
                    Err(error) => ToolResult::Error {
                        message: error.to_string(),
                    },
                }
            }
            TaskPlanningAction::SetStatus => {
                let Some(planning_id) = parse_planning_id(args.planning_id.as_deref()) else {
                    return ToolResult::Error {
                        message: "task_planning set_status requires a valid planning_id"
                            .to_string(),
                    };
                };
                let Some(status) = args.status else {
                    return ToolResult::Error {
                        message: "task_planning set_status requires status".to_string(),
                    };
                };
                let items = match normalize_item_indexes(args.item.as_ref(), "set_status") {
                    Ok(items) => items,
                    Err(message) => return ToolResult::Error { message },
                };
                let Some(mut plan) = read_matching_plan(&context, planning_id).await else {
                    return ToolResult::Error {
                        message: "task_planning planning_id is not active in this conversation"
                            .to_string(),
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
                {
                    if let Some(next) = plan
                        .plan
                        .iter_mut()
                        .skip(last_item)
                        .find(|entry| entry.status == TaskPlanItemStatus::Pending)
                    {
                        next.status = TaskPlanItemStatus::InProgress;
                    }
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
                    Ok(None) => ToolResult::Error {
                        message: "task_planning planning_id is no longer active".to_string(),
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
            TaskPlanningAction::RemoveItem => {
                let Some(planning_id) = parse_planning_id(args.planning_id.as_deref()) else {
                    return ToolResult::Error {
                        message: "task_planning remove_item requires a valid planning_id"
                            .to_string(),
                    };
                };
                let items = match normalize_item_indexes(args.item.as_ref(), "remove_item") {
                    Ok(items) => items,
                    Err(message) => return ToolResult::Error { message },
                };
                let Some(mut plan) = read_matching_plan(&context, planning_id).await else {
                    return ToolResult::Error {
                        message: "task_planning planning_id is not active in this conversation"
                            .to_string(),
                    };
                };
                if let Err(message) = validate_item_indexes(&items, plan.plan.len()) {
                    return ToolResult::Error { message };
                }
                if items.len() >= plan.plan.len() {
                    return ToolResult::Error {
                        message: "task_planning remove_item must leave at least one item; use clear to remove the plan"
                            .to_string(),
                    };
                }
                for item in items.into_iter().rev() {
                    plan.plan.remove(item - 1);
                }
                match context
                    .store
                    .update_task_plan(
                        context.conversation_id,
                        planning_id,
                        plan.explanation.clone(),
                        plan.plan.clone(),
                    )
                    .await
                {
                    Ok(Some(updated)) => plan_result("Plan item removed", &updated),
                    Ok(None) => ToolResult::Error {
                        message: "task_planning planning_id is no longer active".to_string(),
                    },
                    Err(error) => ToolResult::Error {
                        message: error.to_string(),
                    },
                }
            }
            TaskPlanningAction::Clear => {
                let planning_id = match args.planning_id.as_deref() {
                    Some(value) => match parse_planning_id(Some(value)) {
                        Some(planning_id) => Some(planning_id),
                        None => {
                            return ToolResult::Error {
                                message: "task_planning clear requires a valid planning_id"
                                    .to_string(),
                            };
                        }
                    },
                    None => None,
                };
                let active_plan = context
                    .store
                    .read_task_plan(context.conversation_id)
                    .await
                    .ok()
                    .flatten();
                if let Some(planning_id) = planning_id {
                    if active_plan
                        .as_ref()
                        .is_none_or(|plan| plan.planning_id != planning_id)
                    {
                        return ToolResult::Error {
                            message: "task_planning planning_id is not active in this conversation"
                                .to_string(),
                        };
                    }
                }
                let cleared_planning_id =
                    planning_id.or_else(|| active_plan.as_ref().map(|plan| plan.planning_id));
                match context
                    .store
                    .deactivate_task_plan(context.conversation_id, planning_id)
                    .await
                {
                    Ok(removed) => ToolResult::Completed {
                        stdout: json!({
                            "status": "completed",
                            "tool": TASK_PLANNING_TOOL_NAME,
                            "active": false,
                            "cleared": removed,
                            "retained": removed,
                            "planning_id": cleared_planning_id,
                        })
                        .to_string(),
                        stderr: String::new(),
                        exit_code: 0,
                        duration_ms: 0,
                        runtime: Some("builtin_task_planning".to_string()),
                        output_cursor: 0,
                    },
                    Err(error) => ToolResult::Error {
                        message: error.to_string(),
                    },
                }
            }
        }
    }
}

async fn read_matching_plan(context: &ToolExecutionContext, planning_id: Uuid) -> Option<TaskPlan> {
    context
        .store
        .read_task_plan(context.conversation_id)
        .await
        .ok()
        .flatten()
        .filter(|plan| plan.planning_id == planning_id)
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

    #[tokio::test]
    async fn clear_rejects_a_stale_or_malformed_planning_id() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;

        let result = tool
            .execute(
                serde_json::json!({"action": "clear", "planning_id": "not-a-uuid"}),
                context(store.clone(), conversation_id),
            )
            .await;
        assert!(matches!(result, ToolResult::Error { .. }));

        let plan = store
            .create_task_plan(
                conversation_id,
                None,
                vec![TaskPlanItem {
                    step: "Keep me".to_string(),
                    status: TaskPlanItemStatus::Pending,
                }],
            )
            .await
            .unwrap();
        let result = tool
            .execute(
                serde_json::json!({
                    "action": "clear",
                    "planning_id": Uuid::new_v4()
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        assert!(matches!(result, ToolResult::Error { .. }));
        assert_eq!(
            store
                .read_task_plan(conversation_id)
                .await
                .unwrap()
                .unwrap()
                .planning_id,
            plan.planning_id
        );
    }

    #[tokio::test]
    async fn clear_without_id_returns_the_retained_plan_id() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let conversation_id = Uuid::new_v4();
        let tool = TaskPlanningTool;

        let created = tool
            .execute(
                serde_json::json!({
                    "action": "create",
                    "plan": [{"step": "Keep history", "status": "pending"}]
                }),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = created else {
            panic!("expected plan creation to succeed");
        };
        let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let planning_id = created["planning_id"].as_str().unwrap().to_string();

        let cleared = tool
            .execute(
                serde_json::json!({"action": "clear"}),
                context(store.clone(), conversation_id),
            )
            .await;
        let ToolResult::Completed { stdout, .. } = cleared else {
            panic!("expected plan clear to succeed");
        };
        let cleared: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(cleared["planning_id"], planning_id);
        assert_eq!(cleared["retained"], true);
    }
}
