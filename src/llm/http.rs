use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response, header::RETRY_AFTER};

use crate::{
    domain::TokenUsage,
    error::{AppError, AppResult},
    llm::{
        client::{ChatStream, RequestOptions},
        keys::ApiKeyCycle,
        types::StreamEvent,
    },
    storage::{
        LlmLogger,
        llm_logs::{LlmLogIdentity, LlmLogMetrics, LlmLogPayload, LlmLogRequest, elapsed_ms},
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
const RETRY_BASE_DELAY_MS: u64 = 1_000;
const RETRY_MAX_DELAY_MS: u64 = 30_000;
const RETRY_JITTER_PERCENT: u64 = 25;
const MAX_RETRY_AFTER_SECONDS: u64 = 30;

/// LLM traffic can be routed through a per-provider proxy. Clients are cached
/// by their normalized proxy URL so connection pools survive across requests.
const MAX_CACHED_PROXY_CLIENTS: usize = 32;

fn proxy_client_cache() -> &'static Mutex<HashMap<String, reqwest::Client>> {
    static CACHE: OnceLock<Mutex<HashMap<String, reqwest::Client>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

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

    /// Resolve the client for a provider: the shared direct client when no
    /// proxy is configured, otherwise a cached client bound to that proxy.
    pub fn client_for(&self, proxy_url: Option<&str>) -> AppResult<reqwest::Client> {
        let Some(proxy) = proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| crate::domain::ip_proxies::proxy_url_with_remote_dns(value).into_owned())
        else {
            return Ok(self.client.clone());
        };

        if let Ok(guard) = proxy_client_cache().lock()
            && let Some(client) = guard.get(&proxy)
        {
            return Ok(client.clone());
        }
        let proxy_config = reqwest::Proxy::all(&proxy)
            .map_err(|error| AppError::Llm(format!("failed to configure LLM proxy: {error}")))?;
        let client = reqwest::Client::builder()
            .proxy(proxy_config)
            .build()
            .map_err(|error| AppError::Llm(format!("failed to build LLM HTTP client: {error}")))?;
        if let Ok(mut guard) = proxy_client_cache().lock() {
            if guard.len() >= MAX_CACHED_PROXY_CLIENTS {
                guard.clear();
            }
            guard.insert(proxy, client.clone());
        }
        Ok(client)
    }

    /// Fetch provider-returned or user-supplied image URLs without following
    /// redirects. Each redirect target would otherwise need a fresh private-IP
    /// check before it could be requested.
    pub fn client_for_image_fetch(&self, proxy_url: Option<&str>) -> AppResult<reqwest::Client> {
        let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        if let Some(proxy) = proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| crate::domain::ip_proxies::proxy_url_with_remote_dns(value).into_owned())
        {
            let proxy_config = reqwest::Proxy::all(&proxy).map_err(|error| {
                AppError::Llm(format!("failed to configure LLM proxy: {error}"))
            })?;
            builder = builder.proxy(proxy_config);
        }
        builder
            .build()
            .map_err(|error| AppError::Llm(format!("failed to build image HTTP client: {error}")))
    }

    pub fn logger(&self) -> Option<Arc<LlmLogger>> {
        self.logger.clone()
    }

    pub fn log_enabled(&self) -> bool {
        self.logger
            .as_ref()
            .is_some_and(|logger| logger.is_enabled())
    }

    /// Send with retries. The configured retry count is the single budget for
    /// every failure class. When several keys are configured, each retry
    /// advances to the next key without increasing that budget.
    pub async fn retry_send<F>(
        &self,
        options: &RequestOptions,
        keys: &mut ApiKeyCycle,
        request: F,
    ) -> AppResult<Response>
    where
        F: Fn(&str) -> RequestBuilder,
    {
        let max_retries = options.retry_attempts.max(1);
        let max_requests = max_retries.saturating_add(1);
        let mut last_error = None;
        for request_number in 1..=max_requests {
            let mut retry_after = None;
            match self.send_once(options, keys, &request).await {
                Ok(response) => {
                    let status = response.status();
                    if !is_retryable_status(status)
                        && !(keys.has_alternatives() && is_rotatable_status(status))
                    {
                        return Ok(response);
                    }
                    let key = keys.current_key().to_string();
                    keys.note_failure();
                    if request_number >= max_requests {
                        // Hand the final response back untouched so callers
                        // keep their upstream status/body error reporting.
                        return Ok(response);
                    }
                    last_error = Some(AppError::Llm(with_key_label(
                        keys,
                        &key,
                        &format!("upstream returned {status}"),
                    )));
                    retry_after = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(parse_retry_after);
                }
                Err(error) => last_error = Some(error),
            }
            if request_number < max_requests {
                let reason = last_error
                    .as_ref()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "request failed".to_string());
                if let Some(on_retry) = options.on_retry.as_ref() {
                    on_retry(request_number, max_retries, &reason);
                }
                tokio::time::sleep(retry_delay(request_number, retry_after)).await;
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm("request failed".to_string())))
    }

    /// Send a request and consume its body within the same retry budget.
    /// This covers response-body idle timeouts, which happen after the HTTP
    /// headers have arrived and therefore cannot be retried by `retry_send`.
    pub async fn retry_send_read_body<F>(
        &self,
        options: &RequestOptions,
        keys: &mut ApiKeyCycle,
        request: F,
    ) -> AppResult<(reqwest::StatusCode, String)>
    where
        F: Fn(&str) -> RequestBuilder,
    {
        let max_retries = options.retry_attempts.max(1);
        let max_requests = max_retries.saturating_add(1);
        let mut last_error = None;
        for request_number in 1..=max_requests {
            let mut retry_after = None;
            match self.send_once(options, keys, &request).await {
                Ok(response) => {
                    let status = response.status();
                    retry_after = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(parse_retry_after);
                    match read_response_body_limited(response, options.timeout).await {
                        Ok(body) => {
                            if (!is_retryable_status(status)
                                && !(keys.has_alternatives() && is_rotatable_status(status)))
                                || request_number >= max_requests
                            {
                                return Ok((status, body));
                            }
                            last_error = Some(AppError::Llm(format!("upstream returned {status}")));
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                Err(error) => last_error = Some(error),
            }
            if request_number < max_requests {
                let reason = last_error
                    .as_ref()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "request failed".to_string());
                if let Some(on_retry) = options.on_retry.as_ref() {
                    on_retry(request_number, max_retries, &reason);
                }
                tokio::time::sleep(retry_delay(request_number, retry_after)).await;
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm("request failed".to_string())))
    }

    /// Execute exactly one HTTP request attempt. Stream consumers use this so
    /// an idle timeout while reading the response can start a fresh attempt
    /// without restarting the whole retry budget.
    pub async fn send_once<F>(
        &self,
        options: &RequestOptions,
        keys: &mut ApiKeyCycle,
        request: F,
    ) -> AppResult<Response>
    where
        F: Fn(&str) -> RequestBuilder,
    {
        let key = keys.next_key().to_string();
        match tokio::time::timeout(options.timeout, request(&key).send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                keys.note_failure();
                Err(AppError::Llm(with_key_label(
                    keys,
                    &key,
                    &error.to_string(),
                )))
            }
            Err(_) => {
                keys.note_failure();
                Err(AppError::Llm(with_key_label(
                    keys,
                    &key,
                    &format!("request timed out after {}s", options.timeout.as_secs()),
                )))
            }
        }
    }
}

