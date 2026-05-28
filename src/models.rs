use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_avatar_icon")]
    pub avatar_icon: String,
    #[serde(default = "default_avatar_color")]
    pub avatar_color: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            log_level: default_log_level(),
            avatar_icon: default_avatar_icon(),
            avatar_color: default_avatar_color(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_avatar_icon() -> String {
    "bot".to_string()
}

fn default_avatar_color() -> String {
    "#22c55e".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProvider {
    pub id: Uuid,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelProviderResponse {
    pub id: Uuid,
    pub name: String,
    pub base_url: String,
    pub has_api_key: bool,
    pub masked_api_key: String,
    pub models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ModelProvider> for ModelProviderResponse {
    fn from(provider: &ModelProvider) -> Self {
        Self {
            id: provider.id,
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            has_api_key: !provider.api_key.trim().is_empty(),
            masked_api_key: mask_api_key(&provider.api_key),
            models: provider.models.clone(),
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

fn mask_api_key(api_key: &str) -> String {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let suffix = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("****{suffix}")
}

#[derive(Debug, Deserialize)]
pub struct CreateModelProviderRequest {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModelProviderRequest {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    #[serde(default = "default_avatar_icon")]
    pub avatar_icon: String,
    #[serde(default = "default_avatar_color")]
    pub avatar_color: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    pub system_prompt: String,
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
            system_prompt: agent.system_prompt.clone(),
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
    pub system_prompt: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    pub avatar_icon: Option<String>,
    pub avatar_color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub name: Option<String>,
    pub system_prompt: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub role: ChatRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_openai_role(&self) -> &'static str {
        match self {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub user: ChatMessage,
    pub assistant: ChatMessage,
}
