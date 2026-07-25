use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response};

use crate::{
    domain::TokenUsage,
    error::{AppError, AppResult},
    llm::{
        client::{ChatStream, RequestOptions},
        types::StreamEvent,
    },
    storage::{
        LlmLogger,
        llm_logs::{LlmLogMetrics, elapsed_ms},
    },
};

const MAX_LLM_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_LLM_LOG_BYTES: usize = 1024 * 1024;
/// Capture budget when payload logging is disabled: only the stream tail is
/// kept so usage metrics can still be extracted from the final SSE lines.
const METRICS_ONLY_LOG_BYTES: usize = 16 * 1024;
const MAX_SSE_LINE_BUFFER_BYTES: usize = 1024 * 1024;
pub(crate) const DEFAULT_MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_LOG_TRUNCATION_MARKER: &str = "[LLM stream log middle truncated]";

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
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_log(
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
        logger.begin_pending_write();
        let agent_name = agent_name.to_string();
        let agent_id = agent_id.map(str::to_string);
        let conversation_id = conversation_id.map(str::to_string);
        let model_id = model_id.to_string();
        let provider_name = provider_name.to_string();
        let raw_request = raw_request.to_string();
        let raw_response = raw_response.map(ToString::to_string);
        let error = error.map(ToString::to_string);
        tokio::spawn(async move {
            let result = logger
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
            logger.finish_pending_write(&result);
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

pub(crate) fn stream_metrics(
    started: Instant,
    first_token_ms: Option<u64>,
    usage: Option<&TokenUsage>,
) -> LlmLogMetrics {
    let mut metrics = LlmLogMetrics::timed(started).with_first_token(first_token_ms);
    if let Some(usage) =
        usage.filter(|usage| usage.total_tokens() > 0 || usage.total_prompt_tokens > 0)
    {
        metrics.input_tokens = Some(usage.input_tokens);
        metrics.output_tokens = Some(usage.output_tokens);
        metrics.cache_read_tokens = Some(usage.cache_read_tokens);
        metrics.cache_write_tokens = Some(usage.cache_write_tokens);
    }
    metrics
}

fn capture_log_line(
    line: &str,
    raw_lines: &mut Vec<String>,
    raw_bytes: &mut usize,
    raw_truncated: &mut bool,
    max_bytes: usize,
) {
    if !*raw_truncated && raw_bytes.saturating_add(line.len()) <= max_bytes {
        *raw_bytes += line.len();
        raw_lines.push(line.to_string());
        return;
    }

    let available = max_bytes.saturating_sub(STREAM_LOG_TRUNCATION_MARKER.len() + 2);
    let head_bytes = available / 2;
    let tail_bytes = available.saturating_sub(head_bytes);

    if *raw_truncated {
        // Amortized bounded tail: append and only trim the front once the tail
        // doubles its budget, so each retained byte moves at most once instead
        // of re-copying ~tail_bytes for every incoming SSE line.
        if raw_lines.len() < 2 {
            raw_lines.resize(2, String::new());
        }
        let tail = &mut raw_lines[1];
        if !tail.is_empty() {
            tail.push('\n');
        }
        tail.push_str(line);
        if tail.len() > tail_bytes.saturating_mul(2) {
            let cut = tail.len() - suffix_at_most(tail, tail_bytes).len();
            tail.drain(..cut);
        }
    } else {
        // Keep both ends of a long stream: the beginning is useful for context,
        // while the end usually contains usage, finish, and cache metadata.
        let existing = raw_lines.join("\n");
        let combined = if existing.is_empty() {
            line.to_string()
        } else {
            format!("{existing}\n{line}")
        };
        *raw_lines = vec![
            prefix_at_most(&combined, head_bytes).to_string(),
            suffix_at_most(&combined, tail_bytes).to_string(),
        ];
    }
    *raw_bytes = raw_lines.iter().map(String::len).sum();
    *raw_truncated = true;
}

fn prefix_at_most(value: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn suffix_at_most(value: &str, max_bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(max_bytes);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn format_stream_log(raw_lines: &[String], truncated: bool) -> String {
    if truncated {
        let head = raw_lines.first().map(String::as_str).unwrap_or_default();
        let tail = raw_lines.get(1).map(String::as_str).unwrap_or_default();
        format!("{head}\n{STREAM_LOG_TRUNCATION_MARKER}\n{tail}")
    } else {
        raw_lines.join("\n")
    }
}

fn drain_sse_line(buffer: &mut Vec<u8>) -> Option<AppResult<String>> {
    let index = buffer.iter().position(|byte| *byte == b'\n')?;
    let mut line = buffer.drain(..=index).collect::<Vec<_>>();
    line.pop();
    Some(decode_sse_line(&line))
}

fn decode_remaining_sse_line(buffer: &[u8]) -> AppResult<Option<String>> {
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

/// Persist one stream-log record (metrics always; payload when enabled).
fn log_stream_result(
    context: &StreamLogContext,
    raw_lines: &[String],
    raw_truncated: bool,
    first_token_ms: Option<u64>,
    usage: Option<&TokenUsage>,
    error: Option<&str>,
) {
    spawn_log(
        &context.logger,
        context.enabled,
        &context.agent_name,
        context.agent_id.as_deref(),
        context.conversation_id.as_deref(),
        &context.model_id,
        &context.provider_name,
        &context.request_json,
        Some(&format_stream_log(raw_lines, raw_truncated)),
        Some(context.status),
        error,
        stream_metrics(context.started, first_token_ms, usage),
    );
}

/// Mutable per-stream logging state. Writes exactly one log record: either via
/// `finish` on the normal completion / error paths, or — if the stream is
/// dropped before completing (consumer cancelled, connection dropped) — via
/// `Drop`, which records a cancellation entry with whatever was captured.
struct StreamLogState {
    context: StreamLogContext,
    capture_budget: usize,
    raw_lines: Vec<String>,
    raw_bytes: usize,
    raw_truncated: bool,
    first_token_ms: Option<u64>,
    usage: Option<TokenUsage>,
    completed: bool,
}

impl StreamLogState {
    fn new(context: StreamLogContext) -> Self {
        // Without payload logging only a small tail is kept so token usage can
        // still be recovered from the closing SSE lines.
        let capture_budget = if context.enabled {
            MAX_LLM_LOG_BYTES
        } else {
            METRICS_ONLY_LOG_BYTES
        };
        Self {
            context,
            capture_budget,
            raw_lines: Vec::new(),
            raw_bytes: 0,
            raw_truncated: false,
            first_token_ms: None,
            usage: None,
            completed: false,
        }
    }

    fn capture(&mut self, line: &str) {
        capture_log_line(
            line,
            &mut self.raw_lines,
            &mut self.raw_bytes,
            &mut self.raw_truncated,
            self.capture_budget,
        );
    }

    fn note_event(&mut self, event: &StreamEvent) {
        if let StreamEvent::Usage(value) = event {
            self.usage = Some(value.clone());
        }
        if self.first_token_ms.is_none() && is_first_token_event(event) {
            self.first_token_ms = Some(elapsed_ms(self.context.started));
        }
    }

    fn finish(&mut self, error: Option<&str>) {
        self.completed = true;
        log_stream_result(
            &self.context,
            &self.raw_lines,
            self.raw_truncated,
            self.first_token_ms,
            self.usage.as_ref(),
            error,
        );
    }
}

impl Drop for StreamLogState {
    fn drop(&mut self) {
        if !self.completed {
            self.finish(Some("stream cancelled or connection dropped"));
        }
    }
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
        let mut state = StreamLogState::new(context);

        loop {
            let next = match tokio::time::timeout(options.timeout, bytes.next()).await {
                Ok(next) => next,
                Err(_) => {
                    let error = AppError::Llm(format!(
                        "LLM stream idle timeout after {}s",
                        options.timeout.as_secs()
                    ));
                    state.finish(Some(&error.to_string()));
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
                                state.finish(Some(&error.to_string()));
                                yield Err(error);
                                return;
                            }
                        };
                        state.capture(&line);
                        for event in parser.parse_line(&line) {
                            match event {
                                Ok(event) => {
                                    state.note_event(&event);
                                    yield Ok(event);
                                }
                                Err(error) => {
                                    state.finish(Some(&error.to_string()));
                                    yield Err(error);
                                    return;
                                }
                            }
                        }
                    }
                    if buffer.len() > MAX_SSE_LINE_BUFFER_BYTES {
                        let error = AppError::Llm(format!(
                            "SSE line exceeded {MAX_SSE_LINE_BUFFER_BYTES} bytes"
                        ));
                        state.finish(Some(&error.to_string()));
                        yield Err(error);
                        return;
                    }
                }
                Err(error) => {
                    let error = AppError::Llm(error.to_string());
                    state.finish(Some(&error.to_string()));
                    yield Err(error);
                    return;
                }
            }
        }
        if let Some(line) = match decode_remaining_sse_line(&buffer) {
            Ok(line) => line,
            Err(error) => {
                state.finish(Some(&error.to_string()));
                yield Err(error);
                return;
            }
        } {
            state.capture(&line);
            for event in parser.parse_line(&line) {
                match event {
                    Ok(event) => {
                        state.note_event(&event);
                        yield Ok(event);
                    }
                    Err(error) => {
                        state.finish(Some(&error.to_string()));
                        yield Err(error);
                        return;
                    }
                }
            }
        }
        state.finish(None);
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

    #[test]
    fn stream_metrics_keep_usage_after_payload_capture_is_truncated() {
        let usage = TokenUsage {
            input_tokens: 120,
            output_tokens: 240,
            cache_read_tokens: 360,
            cache_write_tokens: 12,
            ..TokenUsage::default()
        };
        let metrics = stream_metrics(Instant::now(), Some(17), Some(&usage));
        assert_eq!(metrics.first_token_ms, Some(17));
        assert_eq!(metrics.input_tokens, Some(120));
        assert_eq!(metrics.output_tokens, Some(240));
        assert_eq!(metrics.cache_read_tokens, Some(360));
        assert_eq!(metrics.cache_write_tokens, Some(12));
    }

    #[test]
    fn stream_log_capture_keeps_the_head_and_latest_tail() {
        let mut lines = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        capture_log_line(
            "HEAD",
            &mut lines,
            &mut bytes,
            &mut truncated,
            MAX_LLM_LOG_BYTES,
        );
        capture_log_line(
            &"M".repeat(MAX_LLM_LOG_BYTES),
            &mut lines,
            &mut bytes,
            &mut truncated,
            MAX_LLM_LOG_BYTES,
        );
        capture_log_line(
            "TAIL",
            &mut lines,
            &mut bytes,
            &mut truncated,
            MAX_LLM_LOG_BYTES,
        );

        let formatted = format_stream_log(&lines, truncated);
        assert!(truncated);
        assert!(formatted.starts_with("HEAD"));
        assert!(formatted.contains("[LLM stream log middle truncated]"));
        assert!(formatted.ends_with("TAIL"));
    }
}