/// These statuses generally indicate a transient upstream condition and are
/// retried even when the provider has only one configured key.
pub(crate) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || matches!(status.as_u16(), 408 | 409 | 425 | 429)
}

/// With several keys configured, explicit key rejections are worth retrying
/// on another key. Transient statuses are included for key failover too.
pub(crate) fn is_rotatable_status(status: reqwest::StatusCode) -> bool {
    is_retryable_status(status) || matches!(status.as_u16(), 401 | 402 | 403)
}

pub(crate) fn parse_retry_after(value: &reqwest::header::HeaderValue) -> Option<Duration> {
    value
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|seconds| Duration::from_secs(seconds.min(MAX_RETRY_AFTER_SECONDS)))
}

pub(crate) fn retry_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exponent = u64::from(attempt.saturating_sub(1).min(5));
    let base = RETRY_BASE_DELAY_MS.saturating_mul(1_u64 << exponent);
    let capped = base.min(RETRY_MAX_DELAY_MS);
    let random = uuid::Uuid::new_v4().as_u128() as u64;
    let jitter = 100 - RETRY_JITTER_PERCENT + random % (RETRY_JITTER_PERCENT * 2 + 1);
    let fallback = Duration::from_millis(capped.saturating_mul(jitter) / 100)
        .min(Duration::from_millis(RETRY_MAX_DELAY_MS));

    // A missing, zero, or very small Retry-After should not collapse the
    // backoff loop into a burst of requests. Longer server values still win.
    retry_after.map_or(fallback, |server_delay| {
        server_delay
            .max(fallback)
            .min(Duration::from_secs(MAX_RETRY_AFTER_SECONDS))
    })
}

