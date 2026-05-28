use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkspaceSettings {
    #[serde(default)]
    pub is_builtin: bool,
    #[serde(default)]
    pub builtin_key: Option<String>,
    #[serde(default)]
    pub display_label: Option<String>,
    #[serde(default)]
    pub model_note: Option<String>,
    #[serde(default)]
    pub can_delete: bool,
}

impl Default for AgentWorkspaceSettings {
    fn default() -> Self {
        Self {
            is_builtin: false,
            builtin_key: None,
            display_label: None,
            model_note: None,
            can_delete: true,
        }
    }
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
    pub builtin_key: Option<String>,
    pub display_label: Option<String>,
    pub model_note: Option<String>,
    pub can_delete: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentResponse {
    pub fn from_agent(agent: &Agent, settings: AgentWorkspaceSettings) -> Self {
        Self {
            id: agent.id,
            name: agent.name.clone(),
            model_provider_id: agent.model_provider_id,
            model_id: agent.model_id.clone(),
            avatar_icon: agent.avatar_icon.clone(),
            avatar_color: agent.avatar_color.clone(),
            is_builtin: settings.is_builtin,
            builtin_key: settings.builtin_key,
            display_label: settings.display_label,
            model_note: settings.model_note,
            can_delete: settings.can_delete,
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
    pub settings: AgentWorkspaceSettings,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentWorkspaceRequest {
    pub personality: String,
    pub memory: String,
    pub instructions: String,
}
