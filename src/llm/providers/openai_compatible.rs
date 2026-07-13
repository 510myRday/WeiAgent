use async_trait::async_trait;
use futures_util::StreamExt;
use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        sse::parse_sse_line,
        types::{
            ChatCompletionOutput, ChatCompletionResponse, ModelsListResponse, ToolDefinition,
            build_chat_request,
        },
    },
    storage::LlmLogger,
};

const MAX_LLM_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_LLM_LOG_BYTES: usize = 1024 * 1024;
const MAX_SSE_LINE_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct OpenAiCompatibleChatClient {
    http: reqwest::Client,
    llm_logger: Option<Arc<LlmLogger>>,
}

impl OpenAiCompatibleChatClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            llm_logger: None,
        }
    }

    pub fn with_logger(mut self, logger: Arc<LlmLogger>) -> Self {
        self.llm_logger = Some(logger);
        self
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

    #[allow(clippy::too_many_arguments)]
    fn spawn_log(
        logger: &Option<Arc<LlmLogger>>,
        enabled: bool,
        agent_name: &str,
        model_id: &str,
        provider_name: &str,
        raw_request: &str,
        raw_response: Option<&str>,
        raw_status: Option<u16>,
        error: Option<&str>,
    ) {
        if !enabled {
            return;
        }
        if let Some(logger) = logger {
            let logger = logger.clone();
            let agent_name = agent_name.to_string();
            let model_id = model_id.to_string();
            let provider_name = provider_name.to_string();
            let raw_request = raw_request.to_string();
            let raw_response = raw_response.map(|s| s.to_string());
            let error = error.map(|s| s.to_string());
            tokio::spawn(async move {
                let _ = logger
                    .write_log(
                        &agent_name,
                        &model_id,
                        &provider_name,
                        &raw_request,
                        raw_response.as_deref(),
                        raw_status,
                        error.as_deref(),
                    )
                    .await;
            });
        }
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
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatCompletionOutput> {
        let request = build_chat_request(
            agent,
            messages,
            false,
            system_prompt,
            &provider.extra_body,
            tools,
        );
        let request_json =
            serde_json::to_string_pretty(&request).unwrap_or_else(|_| format!("{request:?}"));
        let api_key = self.api_key(provider)?.to_string();
        let chat_url = self.chat_url(provider);

        // Snapshot log state at request start
        let log_enabled = self
            .llm_logger
            .as_ref()
            .map_or(false, |logger| logger.is_enabled());

        let response = match self
            .retry_request(options, || {
                self.http
                    .post(chat_url.clone())
                    .bearer_auth(api_key.clone())
                    .json(&request)
                    .send()
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                Self::spawn_log(
                    &self.llm_logger,
                    log_enabled,
                    &agent.name,
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    None,
                    None,
                    Some(&error.to_string()),
                );
                return Err(error);
            }
        };

        let status = response.status();
        let body = match read_response_body_limited(response).await {
            Ok(body) => body,
            Err(error) => {
                Self::spawn_log(
                    &self.llm_logger,
                    log_enabled,
                    &agent.name,
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    None,
                    Some(status.as_u16()),
                    Some(&format!("failed to read response body: {error}")),
                );
                return Err(error);
            }
        };

        if !status.is_success() {
            Self::spawn_log(
                &self.llm_logger,
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&format!("upstream returned {status}")),
            );
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let response_parsed: ChatCompletionResponse =
            serde_json::from_str(&body).map_err(|error| {
                Self::spawn_log(
                    &self.llm_logger,
                    log_enabled,
                    &agent.name,
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    Some(&body),
                    Some(status.as_u16()),
                    Some(&error.to_string()),
                );
                AppError::Llm(error.to_string())
            })?;

        let choice = response_parsed.choices.into_iter().next().ok_or_else(|| {
            Self::spawn_log(
                &self.llm_logger,
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some("empty model response"),
            );
            AppError::Llm("empty model response".to_string())
        })?;

        let content = choice.message.content;
        let tool_calls = choice.message.tool_calls.unwrap_or_default();
        if content.is_empty() && tool_calls.is_empty() {
            Self::spawn_log(
                &self.llm_logger,
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some("empty model response"),
            );
            return Err(AppError::Llm("empty model response".to_string()));
        }

        Self::spawn_log(
            &self.llm_logger,
            log_enabled,
            &agent.name,
            &agent.model_id,
            &provider.name,
            &request_json,
            Some(&body),
            Some(status.as_u16()),
            None,
        );

        Ok(ChatCompletionOutput {
            content,
            tool_calls,
        })
    }

    async fn stream(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatStream> {
        let request = build_chat_request(
            agent,
            messages,
            true,
            system_prompt,
            &provider.extra_body,
            tools,
        );
        let request_json =
            serde_json::to_string_pretty(&request).unwrap_or_else(|_| format!("{request:?}"));
        let api_key = self.api_key(provider)?.to_string();
        let chat_url = self.chat_url(provider);

        // Snapshot log state at request start
        let log_enabled = self
            .llm_logger
            .as_ref()
            .map_or(false, |logger| logger.is_enabled());

        let response = match self
            .retry_request(options, || {
                self.http
                    .post(chat_url.clone())
                    .bearer_auth(api_key.clone())
                    .json(&request)
                    .send()
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                Self::spawn_log(
                    &self.llm_logger,
                    log_enabled,
                    &agent.name,
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    None,
                    None,
                    Some(&error.to_string()),
                );
                return Err(error);
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = read_response_body_limited(response)
                .await
                .unwrap_or_else(|_| "failed to read error body".to_string());
            Self::spawn_log(
                &self.llm_logger,
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&format!("upstream returned {status}")),
            );
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let logger_for_stream = self.llm_logger.clone();
        let agent_name = agent.name.clone();
        let agent_model_id = agent.model_id.clone();
        let provider_name = provider.name.clone();
        let request_json_clone = request_json.clone();
        let stream_status = status.as_u16();

        let mut bytes = response.bytes_stream();
        let stream = async_stream::stream! {
            let mut buffer = String::new();

            // Drop guard: if stream is dropped before completion, write what we have
            struct StreamLogGuard {
                logger: Option<Arc<LlmLogger>>,
                enabled: bool,
                agent_name: String,
                model_id: String,
                provider_name: String,
                request_json: String,
                raw_lines: Vec<String>,
                raw_bytes: usize,
                raw_truncated: bool,
                raw_status: u16,
                completed: bool,
            }
            impl Drop for StreamLogGuard {
                fn drop(&mut self) {
                    if self.completed {
                        return;
                    }
                    if !self.enabled {
                        return;
                    }
                    if let Some(ref logger) = self.logger {
                        let logger = logger.clone();
                        let agent_name = self.agent_name.clone();
                        let model_id = self.model_id.clone();
                        let provider_name = self.provider_name.clone();
                        let request_json = self.request_json.clone();
                        let raw_lines = std::mem::take(&mut self.raw_lines);
                        let raw_truncated = self.raw_truncated;
                        let raw_status = self.raw_status;
                        tokio::spawn(async move {
                            let response_body = if raw_lines.is_empty() {
                                None
                            } else {
                                Some(format!(
                                    "{}{}",
                                    raw_lines.join("\n"),
                                    if raw_truncated {
                                        "\n[LLM stream log truncated]"
                                    } else {
                                        ""
                                    }
                                ))
                            };
                            let _ = logger
                                .write_log(
                                    &agent_name,
                                    &model_id,
                                    &provider_name,
                                    &request_json,
                                    response_body.as_deref(),
                                    Some(raw_status),
                                    Some("stream cancelled or connection dropped"),
                                )
                                .await;
                        });
                    }
                }
            }
            let mut guard = StreamLogGuard {
                logger: logger_for_stream,
                enabled: log_enabled,
                agent_name: agent_name.clone(),
                model_id: agent_model_id.clone(),
                provider_name: provider_name.clone(),
                request_json: request_json_clone,
                raw_lines: Vec::new(),
                raw_bytes: 0,
                raw_truncated: false,
                raw_status: stream_status,
                completed: false,
            };

            loop {
                let next = match tokio::time::timeout(options.timeout, bytes.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        let error = AppError::Llm(format!(
                            "LLM stream idle timeout after {}s",
                            options.timeout.as_secs()
                        ));
                        guard.completed = true;
                        yield Err(error);
                        return;
                    }
                };
                let Some(chunk) = next else { break; };
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(index) = buffer.find('\n') {
                            let line = buffer[..index].trim_end_matches('\r').to_string();
                            buffer = buffer[index + 1..].to_string();
                            if guard.raw_bytes.saturating_add(line.len()) <= MAX_LLM_LOG_BYTES {
                                guard.raw_bytes += line.len();
                                guard.raw_lines.push(line.clone());
                            } else {
                                guard.raw_truncated = true;
                            }
                            for item in parse_sse_line(&line) {
                                match item {
                                    Ok(event) => {
                                        yield Ok(event);
                                    }
                                    Err(error) => {
                                        let error_text = error.to_string();
                                        Self::spawn_log(
                                            &guard.logger,
                                            guard.enabled,
                                            &guard.agent_name,
                                            &guard.model_id,
                                            &guard.provider_name,
                                            &guard.request_json,
                                            Some(&guard.raw_lines.join("\n")),
                                            Some(guard.raw_status),
                                            Some(&error_text),
                                        );
                                        guard.completed = true;
                                        yield Err(error);
                                        return;
                                    }
                                }
                            }
                        }
                        if buffer.len() > MAX_SSE_LINE_BUFFER_BYTES {
                            guard.completed = true;
                            yield Err(AppError::Llm(format!(
                                "SSE line exceeded {MAX_SSE_LINE_BUFFER_BYTES} bytes"
                            )));
                            return;
                        }
                    }
                    Err(error) => {
                        Self::spawn_log(
                            &guard.logger,
                            guard.enabled,
                            &guard.agent_name,
                            &guard.model_id,
                            &guard.provider_name,
                            &guard.request_json,
                            Some(&format!("[SSE stream error, {} raw lines received]\n{}", guard.raw_lines.len(), guard.raw_lines.join("\n"))),
                            Some(guard.raw_status),
                            Some(&error.to_string()),
                        );
                        guard.completed = true;
                        yield Err(AppError::Llm(error.to_string()));
                        return;
                    }
                }
            }
            if !buffer.trim().is_empty() {
                let line = buffer.trim_end_matches('\r').to_string();
                if guard.raw_bytes.saturating_add(line.len()) <= MAX_LLM_LOG_BYTES {
                    guard.raw_bytes += line.len();
                    guard.raw_lines.push(line.clone());
                } else {
                    guard.raw_truncated = true;
                }
                for item in parse_sse_line(&line) {
                    match item {
                        Ok(event) => yield Ok(event),
                        Err(error) => {
                            guard.completed = true;
                            yield Err(error);
                            return;
                        }
                    }
                }
            }

            // Normal completion
            guard.completed = true;
            Self::spawn_log(
                &guard.logger,
                guard.enabled,
                &guard.agent_name,
                &guard.model_id,
                &guard.provider_name,
                &guard.request_json,
                Some(&guard.raw_lines.join("\n")),
                Some(guard.raw_status),
                None,
            );
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
        let body = read_response_body_limited(response).await?;
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

async fn read_response_body_limited(response: reqwest::Response) -> AppResult<String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| AppError::Llm(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_LLM_RESPONSE_BYTES {
            return Err(AppError::Llm(format!(
                "LLM response exceeded {MAX_LLM_RESPONSE_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
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
            tool_calls: None,
            tool_call_id: None,
        }];

        let request = build_chat_request(
            &agent,
            &messages,
            true,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
        );

        assert_eq!(request.model, "gpt-4o-mini");
        assert!(request.stream);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[0].content, "You write Rust.");
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.messages[1].content, "hello");
    }
}
