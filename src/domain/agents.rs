use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    #[serde(default)]
    pub is_builtin: bool,
    #[serde(default = "default_avatar_icon")]
    pub avatar_icon: String,
    #[serde(default = "default_avatar_color")]
    pub avatar_color: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_avatar_icon() -> String {
    "bot".to_string()
}

fn default_avatar_color() -> String {
    "#22c55e".to_string()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentResponse {
    pub id: Uuid,
    pub name: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    pub avatar_icon: String,
    pub avatar_color: String,
    pub is_builtin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentResponse {
    pub fn from_agent(agent: &Agent) -> Self {
        Self {
            id: agent.id,
            name: agent.name.clone(),
            model_provider_id: agent.model_provider_id,
            model_id: agent.model_id.clone(),
            avatar_icon: agent.avatar_icon.clone(),
            avatar_color: agent.avatar_color.clone(),
            is_builtin: agent.is_builtin,
            created_at: agent.created_at,
            updated_at: agent.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    pub avatar_icon: Option<String>,
    pub avatar_color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub name: Option<String>,
    pub model_provider_id: Option<Uuid>,
    pub model_id: Option<String>,
    pub avatar_icon: Option<String>,
    pub avatar_color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkspace {
    pub personality: String,
    pub memory: String,
    pub instructions: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentWorkspaceRequest {
    pub personality: String,
    pub memory: String,
    pub instructions: String,
}
