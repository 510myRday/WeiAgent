use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ChatMessage, ConversationUsage};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub title: String,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub is_pinned: bool,
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
