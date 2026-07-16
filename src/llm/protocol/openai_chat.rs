use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        http::{
            LlmHttp, MAX_LLM_LOG_BYTES, MAX_SSE_LINE_BUFFER_BYTES, decode_remaining_sse_line,
            drain_sse_line, read_response_body_limited, spawn_log,
        },
        sse::{openai_finish_event, parse_sse_line},
        types::{
            ChatCompletionOutput, ChatCompletionResponse, ModelsListResponse, ToolDefinition,
            build_chat_request,
        },
    },
    storage::LlmLogger,
};

#[derive(Clone)]
pub struct OpenAiChatClient {
    http: LlmHttp,
}

impl OpenAiChatClient {
    pub fn new() -> Self {
        Self {
            http: LlmHttp::new(),
        }
    }

    pub fn with_logger(mut self, logger: Arc<LlmLogger>) -> Self {
        self.http = self.http.with_logger(logger);
        self
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
}

impl Default for OpenAiChatClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChatClient for OpenAiChatClient {
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
        let api_key = self.http.api_key(&provider.api_key)?.to_string();
        let chat_url = self.chat_url(provider);
        let log_enabled = self.http.log_enabled();

        let response = match self
            .http
            .retry_send(options, || {
                self.http
                    .client()
                    .post(chat_url.clone())
                    .bearer_auth(api_key.clone())
                    .json(&request)
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.http.spawn_log(
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
        let body = match read_response_body_limited(response, options.timeout).await {
            Ok(body) => body,
            Err(error) => {
                self.http.spawn_log(
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
            self.http.spawn_log(
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
                self.http.spawn_log(
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
            self.http.spawn_log(
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

        if let Some(reason) = choice.finish_reason.as_deref()
            && let Err(error) = openai_finish_event(reason)
        {
            self.http.spawn_log(
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&error.to_string()),
            );
            return Err(error);
        }

        let content = choice.message.content.unwrap_or_default();
        let tool_calls = choice.message.tool_calls.unwrap_or_default();
        if content.is_empty() && tool_calls.is_empty() {
            self.http.spawn_log(
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

        self.http.spawn_log(
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
        let api_key = self.http.api_key(&provider.api_key)?.to_string();
        let chat_url = self.chat_url(provider);
        let log_enabled = self.http.log_enabled();

        let response = match self
            .http
            .retry_send(options, || {
                self.http
                    .client()
                    .post(chat_url.clone())
                    .bearer_auth(api_key.clone())
                    .json(&request)
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.http.spawn_log(
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
            let body = read_response_body_limited(response, options.timeout)
                .await
                .unwrap_or_else(|_| "failed to read error body".to_string());
            self.http.spawn_log(
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

        let logger_for_stream = self.http.logger();
        let agent_name = agent.name.clone();
        let agent_model_id = agent.model_id.clone();
        let provider_name = provider.name.clone();
        let stream_status = status.as_u16();
        let mut bytes = response.bytes_stream();
        let stream = async_stream::stream! {
            let mut buffer: Vec<u8> = Vec::new();

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
                    if self.completed || !self.enabled {
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
                agent_name,
                model_id: agent_model_id,
                provider_name,
                request_json,
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
                let Some(chunk) = next else {
                    break;
                };
                match chunk {
                    Ok(bytes) => {
                        buffer.extend_from_slice(&bytes);
                        while let Some(line) = drain_sse_line(&mut buffer) {
                            let line = match line {
                                Ok(line) => line,
                                Err(error) => {
                                    spawn_log(
                                        &guard.logger,
                                        guard.enabled,
                                        &guard.agent_name,
                                        &guard.model_id,
                                        &guard.provider_name,
                                        &guard.request_json,
                                        Some(&guard.raw_lines.join("\n")),
                                        Some(guard.raw_status),
                                        Some(&error.to_string()),
                                    );
                                    guard.completed = true;
                                    yield Err(error);
                                    return;
                                }
                            };
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
                                        spawn_log(
                                            &guard.logger,
                                            guard.enabled,
                                            &guard.agent_name,
                                            &guard.model_id,
                                            &guard.provider_name,
                                            &guard.request_json,
                                            Some(&guard.raw_lines.join("\n")),
                                            Some(guard.raw_status),
                                            Some(&error.to_string()),
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
                        spawn_log(
                            &guard.logger,
                            guard.enabled,
                            &guard.agent_name,
                            &guard.model_id,
                            &guard.provider_name,
                            &guard.request_json,
                            Some(&format!(
                                "[SSE stream error, {} raw lines received]\n{}",
                                guard.raw_lines.len(),
                                guard.raw_lines.join("\n")
                            )),
                            Some(guard.raw_status),
                            Some(&error.to_string()),
                        );
                        guard.completed = true;
                        yield Err(AppError::Llm(error.to_string()));
                        return;
                    }
                }
            }
            if let Some(line) = match decode_remaining_sse_line(&buffer) {
                Ok(line) => line,
                Err(error) => {
                    spawn_log(
                        &guard.logger,
                        guard.enabled,
                        &guard.agent_name,
                        &guard.model_id,
                        &guard.provider_name,
                        &guard.request_json,
                        Some(&guard.raw_lines.join("\n")),
                        Some(guard.raw_status),
                        Some(&error.to_string()),
                    );
                    guard.completed = true;
                    yield Err(error);
                    return;
                }
            } {
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

            guard.completed = true;
            spawn_log(
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
            .client()
            .get(self.models_url(provider))
            .bearer_auth(self.http.api_key(&provider.api_key)?)
            .send()
            .await
            .map_err(|error| AppError::Llm(error.to_string()))?;

        let status = response.status();
        let body =
            read_response_body_limited(response, crate::llm::http::DEFAULT_MODEL_LIST_TIMEOUT)
                .await?;
        if !status.is_success() {
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let parsed: ModelsListResponse =
            serde_json::from_str(&body).map_err(|error| AppError::Llm(error.to_string()))?;

        let mut models: Vec<String> = parsed
            .data
            .into_iter()
            .map(|model| model.id)
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

    use super::*;
    use crate::domain::{ChatMessageStatus, ChatRole};

    #[test]
    fn builds_openai_chat_completion_request() {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes and reviews code.".to_string(),
            model_provider_id: Uuid::new_v4(),
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
            conversation_id: Uuid::new_v4(),
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
        assert_eq!(request.messages[1].role, "user");
    }

    #[test]
    fn parses_nullable_content_for_tool_calls() {
        let parsed: ChatCompletionResponse = serde_json::from_str(
            r#"{
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": "run", "arguments": "{}"}
                        }]
                    }
                }]
            }"#,
        )
        .unwrap();

        let choice = parsed.choices.into_iter().next().unwrap();
        assert_eq!(choice.message.content.unwrap_or_default(), "");
        assert_eq!(choice.message.tool_calls.unwrap()[0].id, "call-1");
    }
}
