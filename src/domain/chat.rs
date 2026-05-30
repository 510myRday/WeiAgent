use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub role: ChatRole,
    pub content: String,
    #[serde(default)]
    pub status: ChatMessageStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    /// Present when role = Tool, links to the tool_call_id from assistant's tool_calls
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatMessageStatus {
    #[default]
    Done,
    Streaming,
    Failed,
}

impl ChatRole {
    pub fn as_openai_role(&self) -> &'static str {
        match self {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default = "default_stream_response")]
    pub stream: bool,
}

fn default_stream_response() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub user: ChatMessage,
    pub assistant: ChatMessage,
}
