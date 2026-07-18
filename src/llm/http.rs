use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response};

use crate::{
    error::{AppError, AppResult},
    llm::{
        client::{ChatStream, RequestOptions},
        types::StreamEvent,
    },
    storage::{
        llm_logs::{elapsed_ms, LlmLogMetrics},
        LlmLogger,
    },
};

pub const MAX_LLM_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LLM_LOG_BYTES: usize = 1024 * 1024;
pub const MAX_SSE_LINE_BUFFER_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct LlmHttp {
    client: reqwest::Client,
    logger: Option<Arc<LlmLogger>>,
}

impl LlmHttp {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            logger: None,
        }
    }

    pub fn with_logger(mut self, logger: Arc<LlmLogger>) -> Self {
        self.logger = Some(logger);
        self
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn logger(&self) -> Option<Arc<LlmLogger>> {
        self.logger.clone()
    }

    pub fn log_enabled(&self) -> bool {
        self.logger
            .as_ref()
            .is_some_and(|logger| logger.is_enabled())
    }

    pub fn api_key<'a>(&self, api_key: &'a str) -> AppResult<&'a str> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(AppError::MissingApiKey);
        }
        Ok(api_key)
    }

    pub async fn retry_send<F>(&self, options: &RequestOptions, request: F) -> AppResult<Response>
    where
        F: Fn() -> RequestBuilder,
    {
        let attempts = options.retry_attempts.max(1);
        let mut last_error = None;
        for attempt in 1..=attempts {
            match tokio::time::timeout(options.timeout, request().send()).await {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(error)) => last_error = Some(AppError::Llm(error.to_string())),
                Err(_) => {
                    last_error = Some(AppError::Llm(format!(
                        "request timed out after {}s",
                        options.timeout.as_secs()
                    )));
                }
            }
            if attempt < attempts {
                let reason = last_error
                    .as_ref()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "request failed".to_string());
                if let Some(on_retry) = options.on_retry.as_ref() {
                    on_retry(attempt + 1, attempts, &reason);
                }
                tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm("request failed".to_string())))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_log(
        &self,
        // When true, persist full request/response payloads; summary metrics always persist.
        include_payload: bool,
        agent_name: &str,
        agent_id: Option<&str>,
        conversation_id: Option<&str>,
        model_id: &str,
        provider_name: &str,
        raw_request: &str,
        raw_response: Option<&str>,
        raw_status: Option<u16>,
        error: Option<&str>,
        metrics: LlmLogMetrics,
    ) {
        spawn_log(
            &self.logger,
            include_payload,
            agent_name,
            agent_id,
            conversation_id,
            model_id,
            provider_name,
            raw_request,
            raw_response,
            raw_status,
            error,
            metrics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_log(
    logger: &Option<Arc<LlmLogger>>,
    // When true, persist full request/response payloads; summary metrics always persist.
    include_payload: bool,
    agent_name: &str,
    agent_id: Option<&str>,
    conversation_id: Option<&str>,
    model_id: &str,
    provider_name: &str,
    raw_request: &str,
    raw_response: Option<&str>,
    raw_status: Option<u16>,
    error: Option<&str>,
    metrics: LlmLogMetrics,
) {
    if let Some(logger) = logger {
        let logger = logger.clone();
        let agent_name = agent_name.to_string();
        let agent_id = agent_id.map(str::to_string);
        let conversation_id = conversation_id.map(str::to_string);
        let model_id = model_id.to_string();
        let provider_name = provider_name.to_string();
        let raw_request = raw_request.to_string();
        let raw_response = raw_response.map(ToString::to_string);
        let error = error.map(ToString::to_string);
        tokio::spawn(async move {
            let _ = logger
                .write_log(
                    &agent_name,
                    agent_id.as_deref(),
                    conversation_id.as_deref(),
                    &model_id,
                    &provider_name,
                    &raw_request,
                    raw_response.as_deref(),
                    raw_status,
                    error.as_deref(),
                    metrics,
                    include_payload,
                )
                .await;
        });
    }
}

pub async fn read_response_body_limited(
    response: Response,
    idle_timeout: Duration,
) -> AppResult<String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let next = tokio::time::timeout(idle_timeout, stream.next())
            .await
            .map_err(|_| {
                AppError::Llm(format!(
                    "LLM response body idle timeout after {}s",
                    idle_timeout.as_secs()
                ))
            })?;
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|error| AppError::Llm(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_LLM_RESPONSE_BYTES {
            return Err(AppError::Llm(format!(
                "LLM response exceeded {MAX_LLM_RESPONSE_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|error| AppError::Llm(format!("LLM response was not valid UTF-8: {error}")))
}

pub trait StreamLineParser: Send {
    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>>;
}

pub struct StreamLogContext {
    pub logger: Option<Arc<LlmLogger>>,
    pub enabled: bool,
    pub agent_name: String,
    pub agent_id: Option<String>,
    pub conversation_id: Option<String>,
    pub model_id: String,
    pub provider_name: String,
    pub request_json: String,
    pub status: u16,
    pub started: Instant,
}

fn is_first_token_event(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta(_)
            | StreamEvent::ReasoningDelta(_)
            | StreamEvent::ToolCallStart { .. }
    )
}

fn stream_metrics(started: Instant, first_token_ms: Option<u64>) -> LlmLogMetrics {
    LlmLogMetrics::timed(started).with_first_token(first_token_ms)
}

pub fn capture_log_line(
    line: &str,
    raw_lines: &mut Vec<String>,
    raw_bytes: &mut usize,
    raw_truncated: &mut bool,
) {
    if raw_bytes.saturating_add(line.len()) <= MAX_LLM_LOG_BYTES {
        *raw_bytes += line.len();
        raw_lines.push(line.to_string());
    } else {
        *raw_truncated = true;
    }
}

pub fn format_stream_log(raw_lines: &[String], truncated: bool) -> String {
    format!(
        "{}{}",
        raw_lines.join("\n"),
        if truncated {
            "\n[LLM stream log truncated]"
        } else {
            ""
        }
    )
}

pub fn drain_sse_line(buffer: &mut Vec<u8>) -> Option<AppResult<String>> {
    let index = buffer.iter().position(|byte| *byte == b'\n')?;
    let mut line = buffer.drain(..=index).collect::<Vec<_>>();
    line.pop();
    Some(decode_sse_line(&line))
}

pub fn decode_remaining_sse_line(buffer: &[u8]) -> AppResult<Option<String>> {
    if buffer.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(None);
    }
    decode_sse_line(buffer).map(Some)
}

fn decode_sse_line(line: &[u8]) -> AppResult<String> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    std::str::from_utf8(line)
        .map(str::to_string)
        .map_err(|error| AppError::Llm(format!("invalid SSE UTF-8: {error}")))
}

pub fn stream_response(
    response: Response,
    mut parser: Box<dyn StreamLineParser>,
    options: RequestOptions,
    context: StreamLogContext,
) -> ChatStream {
    let mut bytes = response.bytes_stream();
    let stream = async_stream::stream! {
        let mut buffer: Vec<u8> = Vec::new();
        let mut raw_lines = Vec::new();
        let mut raw_bytes = 0_usize;
        let mut raw_truncated = false;
        let mut first_token_ms = None;

        loop {
            let next = match tokio::time::timeout(options.timeout, bytes.next()).await {
                Ok(next) => next,
                Err(_) => {
                    let error = AppError::Llm(format!(
                        "LLM stream idle timeout after {}s",
                        options.timeout.as_secs()
                    ));
                    spawn_log(
                        &context.logger,
                        context.enabled,
                        &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
                        &context.model_id,
                        &context.provider_name,
                        &context.request_json,
                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                        Some(context.status),
                        Some(&error.to_string()),
                        stream_metrics(context.started, first_token_ms),
                    );
                    yield Err(error);
                    return;
                }
            };
            let Some(chunk) = next else {
                break;
            };
            match chunk {
                Ok(chunk) => {
                    buffer.extend_from_slice(&chunk);
                    while let Some(line) = drain_sse_line(&mut buffer) {
                        let line = match line {
                            Ok(line) => line,
                            Err(error) => {
                                spawn_log(
                                    &context.logger,
                                    context.enabled,
                                    &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
                                    &context.model_id,
                                    &context.provider_name,
                                    &context.request_json,
                                    Some(&format_stream_log(&raw_lines, raw_truncated)),
                                    Some(context.status),
                                    Some(&error.to_string()),
                                    stream_metrics(context.started, first_token_ms),
                                );
                                yield Err(error);
                                return;
                            }
                        };
                        capture_log_line(
                            &line,
                            &mut raw_lines,
                            &mut raw_bytes,
                            &mut raw_truncated,
                        );
                        for event in parser.parse_line(&line) {
                            match event {
                                Ok(event) => {
                                    if first_token_ms.is_none() && is_first_token_event(&event) {
                                        first_token_ms = Some(elapsed_ms(context.started));
                                    }
                                    yield Ok(event);
                                }
                                Err(error) => {
                                    spawn_log(
                                        &context.logger,
                                        context.enabled,
                                        &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
                                        &context.model_id,
                                        &context.provider_name,
                                        &context.request_json,
                                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                                        Some(context.status),
                                        Some(&error.to_string()),
                                        stream_metrics(context.started, first_token_ms),
                                    );
                                    yield Err(error);
                                    return;
                                }
                            }
                        }
                    }
                    if buffer.len() > MAX_SSE_LINE_BUFFER_BYTES {
                        yield Err(AppError::Llm(format!(
                            "SSE line exceeded {MAX_SSE_LINE_BUFFER_BYTES} bytes"
                        )));
                        return;
                    }
                }
                Err(error) => {
                    let error = AppError::Llm(error.to_string());
                    spawn_log(
                        &context.logger,
                        context.enabled,
                        &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
                        &context.model_id,
                        &context.provider_name,
                        &context.request_json,
                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                        Some(context.status),
                        Some(&error.to_string()),
                        stream_metrics(context.started, first_token_ms),
                    );
                    yield Err(error);
                    return;
                }
            }
        }
        if let Some(line) = match decode_remaining_sse_line(&buffer) {
            Ok(line) => line,
            Err(error) => {
                spawn_log(
                    &context.logger,
                    context.enabled,
                    &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
                    &context.model_id,
                    &context.provider_name,
                    &context.request_json,
                    Some(&format_stream_log(&raw_lines, raw_truncated)),
                    Some(context.status),
                    Some(&error.to_string()),
                    stream_metrics(context.started, first_token_ms),
                );
                yield Err(error);
                return;
            }
        } {
            capture_log_line(
                &line,
                &mut raw_lines,
                &mut raw_bytes,
                &mut raw_truncated,
            );
            for event in parser.parse_line(&line) {
                match event {
                    Ok(event) => {
                        if first_token_ms.is_none() && is_first_token_event(&event) {
                            first_token_ms = Some(elapsed_ms(context.started));
                        }
                        yield Ok(event);
                    }
                    Err(error) => {
                        spawn_log(
                            &context.logger,
                            context.enabled,
                            &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
                            &context.model_id,
                            &context.provider_name,
                            &context.request_json,
                            Some(&format_stream_log(&raw_lines, raw_truncated)),
                            Some(context.status),
                            Some(&error.to_string()),
                            stream_metrics(context.started, first_token_ms),
                        );
                        yield Err(error);
                        return;
                    }
                }
            }
        }
        spawn_log(
            &context.logger,
            context.enabled,
            &context.agent_name,
                        context.agent_id.as_deref(),
                        context.conversation_id.as_deref(),
            &context.model_id,
            &context.provider_name,
            &context.request_json,
            Some(&format_stream_log(&raw_lines, raw_truncated)),
            Some(context.status),
            None,
            stream_metrics(context.started, first_token_ms),
        );
    };
    Box::pin(stream)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn response_body_has_an_idle_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let response = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        let error = read_response_body_limited(response, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("idle timeout"));
        server.abort();
    }
}
