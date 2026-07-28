use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
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
            "Create and maintain the active task plan for the current conversation. When creating a plan, include a clear explanation of the user's requirements, objective, completion criteria, and key constraints so it can be reused in task summaries. Keep the plan updated as work progresses.",
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
                        "description": "A 1-based task index or an array of indexes. Arrays let set_status update multiple tasks in one call. Required for set_status."
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
                        "required": ["action", "explanation"]
                    },
                    {
                        "properties": {
                            "action": {"enum": ["set_status"]}
                        },
                        "required": ["action"]
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