/// Retry reasons name the key (masked) when the pool has several, so users can
/// tell which key failed from the chat retry bubble.
fn with_key_label(keys: &ApiKeyCycle, key: &str, reason: &str) -> String {
    if keys.has_alternatives() {
        format!("key {}: {reason}", crate::domain::tools::mask_api_key(key))
    } else {
        reason.to_string()
    }
}

pub(crate) fn spawn_log(logger: &Option<Arc<LlmLogger>>, request: LlmLogRequest) {
    if let Some(logger) = logger {
        let logger = logger.clone();
        logger.begin_pending_write();
        tokio::spawn(async move {
            let result = logger.write_log(request).await;
            logger.finish_pending_write(&result);
        });
    }
}

pub(crate) fn log_request(
    identity: LlmLogIdentity,
    raw_request: &str,
    raw_response: Option<&str>,
    raw_status: Option<u16>,
    error: Option<&str>,
    metrics: LlmLogMetrics,
    include_payload: bool,
) -> LlmLogRequest {
    LlmLogRequest {
        identity,
        payload: LlmLogPayload {
            raw_request: raw_request.to_string(),
            raw_response: raw_response.map(ToString::to_string),
            raw_status,
            error: error.map(ToString::to_string),
        },
        metrics,
        include_payload,
    }
}

pub async fn read_response_body_limited(
    response: Response,
    idle_timeout: Duration,
) -> AppResult<String> {
    let body = read_response_body_limited_to(
        response,
        idle_timeout,
        MAX_LLM_RESPONSE_BYTES,
        "LLM response",
    )
    .await?;
    String::from_utf8(body)
        .map_err(|error| AppError::Llm(format!("LLM response was not valid UTF-8: {error}")))
}

