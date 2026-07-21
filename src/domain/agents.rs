use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT: u8 = 90;
pub const MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT: u8 = 1;
pub const MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT: u8 = 99;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    #[serde(default)]
    pub is_builtin: bool,
    #[serde(default = "default_avatar_icon")]
    pub avatar_icon: String,
    #[serde(default = "default_avatar_color")]
    pub avatar_color: String,
    /// Auto-compact when estimated context usage reaches this percent of the model window.
    /// Always enabled. Valid range is 1..=99 (default 90).
    #[serde(default = "default_context_compress_threshold_percent")]
    pub context_compress_threshold_percent: u8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_avatar_icon() -> String {
    "bot".to_string()
}

fn default_avatar_color() -> String {
    "#22c55e".to_string()
}

fn default_context_compress_threshold_percent() -> u8 {
    DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT
}

/// Clamp and validate a user-provided compression threshold.
pub fn normalize_context_compress_threshold(value: u8) -> Result<u8, String> {
    if (MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT..=MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT)
        .contains(&value)
    {
        Ok(value)
    } else {
        Err(format!(
            "context_compress_threshold_percent must be between {MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT} and {MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT}"
        ))
    }
}

/// Coerce legacy/out-of-range stored values into the supported band.
pub fn coerce_context_compress_threshold(value: u8) -> u8 {
    value.clamp(
        MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT,
        MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT,
    )
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    pub avatar_icon: String,
    pub avatar_color: String,
    pub is_builtin: bool,
    pub context_compress_threshold_percent: u8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentResponse {
    pub fn from_agent(agent: &Agent) -> Self {
        Self {
            id: agent.id,
            name: agent.name.clone(),
            description: agent.description.clone(),
            model_provider_id: agent.model_provider_id,
            model_id: agent.model_id.clone(),
            avatar_icon: agent.avatar_icon.clone(),
            avatar_color: agent.avatar_color.clone(),
            is_builtin: agent.is_builtin,
            context_compress_threshold_percent: coerce_context_compress_threshold(
                agent.context_compress_threshold_percent,
            ),
            created_at: agent.created_at,
            updated_at: agent.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: String,
    pub model_provider_id: Uuid,
    pub model_id: String,
    pub avatar_icon: Option<String>,
    pub avatar_color: Option<String>,
    pub context_compress_threshold_percent: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub model_provider_id: Option<Uuid>,
    pub model_id: Option<String>,
    pub avatar_icon: Option<String>,
    pub avatar_color: Option<String>,
    pub context_compress_threshold_percent: Option<u8>,
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

#[derive(Debug, Deserialize)]
pub struct UpdateAgentProfileRequest {
    pub agent: UpdateAgentRequest,
    pub workspace: UpdateAgentWorkspaceRequest,
}

#[derive(Debug, Serialize)]
pub struct AgentProfileResponse {
    pub agent: AgentResponse,
    pub workspace: AgentWorkspace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_defaults_and_rejects_out_of_range() {
        assert_eq!(
            default_context_compress_threshold_percent(),
            DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT
        );
        assert!(normalize_context_compress_threshold(1).is_ok());
        assert!(normalize_context_compress_threshold(99).is_ok());
        assert!(normalize_context_compress_threshold(0).is_err());
        assert!(normalize_context_compress_threshold(100).is_err());
        assert_eq!(coerce_context_compress_threshold(0), 1);
        assert_eq!(coerce_context_compress_threshold(150), 99);
    }

    #[test]
    fn agent_json_defaults_threshold() {
        let raw = serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "name": "A",
            "model_provider_id": "22222222-2222-2222-2222-222222222222",
            "model_id": "m",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let agent: Agent = serde_json::from_value(raw).unwrap();
        assert_eq!(
            agent.context_compress_threshold_percent,
            DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT
        );
    }
}
