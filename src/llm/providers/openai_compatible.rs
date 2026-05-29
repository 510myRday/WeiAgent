use async_trait::async_trait;
use futures_util::StreamExt;
use std::{future::Future, time::Duration};

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        sse::parse_sse_line,
        types::{ChatCompletionResponse, ModelsListResponse, build_chat_request},
    },
};

#[derive(Clone)]
pub struct OpenAiCompatibleChatClient {
    http: reqwest::Client,
}

impl OpenAiCompatibleChatClient {
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

    fn models_url(&self, provider: &ModelProvider) -> String {
        format!("{}/models", provider.base_url.trim_end_matches('/'))
    }

    async fn retry_request<T, Fut, F>(
        &self,
        options: RequestOptions,
        mut request: F,
    ) -> AppResult<T>
    where
        F: FnMut() -> Fut + Send,
        Fut: Future<Output = Result<T, reqwest::Error>> + Send,
    {
        let attempts = options.retry_attempts.max(1);
        let mut last_error = None;
        for attempt in 1..=attempts {
            match tokio::time::timeout(options.timeout, request()).await {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(error)) => last_error = Some(AppError::Llm(error.to_string())),
                Err(_) => {
                    last_error = Some(AppError::Llm(format!(
                        "request timed out after {}s",
                        options.timeout.as_secs()
                    )));
                }
            }
            if attempt < attempts {
                tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm("request failed".to_string())))
    }
}

#[async_trait]
impl ChatClient for OpenAiCompatibleChatClient {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
    ) -> AppResult<String> {
        let request =
            build_chat_request(agent, messages, false, system_prompt, &provider.extra_body);
        let api_key = self.api_key(provider)?.to_string();
        let chat_url = self.chat_url(provider);
        let response = self
            .retry_request(options, || {
                self.http
                    .post(chat_url.clone())
                    .bearer_auth(api_key.clone())
                    .json(&request)
                    .send()
            })
            .await?;

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
        system_prompt: &str,
        options: RequestOptions,
    ) -> AppResult<ChatStream> {
        let request =
            build_chat_request(agent, messages, true, system_prompt, &provider.extra_body);
        let api_key = self.api_key(provider)?.to_string();
        let chat_url = self.chat_url(provider);
        let response = self
            .retry_request(options, || {
                self.http
                    .post(chat_url.clone())
                    .bearer_auth(api_key.clone())
                    .json(&request)
                    .send()
            })
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

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>> {
        let response = self
            .http
            .get(self.models_url(provider))
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

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use crate::domain::{Agent, ChatMessage, ChatMessageStatus, ChatRole};

    use super::*;

    #[test]
    fn builds_openai_compatible_chat_completion_request() {
        let now = Utc::now();
        let provider_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            model_provider_id: provider_id,
            model_id: "gpt-4o-mini".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id,
            role: ChatRole::User,
            content: "hello".to_string(),
            status: ChatMessageStatus::Done,
            created_at: now,
        }];

        let request = build_chat_request(
            &agent,
            &messages,
            true,
            "You write Rust.",
            &serde_json::Value::Null,
        );

        assert_eq!(request.model, "gpt-4o-mini");
        assert!(request.stream);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[0].content, "You write Rust.");
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.messages[1].content, "hello");
    }
}
