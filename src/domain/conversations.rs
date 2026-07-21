use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ChatMessage, ConversationUsage};

/// Persistent auto-compact boundary for a conversation.
/// Full UI transcript stays intact; only LLM-facing history is rewritten via this summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextCheckpoint {
    /// Dense task-state summary used as the LLM prefix after compaction.
    pub summary: String,
    /// Inclusive boundary: messages up to and including this id are represented by `summary`.
    pub covers_until_message_id: Uuid,
    pub created_at: DateTime<Utc>,
    /// Monotonic generation for debugging repeated compaction.
    #[serde(default)]
    pub generation: u32,
    /// Estimated tokens of the summarized region when the checkpoint was written.
    #[serde(default)]
    pub source_tokens_estimate: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub title: String,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub is_pinned: bool,
    /// Latest compaction checkpoint. Omitted in legacy conversation files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_checkpoint: Option<ContextCheckpoint>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateConversationRequest {
    pub is_pinned: bool,
}

#[derive(Debug, Serialize)]
pub struct ConversationMessagePage {
    pub messages: Vec<ChatMessage>,
    pub next_cursor: Option<Uuid>,
    pub has_more: bool,
    pub usage: ConversationUsage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_json_defaults_missing_checkpoint() {
        let raw = serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "agent_id": "22222222-2222-2222-2222-222222222222",
            "title": "t",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let conversation: Conversation = serde_json::from_value(raw).unwrap();
        assert!(conversation.context_checkpoint.is_none());
        assert!(!conversation.is_pinned);
        assert_eq!(conversation.message_count, 0);
    }
}
