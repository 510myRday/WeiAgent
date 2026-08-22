use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub source_conversation_id: Option<Uuid>,
    pub schedule_expr: String,
    pub timezone_offset: String,
    pub prompt: String,
    pub enabled: bool,
    pub next_run_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub active_execution_id: Option<Uuid>,
    pub latest_execution_conversation_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTaskExecution {
    pub id: Uuid,
    pub task_id: Uuid,
    pub conversation_id: Uuid,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub error: Option<String>,
    #[serde(default = "default_execution_trigger")]
    pub trigger_type: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub output_preview: Option<String>,
}

fn default_execution_trigger() -> String {
    "scheduled".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationHeartbeat {
    pub conversation_id: Uuid,
    pub agent_id: Uuid,
    pub interval_seconds: u64,
    pub prompt: String,
    pub enabled: bool,
    pub next_fire_at: DateTime<Utc>,
    pub pending: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationHeartbeatExecution {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub error: Option<String>,
    #[serde(default = "default_execution_trigger")]
    pub trigger_type: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub output_preview: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateScheduledTaskRequest {
    pub agent_id: Uuid,
    pub source_conversation_id: Option<Uuid>,
    pub schedule: String,
    pub prompt: String,
    pub timezone_offset: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateScheduledTaskRequest {
    pub agent_id: Option<Uuid>,
    pub schedule: Option<String>,
    pub prompt: Option<String>,
    pub timezone_offset: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatRequest {
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    pub interval_seconds: u64,
    pub prompt: String,
}
