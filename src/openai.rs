use std::{pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    models::{Agent, ChatMessage, ModelProvider},
};

pub type ChatStream = Pin<Box<dyn Stream<Item = AppResult<String>> + Send>>;

#[async_trait]
pub trait ChatClient: Send + Sync {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
    ) -> AppResult<String>;
    async fn stream(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
    ) -> AppResult<ChatStream>;
}

pub type DynChatClient = Arc<dyn ChatClient>;

#[derive(Clone)]
pub struct OpenAiChatClient {
    http: reqwest::Client,
}

impl OpenAiChatClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    fn api_key<'a>(&self, provider: &'a ModelProvider) -> AppResult<&'a str> {
        let api_key = provider.api_key.trim();
        if api_key.is_empty() {
            return Err(AppError::MissingApiKey);
        }
        Ok(api_key)
    }

    fn chat_url(&self, provider: &ModelProvider) -> String {
        format!(
            "{}/chat/completions",
            provider.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait]
impl ChatClient for OpenAiChatClient {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
    ) -> AppResult<String> {
        let request = build_chat_request(agent, messages, false);
        let response = self
            .http
            .post(self.chat_url(provider))
            .bearer_auth(self.api_key(provider)?)
            .json(&request)
            .send()
            .await
            .map_err(|error| AppError::Llm(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| AppError::Llm(error.to_string()))?;
        if !status.is_success() {
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let response: ChatCompletionResponse =
            serde_json::from_str(&body).map_err(|error| AppError::Llm(error.to_string()))?;

        response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .filter(|content| !content.is_empty())
            .ok_or_else(|| AppError::Llm("empty model response".to_string()))
    }

    async fn stream(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
    ) -> AppResult<ChatStream> {
        let request = build_chat_request(agent, messages, true);
        let response = self
            .http
            .post(self.chat_url(provider))
            .bearer_auth(self.api_key(provider)?)
            .json(&request)
            .send()
            .await
            .map_err(|error| AppError::Llm(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read error body".to_string());
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let mut bytes = response.bytes_stream();
        let stream = async_stream::stream! {
            let mut buffer = String::new();
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(index) = buffer.find('\n') {
                            let line = buffer[..index].trim_end_matches('\r').to_string();
                            buffer = buffer[index + 1..].to_string();
                            for item in parse_sse_line(&line) {
                                yield item;
                            }
                        }
                    }
                    Err(error) => {
                        yield Err(AppError::Llm(error.to_string()));
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

impl OpenAiChatClient {
    pub async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>> {
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let response = self
            .http
            .get(&url)
            .bearer_auth(self.api_key(provider)?)
            .send()
            .await
            .map_err(|error| AppError::Llm(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| AppError::Llm(error.to_string()))?;
        if !status.is_success() {
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let parsed: ModelsListResponse =
            serde_json::from_str(&body).map_err(|error| AppError::Llm(error.to_string()))?;

        let mut models: Vec<String> = parsed
            .data
            .into_iter()
            .map(|m| m.id)
            .filter(|id| !id.is_empty())
            .collect();
        models.sort();
        models.dedup();
        Ok(models)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
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
) -> ChatCompletionRequest {
    let mut request_messages = Vec::with_capacity(messages.len() + 1);
    request_messages.push(OpenAiMessage {
        role: "system".to_string(),
        content: agent.system_prompt.clone(),
    });
    request_messages.extend(messages.iter().map(|message| OpenAiMessage {
        role: message.role.as_openai_role().to_string(),
        content: message.content.clone(),
    }));

    ChatCompletionRequest {
        model: agent.model_id.clone(),
        messages: request_messages,
        stream,
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
}

#[cfg(test)]
fn parse_sse_chunk(chunk: &str) -> Vec<AppResult<String>> {
    chunk.lines().flat_map(parse_sse_line).collect()
}

fn parse_sse_line(line: &str) -> Vec<AppResult<String>> {
    let Some(data) = line.strip_prefix("data: ") else {
        return Vec::new();
    };
    if data == "[DONE]" {
        return Vec::new();
    }

    let parsed = serde_json::from_str::<StreamChunk>(data)
        .map(|chunk| {
            chunk
                .choices
                .into_iter()
                .filter_map(|choice| choice.delta.content)
                .collect::<String>()
        })
        .map_err(AppError::from);

    match parsed {
        Ok(content) if content.is_empty() => Vec::new(),
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::models::{ChatMessage, ChatRole};

    #[test]
    fn builds_openai_chat_completion_request() {
        let now = Utc::now();
        let provider_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            system_prompt: "You write Rust.".to_string(),
            model_provider_id: provider_id,
            model_id: "gpt-4o-mini".to_string(),
            created_at: now,
            updated_at: now,
        };
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id,
            role: ChatRole::User,
            content: "hello".to_string(),
            created_at: now,
        }];

        let request = build_chat_request(&agent, &messages, true);

        assert_eq!(request.model, "gpt-4o-mini");
        assert!(request.stream);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[0].content, "You write Rust.");
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.messages[1].content, "hello");
    }

    #[test]
    fn parses_openai_sse_chunks() {
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\
             data: [DONE]\n",
        );

        let values = parsed.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(values, vec!["hel", "lo"]);
    }
}
