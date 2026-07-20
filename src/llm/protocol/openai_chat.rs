use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use futures_util::StreamExt;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        cache::{apply_openai_fields, apply_provider_extra, build_context},
        http::{
            LlmHttp, MAX_SSE_LINE_BUFFER_BYTES, capture_log_line, decode_remaining_sse_line,
            drain_sse_line, format_stream_log, read_response_body_limited, spawn_log,
            stream_metrics,
        },
        protocol::common::request_json_for_log,
        sse::{meaningful_finish_reason, openai_finish_event, parse_sse_line},
        types::{
            ChatCompletionOutput, ChatCompletionResponse, ModelsListResponse, ToolDefinition,
            build_chat_request,
        },
    },
    storage::{llm_logs::{elapsed_ms, LlmLogMetrics}, LlmLogger},
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
        let mut request = build_chat_request(
            agent,
            messages,
            false,
            system_prompt,
            &provider.extra_body,
            tools,
            options.thinking_level,
        );
        let context = build_context(
            provider,
            &agent.model_id,
            agent.id,
            "openai_chat",
            system_prompt,
            request.tools.as_deref(),
        );
        if let serde_json::Value::Object(mut extra) = request.extra_body {
            apply_openai_fields(&mut extra, provider, &context);
            apply_provider_extra(&mut extra, &context);
            request.extra_body = serde_json::Value::Object(extra);
        }
        let request_json = serde_json::to_value(&request)
            .map(|value| request_json_for_log(&value))
            .unwrap_or_else(|_| format!("{request:?}"));
        let api_key = self.http.api_key(&provider.api_key)?.to_string();
        let chat_url = self.chat_url(provider);
        let log_enabled = self.http.log_enabled();
        let started = Instant::now();
        let agent_id = agent.id.to_string();
        let conversation_id = messages
            .last()
            .map(|message| message.conversation_id.to_string());


        let response = match self
            .http
            .retry_send(&options, || {
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
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    None,
                    None,
                    Some(&error.to_string()),
                    LlmLogMetrics::timed(started),
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
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    None,
                    Some(status.as_u16()),
                    Some(&format!("failed to read response body: {error}")),
                    LlmLogMetrics::timed(started),
                );
                return Err(error);
            }
        };

        if !status.is_success() {
            self.http.spawn_log(
                log_enabled,
                &agent.name,
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&format!("upstream returned {status}")),
                    LlmLogMetrics::timed(started),
                );
            return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
        }

        let response_parsed: ChatCompletionResponse =
            serde_json::from_str(&body).map_err(|error| {
                self.http.spawn_log(
                    log_enabled,
                    &agent.name,
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    Some(&body),
                    Some(status.as_u16()),
                    Some(&error.to_string()),
                    LlmLogMetrics::timed(started),
                );
                AppError::Llm(error.to_string())
            })?;

        let usage = response_parsed
            .usage
            .as_ref()
            .map(crate::llm::types::OpenAiUsage::normalized)
            .unwrap_or_default();
        let choice = response_parsed.choices.into_iter().next().ok_or_else(|| {
            self.http.spawn_log(
                log_enabled,
                &agent.name,
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some("empty model response"),
                    LlmLogMetrics::timed(started),
                );
            AppError::Llm("empty model response".to_string())
        })?;

        if let Some(reason) = meaningful_finish_reason(choice.finish_reason.as_deref())
            && let Err(error) = openai_finish_event(reason)
        {
            self.http.spawn_log(
                log_enabled,
                &agent.name,
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&error.to_string()),
                    LlmLogMetrics::timed(started),
                );
            return Err(error);
        }

        let content = choice.message.content.unwrap_or_default();
        let reasoning_content = choice.message.reasoning_content.unwrap_or_default();
        let tool_calls = choice.message.tool_calls.unwrap_or_default();
        if content.is_empty() && reasoning_content.is_empty() && tool_calls.is_empty() {
            self.http.spawn_log(
                log_enabled,
                &agent.name,
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some("empty model response"),
                    LlmLogMetrics::timed(started),
                );
            return Err(AppError::Llm("empty model response".to_string()));
        }

        self.http.spawn_log(
            log_enabled,
            &agent.name,
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
            &agent.model_id,
            &provider.name,
            &request_json,
            Some(&body),
            Some(status.as_u16()),
            None,
                    LlmLogMetrics::timed(started),
                );

        Ok(ChatCompletionOutput {
            content,
            reasoning_content,
            tool_calls,
            usage,
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
        let mut request = build_chat_request(
            agent,
            messages,
            true,
            system_prompt,
            &provider.extra_body,
            tools,
            options.thinking_level,
        );
        let context = build_context(
            provider,
            &agent.model_id,
            agent.id,
            "openai_chat",
            system_prompt,
            request.tools.as_deref(),
        );
        if let serde_json::Value::Object(mut extra) = request.extra_body {
            apply_openai_fields(&mut extra, provider, &context);
            apply_provider_extra(&mut extra, &context);
            request.extra_body = serde_json::Value::Object(extra);
        }
        let request_json = serde_json::to_value(&request)
            .map(|value| request_json_for_log(&value))
            .unwrap_or_else(|_| format!("{request:?}"));
        let api_key = self.http.api_key(&provider.api_key)?.to_string();
        let chat_url = self.chat_url(provider);
        let log_enabled = self.http.log_enabled();
        let started = Instant::now();
        let agent_id = agent.id.to_string();
        let conversation_id = messages
            .last()
            .map(|message| message.conversation_id.to_string());


        let response = match self
            .http
            .retry_send(&options, || {
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
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                    &agent.model_id,
                    &provider.name,
                    &request_json,
                    None,
                    None,
                    Some(&error.to_string()),
                    LlmLogMetrics::timed(started),
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
                    Some(agent_id.as_str()),
                    conversation_id.as_deref(),
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&format!("upstream returned {status}")),
                    LlmLogMetrics::timed(started),
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
                agent_id: Option<String>,
                conversation_id: Option<String>,
                model_id: String,
                provider_name: String,
                request_json: String,
                raw_lines: Vec<String>,
                raw_bytes: usize,
                raw_truncated: bool,
                raw_status: u16,
                started: Instant,
                first_token_ms: Option<u64>,
                usage: Option<crate::domain::TokenUsage>,
                completed: bool,
            }

            impl StreamLogGuard {
                fn metrics(&self) -> LlmLogMetrics {
                    stream_metrics(self.started, self.first_token_ms, self.usage.as_ref())
                }

                fn note_event(&mut self, event: &crate::llm::types::StreamEvent) {
                    if let crate::llm::types::StreamEvent::Usage(usage) = event {
                        self.usage = Some(usage.clone());
                    }
                    if self.first_token_ms.is_some() {
                        return;
                    }
                    if matches!(
                        event,
                        crate::llm::types::StreamEvent::TextDelta(_)
                            | crate::llm::types::StreamEvent::ReasoningDelta(_)
                            | crate::llm::types::StreamEvent::ToolCallStart { .. }
                    ) {
                        self.first_token_ms = Some(elapsed_ms(self.started));
                    }
                }
            }

            impl Drop for StreamLogGuard {
                fn drop(&mut self) {
                    if self.completed {
                        return;
                    }
                    if let Some(ref logger) = self.logger {
                        let logger = logger.clone();
                        let include_payload = self.enabled;
                        let agent_name = self.agent_name.clone();
                        let agent_id = self.agent_id.clone();
                        let conversation_id = self.conversation_id.clone();
                        let model_id = self.model_id.clone();
                        let provider_name = self.provider_name.clone();
                        let request_json = self.request_json.clone();
                        let raw_lines = std::mem::take(&mut self.raw_lines);
                        let raw_truncated = self.raw_truncated;
                        let raw_status = self.raw_status;
                        let metrics = self.metrics();
                        tokio::spawn(async move {
                            let response_body = (!raw_lines.is_empty())
                                .then(|| format_stream_log(&raw_lines, raw_truncated));
                            let _ = logger
                                .write_log(
                                    &agent_name,
                                    agent_id.as_deref(),
                                    conversation_id.as_deref(),
                                    &model_id,
                                    &provider_name,
                                    &request_json,
                                    response_body.as_deref(),
                                    Some(raw_status),
                                    Some("stream cancelled or connection dropped"),
                                    metrics,
                                    include_payload,
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
                agent_id: Some(agent_id),
                conversation_id,
                model_id: agent_model_id,
                provider_name,
                request_json,
                raw_lines: Vec::new(),
                raw_bytes: 0,
                raw_truncated: false,
                raw_status: stream_status,
                started,
                first_token_ms: None,
                usage: None,
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
                                            guard.agent_id.as_deref(),
                                            guard.conversation_id.as_deref(),
                                        &guard.model_id,
                                        &guard.provider_name,
                                        &guard.request_json,
                                        Some(&format_stream_log(&guard.raw_lines, guard.raw_truncated)),
                                        Some(guard.raw_status),
                                        Some(&error.to_string()),
                    LlmLogMetrics::timed(started),
                );
                                    guard.completed = true;
                                    yield Err(error);
                                    return;
                                }
                            };
                            capture_log_line(
                                &line,
                                &mut guard.raw_lines,
                                &mut guard.raw_bytes,
                                &mut guard.raw_truncated,
                            );
                            for item in parse_sse_line(&line) {
                                match item {
                                    Ok(event) => {
                                        guard.note_event(&event);
                                        yield Ok(event);
                                    },
                                    Err(error) => {
                                        spawn_log(
                                            &guard.logger,
                                            guard.enabled,
                                            &guard.agent_name,
                                            guard.agent_id.as_deref(),
                                            guard.conversation_id.as_deref(),
                                            &guard.model_id,
                                            &guard.provider_name,
                                            &guard.request_json,
                                            Some(&format_stream_log(&guard.raw_lines, guard.raw_truncated)),
                                            Some(guard.raw_status),
                                            Some(&error.to_string()),
                                            guard.metrics(),
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
                                            guard.agent_id.as_deref(),
                                            guard.conversation_id.as_deref(),
                            &guard.model_id,
                            &guard.provider_name,
                            &guard.request_json,
                            Some(&format!(
                                "[SSE stream error, {} raw lines received]\n{}",
                                guard.raw_lines.len(),
                                format_stream_log(&guard.raw_lines, guard.raw_truncated)
                            )),
                            Some(guard.raw_status),
                            Some(&error.to_string()),
                            guard.metrics(),
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
                                            guard.agent_id.as_deref(),
                                            guard.conversation_id.as_deref(),
                        &guard.model_id,
                        &guard.provider_name,
                        &guard.request_json,
                        Some(&format_stream_log(&guard.raw_lines, guard.raw_truncated)),
                        Some(guard.raw_status),
                        Some(&error.to_string()),
                        guard.metrics(),
                    );
                    guard.completed = true;
                    yield Err(error);
                    return;
                }
            } {
                capture_log_line(
                    &line,
                    &mut guard.raw_lines,
                    &mut guard.raw_bytes,
                    &mut guard.raw_truncated,
                );
                for item in parse_sse_line(&line) {
                    match item {
                        Ok(event) => {
                                        guard.note_event(&event);
                                        yield Ok(event);
                                    },
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
                                            guard.agent_id.as_deref(),
                                            guard.conversation_id.as_deref(),
                &guard.model_id,
                &guard.provider_name,
                &guard.request_json,
                Some(&format_stream_log(&guard.raw_lines, guard.raw_truncated)),
                Some(guard.raw_status),
                None,
                guard.metrics(),
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
            reasoning_content: Some("saved-reasoning-must-not-be-sent".to_string()),
            status: ChatMessageStatus::Done,
            created_at: now,
            tool_calls: None,
            tool_call_id: None,
            attachments: vec![crate::domain::ImageAttachment {
                id: "image-1".to_string(),
                name: "sample.png".to_string(),
                media_type: "image/png".to_string(),
                source: crate::domain::ImageSource::Base64 {
                    data: "aGVsbG8=".to_string(),
                },
            }],
            usage: None,
        }];

        let request = build_chat_request(
            &agent,
            &messages,
            true,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
            Some(crate::domain::ThinkingLevel::High),
        );

        assert_eq!(request.model, "gpt-4o-mini");
        assert!(request.stream);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            request.messages[1].content[1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert_eq!(
            request.stream_options.as_ref().unwrap()["include_usage"],
            true
        );
        assert!(
            !serde_json::to_string(&request)
                .unwrap()
                .contains("saved-reasoning-must-not-be-sent")
        );
    }

    #[test]
    fn omits_reasoning_effort_when_thinking_is_off() {
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
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: now,
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }];

        let off_request = build_chat_request(
            &agent,
            &messages,
            false,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
            Some(crate::domain::ThinkingLevel::Off),
        );
        assert!(off_request.reasoning_effort.is_none());
        let serialized = serde_json::to_string(&off_request).unwrap();
        assert!(!serialized.contains("reasoning_effort"));

        let none_request = build_chat_request(
            &agent,
            &messages,
            false,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
            None,
        );
        assert!(none_request.reasoning_effort.is_none());
        let serialized = serde_json::to_string(&none_request).unwrap();
        assert!(!serialized.contains("reasoning_effort"));
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