pub async fn read_response_body_limited_to(
    response: Response,
    idle_timeout: Duration,
    max_bytes: usize,
    label: &str,
) -> AppResult<Vec<u8>> {
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
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(AppError::Llm(format!("{label} exceeded {max_bytes} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub trait StreamLineParser: Send {
    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>>;
}

#[derive(Clone)]
pub struct StreamLogContext {
    pub logger: Option<Arc<LlmLogger>>,
    pub enabled: bool,
    pub agent_name: String,
    pub agent_id: Option<String>,
    pub conversation_id: Option<String>,
    pub is_compaction: bool,
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

fn redact_stream_log_line(line: &str) -> String {
    let (prefix, payload) = if let Some(payload) = line.strip_prefix("data: ") {
        ("data: ", payload)
    } else if let Some(payload) = line.strip_prefix("data:") {
        ("data:", payload)
    } else {
        return line.to_string();
    };
    if !payload.contains("\"encrypted_content\"") {
        return line.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return line.to_string();
    };
    redact_encrypted_content(&mut value);
    format!("{prefix}{value}")
}

fn redact_encrypted_content(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "encrypted_content" && value.as_str().is_some() {
                    let length = value.as_str().map_or(0, str::len);
                    *value = serde_json::Value::String(format!(
                        "[encrypted reasoning omitted: {length} chars]"
                    ));
                } else {
                    redact_encrypted_content(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_encrypted_content(value);
            }
        }
        _ => {}
    }
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
        log_request(
            LlmLogIdentity {
                agent_name: context.agent_name.clone(),
                agent_id: context.agent_id.clone(),
                conversation_id: context.conversation_id.clone(),
                is_compaction: context.is_compaction,
                model_id: context.model_id.clone(),
                provider_name: context.provider_name.clone(),
            },
            &context.request_json,
            Some(&format_stream_log(raw_lines, raw_truncated)),
            Some(context.status),
            error,
            stream_metrics(context.started, first_token_ms, usage),
            context.enabled,
        ),
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
        let line = redact_stream_log_line(line);
        capture_log_line(
            &line,
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

    #[test]
    fn stream_logs_redact_encrypted_reasoning() {
        let line = r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","encrypted_content":"opaque-state"}}"#;
        let redacted = redact_stream_log_line(line);
        assert!(!redacted.contains("opaque-state"));
        assert!(redacted.contains("encrypted reasoning omitted: 12 chars"));

        let no_space = redact_stream_log_line(&line.replacen("data: ", "data:", 1));
        assert!(!no_space.contains("opaque-state"));
    }

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

    fn cycle(keys: &[&str]) -> ApiKeyCycle {
        ApiKeyCycle::from_parts(
            uuid::Uuid::new_v4(),
            &keys.iter().map(ToString::to_string).collect::<Vec<_>>(),
            crate::domain::ApiKeyStrategy::Failover,
            None,
        )
        .unwrap()
    }

    fn retry_test_options(retries: u32, reasons: Arc<Mutex<Vec<String>>>) -> RequestOptions {
        RequestOptions {
            timeout: Duration::from_secs(5),
            retry_attempts: retries,
            thinking_level: None,
            on_retry: Some(Arc::new(move |_, _, reason: &str| {
                if let Ok(mut guard) = reasons.lock() {
                    guard.push(reason.to_string());
                }
            })),
            is_compaction: false,
        }
    }

    #[test]
    fn retry_delay_uses_exponential_backoff_with_bounded_jitter() {
        let first = retry_delay(1, None).as_millis();
        let second = retry_delay(2, None).as_millis();
        let third = retry_delay(3, None).as_millis();
        let capped = retry_delay(20, None).as_millis();

        assert!((750..=1_250).contains(&first), "first delay: {first}ms");
        assert!(
            (1_500..=2_500).contains(&second),
            "second delay: {second}ms"
        );
        assert!((3_000..=5_000).contains(&third), "third delay: {third}ms");
        assert!(
            (22_500..=30_000).contains(&capped),
            "capped delay: {capped}ms"
        );
        assert_eq!(
            retry_delay(3, Some(Duration::from_secs(12))),
            Duration::from_secs(12)
        );
        assert!(retry_delay(1, Some(Duration::ZERO)) >= Duration::from_millis(750));
    }

    /// Answer one request: 200 when it carries the good bearer key, else 401.
    async fn serve_key_check(
        listener: &tokio::net::TcpListener,
        good_key: &str,
        hits: Option<&std::sync::atomic::AtomicUsize>,
    ) {
        let (mut socket, _) = listener.accept().await.unwrap();
        // Count on accept so the total is visible before the client ever
        // sees the response bytes.
        if let Some(hits) = hits {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let mut request = [0_u8; 2048];
        let read = socket.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]).to_lowercase();
        let response: &[u8] = if request.contains(&format!("bearer {good_key}")) {
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
        } else {
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad"
        };
        socket.write_all(response).await.unwrap();
        socket.flush().await.unwrap();
    }

    #[tokio::test]
    async fn retry_send_rotates_to_the_next_key_on_rejection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            serve_key_check(&listener, "good-key", None).await;
            serve_key_check(&listener, "good-key", None).await;
        });

        let reasons = Arc::new(Mutex::new(Vec::new()));
        let options = retry_test_options(2, reasons.clone());
        let mut keys = cycle(&["dead-key", "good-key"]);
        let client = reqwest::Client::new();
        let response = LlmHttp::new()
            .retry_send(&options, &mut keys, |key| {
                client.post(format!("http://{address}")).bearer_auth(key)
            })
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let reasons = reasons.lock().unwrap();
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("****-key"), "{}", reasons[0]);
        assert!(reasons[0].contains("401"), "{}", reasons[0]);
        server.abort();
    }

    #[tokio::test]
    async fn retry_send_returns_bad_statuses_untouched_for_single_key_pools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_hits = hits.clone();
        let server = tokio::spawn(async move {
            loop {
                serve_key_check(&listener, "another-key", Some(&server_hits)).await;
            }
        });

        let reasons = Arc::new(Mutex::new(Vec::new()));
        let options = retry_test_options(3, reasons.clone());
        let mut keys = cycle(&["only-key"]);
        let client = reqwest::Client::new();
        let response = LlmHttp::new()
            .retry_send(&options, &mut keys, |key| {
                client.post(format!("http://{address}")).bearer_auth(key)
            })
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(response.text().await.unwrap(), "bad");
        assert!(reasons.lock().unwrap().is_empty());
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn retry_send_respects_the_configured_budget_with_key_pools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_hits = hits.clone();
        let server = tokio::spawn(async move {
            loop {
                serve_key_check(&listener, "no-such-key", Some(&server_hits)).await;
            }
        });

        let reasons = Arc::new(Mutex::new(Vec::new()));
        let options = retry_test_options(3, reasons.clone());
        let mut keys = cycle(&["k-one", "k-two", "k-three"]);
        let client = reqwest::Client::new();
        let response = LlmHttp::new()
            .retry_send(&options, &mut keys, |key| {
                client.post(format!("http://{address}")).bearer_auth(key)
            })
            .await
            .unwrap();
        // The final rejection is handed back untouched so callers keep their
        // own "upstream returned {status}: {body}" reporting path.
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert_eq!(reasons.lock().unwrap().len(), 3);
        server.abort();
    }

    #[tokio::test]
    async fn retry_send_retries_transient_server_errors_with_one_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_hits = hits.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 2048];
                let _ = socket.read(&mut request).await.unwrap();
                let response: &[u8] = if server_hits
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 0
                {
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 7\r\nConnection: close\r\n\r\ntry-again"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                };
                socket.write_all(response).await.unwrap();
                socket.flush().await.unwrap();
            }
        });

        let reasons = Arc::new(Mutex::new(Vec::new()));
        let options = retry_test_options(2, reasons.clone());
        let mut keys = cycle(&["only-key"]);
        let client = reqwest::Client::new();
        let response = LlmHttp::new()
            .retry_send(&options, &mut keys, |key| {
                client.post(format!("http://{address}")).bearer_auth(key)
            })
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        let reasons = reasons.lock().unwrap();
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("503"), "{}", reasons[0]);
        server.abort();
    }

    #[tokio::test]
    async fn retry_send_read_body_retries_after_response_body_idle_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_hits = hits.clone();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut request = [0_u8; 2048];
                let _ = socket.read(&mut request).await.unwrap();
                if index == 0 {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                        .await
                        .unwrap();
                    socket.flush().await.unwrap();
                    tokio::time::sleep(Duration::from_millis(80)).await;
                } else {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .await
                        .unwrap();
                    socket.flush().await.unwrap();
                }
            }
        });

        let reasons = Arc::new(Mutex::new(Vec::new()));
        let mut options = retry_test_options(2, reasons.clone());
        options.timeout = Duration::from_millis(20);
        let mut keys = cycle(&["only-key"]);
        let client = reqwest::Client::new();
        let result = LlmHttp::new()
            .retry_send_read_body(&options, &mut keys, |key| {
                client.post(format!("http://{address}")).bearer_auth(key)
            })
            .await
            .unwrap();

        assert_eq!(result.0, reqwest::StatusCode::OK);
        assert_eq!(result.1, "ok");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(reasons.lock().unwrap().len(), 1);
        server.abort();
    }
}
