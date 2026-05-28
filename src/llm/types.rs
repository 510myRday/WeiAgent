use serde::{Deserialize, Serialize};

use crate::domain::{Agent, ChatMessage};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    #[serde(flatten)]
    pub extra_body: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: String,
}

pub fn build_chat_request(
    agent: &Agent,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    extra_body: &serde_json::Value,
) -> ChatCompletionRequest {
    let mut request_messages = Vec::with_capacity(messages.len() + 1);
    request_messages.push(OpenAiMessage {
        role: "system".to_string(),
        content: system_prompt.to_string(),
    });
    request_messages.extend(messages.iter().map(|message| OpenAiMessage {
        role: message.role.as_openai_role().to_string(),
        content: message.content.clone(),
    }));

    ChatCompletionRequest {
        model: agent.model_id.clone(),
        messages: request_messages,
        stream,
        extra_body: extra_body.clone(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChoice {
    pub message: ChatCompletionMessage,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionMessage {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
pub struct StreamDelta {
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ModelsListResponse {
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub id: String,
}
