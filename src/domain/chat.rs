use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageAttachment {
    pub id: String,
    pub name: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_mask: bool,
    #[serde(flatten)]
    pub source: ImageSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "source_type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { data: String },
    Url { url: String },
    Local { path: String },
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub total_prompt_tokens: u64,
    #[serde(default)]
    pub uncached_input_tokens: u64,
    #[serde(default)]
    pub cache_hit_ratio: f64,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    pub fn normalized(mut self) -> Self {
        if self.uncached_input_tokens == 0 {
            self.uncached_input_tokens = self.input_tokens;
        }
        if self.total_prompt_tokens == 0 {
            self.total_prompt_tokens = self
                .uncached_input_tokens
                .saturating_add(self.cache_read_tokens)
                .saturating_add(self.cache_write_tokens);
        }
        if self.cache_hit_ratio == 0.0 {
            let denominator = self
                .uncached_input_tokens
                .saturating_add(self.cache_read_tokens);
            if denominator > 0 {
                self.cache_hit_ratio = self.cache_read_tokens as f64 / denominator as f64;
            }
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelUsage {
    #[serde(flatten)]
    pub tokens: TokenUsage,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_count: Option<u32>,
    pub provider_id: Uuid,
    pub model_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConversationUsage {
    #[serde(flatten)]
    pub tokens: TokenUsage,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub role: ChatRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Opaque Responses API items required for stateless reasoning continuity.
    /// They are persisted locally but never exposed through the chat API.
    #[serde(default, skip_serializing)]
    pub openai_response_items: Vec<serde_json::Value>,
    #[serde(default)]
    pub status: ChatMessageStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    /// Present when role = Tool, links to the tool_call_id from assistant's tool_calls
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ImageAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
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

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenerationMode {
    #[default]
    Chat,
    Image,
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
    #[serde(default)]
    pub attachments: Vec<ImageAttachment>,
    #[serde(default)]
    pub thinking_level: ThinkingLevel,
    #[serde(default = "default_stream_response")]
    pub stream: bool,
    #[serde(default)]
    pub generation_mode: GenerationMode,
    #[serde(default)]
    pub restart_from_message_id: Option<Uuid>,
}

fn default_stream_response() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AgentRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub user_message_id: Option<Uuid>,
    pub status: AgentRunStatus,
    pub phase: Option<String>,
    pub latest_sequence: u64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_response_items_are_internal_only() {
        let message = ChatMessage {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            role: ChatRole::Assistant,
            content: "answer".to_string(),
            reasoning_content: None,
            openai_response_items: vec![serde_json::json!({
                "type": "reasoning",
                "encrypted_content": "opaque-state"
            })],
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        };

        let serialized = serde_json::to_value(message).unwrap();
        assert!(serialized.get("openai_response_items").is_none());
        assert!(!serialized.to_string().contains("opaque-state"));
    }
}
