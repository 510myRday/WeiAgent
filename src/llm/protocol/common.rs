use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::RequestBuilder;
use serde_json::{Map, Value};

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        http::{
            DEFAULT_MODEL_LIST_TIMEOUT, LlmHttp, StreamLineParser, StreamLogContext,
            is_retryable_status, is_rotatable_status, log_request, parse_retry_after,
            read_response_body_limited, retry_delay, spawn_log, stream_response,
        },
        keys::ApiKeyCycle,
        types::{ChatCompletionOutput, ToolCall, ToolDefinition, normalize_tool_call_arguments},
    },
    storage::{
        LlmLogger,
        llm_logs::{LlmLogIdentity, LlmLogMetrics},
    },
};

pub(crate) trait ProtocolAdapter: Clone + Send + Sync + 'static {
    fn completion_url(&self, provider: &ModelProvider, agent: &Agent, stream: bool) -> String;

    fn models_url(&self, provider: &ModelProvider) -> String {
        format!("{}/models", provider.base_url.trim_end_matches('/'))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_request(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        stream: bool,
        system_prompt: &str,
        extra_body: &Value,
        tools: Option<Vec<ToolDefinition>>,
        thinking_level: Option<crate::domain::ThinkingLevel>,
    ) -> AppResult<Value>;

    fn authenticated_post(
        &self,
        http: &reqwest::Client,
        url: &str,
        api_key: &str,
        body: &Value,
        provider: &ModelProvider,
    ) -> RequestBuilder;

    fn authenticated_get(&self, http: &reqwest::Client, url: &str, api_key: &str)
    -> RequestBuilder;

    fn parse_response(&self, value: &Value) -> AppResult<ChatCompletionOutput>;

    fn stream_parser(&self) -> Box<dyn StreamLineParser>;

    fn append_models(&self, value: &Value, models: &mut Vec<String>);

    fn next_models_url(
        &self,
        _provider: &ModelProvider,
        _value: &Value,
        _pages: u8,
    ) -> AppResult<Option<String>> {
        Ok(None)
    }
}

#[derive(Clone)]
pub(crate) struct ProtocolClient<A> {
    adapter: A,
    http: LlmHttp,
}

impl<A: ProtocolAdapter> ProtocolClient<A> {
    pub(crate) fn new(adapter: A) -> Self {
        Self {
            adapter,
            http: LlmHttp::new(),
        }
    }

    pub(crate) fn with_logger(mut self, logger: Arc<LlmLogger>) -> Self {
        self.http = self.http.with_logger(logger);
        self
    }
}

fn request_log_identity(
    agent: &Agent,
    provider: &ModelProvider,
    conversation_id: Option<String>,
    is_compaction: bool,
) -> LlmLogIdentity {
    LlmLogIdentity {
        agent_name: agent.name.clone(),
        agent_id: Some(agent.id.to_string()),
        conversation_id,
        is_compaction,
        model_id: agent.model_id.clone(),
        provider_name: provider.name.clone(),
    }
}

fn notify_retry(options: &RequestOptions, retry_number: u32, max_retries: u32, error: &AppError) {
    if let Some(on_retry) = options.on_retry.as_ref() {
        on_retry(retry_number, max_retries, &error.to_string());
    }
}

fn is_retryable_stream_error(error: &AppError) -> bool {
    matches!(error, AppError::Llm(message) if {
        let message = message.to_ascii_lowercase();
        message.contains("idle timeout")
            || message.contains("error decoding response body")
            || message.contains("connection reset")
            || message.contains("connection closed")
            || message.contains("broken pipe")
            || message.contains("incomplete message")
            || message.contains("unexpected eof")
    })
}

fn log_stream_open_error(context: &StreamLogContext, error: &AppError) {
    spawn_log(
        &context.logger,
        log_request(
            context.identity(),
            &context.request_json,
            None,
            None,
            Some(&error.to_string()),
            LlmLogMetrics::timed(context.started),
            context.enabled,
        ),
    );
}

fn log_stream_status_error(context: &StreamLogContext, body: &str, status: u16, error: &AppError) {
    spawn_log(
        &context.logger,
        log_request(
            context.identity(),
            &context.request_json,
            Some(body),
            Some(status),
            Some(&error.to_string()),
            LlmLogMetrics::timed(context.started),
            context.enabled,
        ),
    );
}

#[async_trait]
impl<A: ProtocolAdapter> ChatClient for ProtocolClient<A> {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatCompletionOutput> {
        let request = self.adapter.build_request(
            agent,
            provider,
            messages,
            false,
            system_prompt,
            &provider.extra_body,
            tools,
            options.thinking_level,
        )?;
        let conversation_id = messages
            .last()
            .map(|message| message.conversation_id.to_string());
        // Keep the same upstream key for a conversation so provider-side
        // prompt caches stay warm across multi-turn / tool-loop requests.
        let mut api_keys = ApiKeyCycle::new(provider)?.with_affinity(conversation_id.as_deref());
        let url = self.adapter.completion_url(provider, agent, false);
        let client = self.http.client_for(provider.proxy_url.as_deref())?;
        let log_enabled = self.http.log_enabled();
        // Redacting + pretty-printing the full request (which can embed large
        // base64 images) is only worth it when payload logging is on.
        let request_json = if log_enabled {
            request_json_for_log(&request)
        } else {
            String::new()
        };
        let started = Instant::now();

        let (status, body) = match self
            .http
            .retry_send_read_body(&options, &mut api_keys, |api_key| {
                self.adapter
                    .authenticated_post(&client, &url, api_key, &request, provider)
            })
            .await
        {
            Ok(result) => result,
            Err(error) => {
                spawn_log(
                    &self.http.logger(),
                    log_request(
                        request_log_identity(
                            agent,
                            provider,
                            conversation_id.clone(),
                            options.is_compaction,
                        ),
                        &request_json,
                        None,
                        None,
                        Some(&error.to_string()),
                        LlmLogMetrics::timed(started),
                        log_enabled,
                    ),
                );
                return Err(error);
            }
        };

        if !status.is_success() {
            let error = format!("upstream returned {status}: {body}");
            spawn_log(
                &self.http.logger(),
                log_request(
                    request_log_identity(
                        agent,
                        provider,
                        conversation_id.clone(),
                        options.is_compaction,
                    ),
                    &request_json,
                    Some(&body),
                    Some(status.as_u16()),
                    Some(&error),
                    LlmLogMetrics::timed(started),
                    log_enabled,
                ),
            );
            return Err(AppError::Llm(error));
        }

        let parsed = serde_json::from_str::<Value>(&body)
            .map_err(|error| AppError::Llm(format!("invalid upstream JSON: {error}")))
            .and_then(|value| self.adapter.parse_response(&value));
        match parsed {
            Ok(output)
                if !output.content.is_empty()
                    || !output.reasoning_content.is_empty()
                    || !output.tool_calls.is_empty() =>
            {
                spawn_log(
                    &self.http.logger(),
                    log_request(
                        request_log_identity(
                            agent,
                            provider,
                            conversation_id.clone(),
                            options.is_compaction,
                        ),
                        &request_json,
                        Some(&body),
                        Some(status.as_u16()),
                        None,
                        LlmLogMetrics::timed(started),
                        log_enabled,
                    ),
                );
                Ok(output)
            }
            Ok(_) => {
                let error = AppError::Llm("empty model response".to_string());
                spawn_log(
                    &self.http.logger(),
                    log_request(
                        request_log_identity(
                            agent,
                            provider,
                            conversation_id.clone(),
                            options.is_compaction,
                        ),
                        &request_json,
                        Some(&body),
                        Some(status.as_u16()),
                        Some(&error.to_string()),
                        LlmLogMetrics::timed(started),
                        log_enabled,
                    ),
                );
                Err(error)
            }
            Err(error) => {
                spawn_log(
                    &self.http.logger(),
                    log_request(
                        request_log_identity(
                            agent,
                            provider,
                            conversation_id.clone(),
                            options.is_compaction,
                        ),
                        &request_json,
                        Some(&body),
                        Some(status.as_u16()),
                        Some(&error.to_string()),
                        LlmLogMetrics::timed(started),
                        log_enabled,
                    ),
                );
                Err(error)
            }
        }
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
        let request = self.adapter.build_request(
            agent,
            provider,
            messages,
            true,
            system_prompt,
            &provider.extra_body,
            tools,
            options.thinking_level,
        )?;
        let conversation_id = messages
            .last()
            .map(|message| message.conversation_id.to_string());
        let api_keys = ApiKeyCycle::new(provider)?.with_affinity(conversation_id.as_deref());
        let url = self.adapter.completion_url(provider, agent, true);
        let client = self.http.client_for(provider.proxy_url.as_deref())?;
        let log_enabled = self.http.log_enabled();
        let request_json = if log_enabled {
            request_json_for_log(&request)
        } else {
            String::new()
        };
        let started = Instant::now();
        let agent_id = agent.id.to_string();

        let max_retries = options.retry_attempts.max(1);
        let max_requests = max_retries.saturating_add(1);
        let adapter = self.adapter.clone();
        let http = self.http.clone();
        let provider = provider.clone();
        let agent_name = agent.name.clone();
        let model_id = agent.model_id.clone();
        let request_json = request_json.clone();
        let logger = self.http.logger();
        let stream_options = options.clone();
        let stream_context = StreamLogContext {
            logger,
            enabled: log_enabled,
            agent_name,
            agent_id: Some(agent_id),
            conversation_id,
            is_compaction: options.is_compaction,
            model_id,
            provider_name: provider.name.clone(),
            request_json,
            status: 200,
            started,
        };

        let stream = async_stream::stream! {
            let mut api_keys = api_keys;
            for request_number in 1..=max_requests {
                let response = match http.send_once(&stream_options, &mut api_keys, |api_key| {
                    adapter.authenticated_post(&client, &url, api_key, &request, &provider)
                }).await {
                    Ok(response) => response,
                    Err(error) => {
                        if request_number < max_requests {
                            notify_retry(&stream_options, request_number, max_retries, &error);
                            tokio::time::sleep(retry_delay(request_number, None)).await;
                            continue;
                        }
                        log_stream_open_error(&stream_context, &error);
                        yield Err(error);
                        return;
                    }
                };

                let status = response.status();
                if !status.is_success() {
                    let retry_after = response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(parse_retry_after);
                    let body = read_response_body_limited(response, stream_options.timeout)
                        .await
                        .unwrap_or_else(|_| "failed to read error body".to_string());
                    let error = AppError::Llm(format!("upstream returned {status}: {body}"));
                    if request_number < max_requests
                        && (is_retryable_status(status)
                            || (api_keys.has_alternatives() && is_rotatable_status(status)))
                    {
                        api_keys.note_failure();
                        notify_retry(&stream_options, request_number, max_retries, &error);
                        tokio::time::sleep(retry_delay(request_number, retry_after)).await;
                        continue;
                    }
                    log_stream_status_error(&stream_context, &body, status.as_u16(), &error);
                    yield Err(error);
                    return;
                }

                let mut upstream = stream_response(
                    response,
                    adapter.stream_parser(),
                    stream_options.clone(),
                    StreamLogContext { status: status.as_u16(), ..stream_context.clone() },
                );
                let mut saw_event = false;
                let mut retry_stream = false;
                while let Some(event) = upstream.next().await {
                    match event {
                        Ok(event) => {
                            saw_event = true;
                            yield Ok(event);
                        }
                        Err(error) => {
                            if !saw_event && request_number < max_requests && is_retryable_stream_error(&error) {
                                notify_retry(&stream_options, request_number, max_retries, &error);
                                tokio::time::sleep(retry_delay(request_number, None)).await;
                                retry_stream = true;
                            } else {
                                yield Err(error);
                            }
                            break;
                        }
                    }
                }
                if retry_stream {
                    continue;
                }
                return;
            }
        };
        Ok(Box::pin(stream))
    }

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>> {
        let api_key = ApiKeyCycle::new(provider)?.next_key().to_string();
        let client = self.http.client_for(provider.proxy_url.as_deref())?;
        let mut url = self.adapter.models_url(provider);
        let mut models = Vec::new();
        let mut pages = 0_u8;

        loop {
            pages += 1;
            let response = tokio::time::timeout(
                DEFAULT_MODEL_LIST_TIMEOUT,
                self.adapter
                    .authenticated_get(&client, &url, &api_key)
                    .send(),
            )
            .await
            .map_err(|_| AppError::Llm("model list request timed out after 30s".to_string()))?
            .map_err(|error| AppError::Llm(error.to_string()))?;
            let status = response.status();
            let body = read_response_body_limited(response, DEFAULT_MODEL_LIST_TIMEOUT).await?;
            if !status.is_success() {
                return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|error| AppError::Llm(format!("invalid model list JSON: {error}")))?;
            self.adapter.append_models(&value, &mut models);

            let Some(next_url) = self.adapter.next_models_url(provider, &value, pages)? else {
                break;
            };
            url = next_url;
        }

        models.sort();
        models.dedup();
        Ok(models)
    }
}

/// One SSE `data:` line, decoded far enough for a protocol stream parser.
pub(crate) enum SseData {
    /// Terminal `[DONE]` sentinel.
    Done,
    /// Not a data line, or an empty payload — nothing to parse.
    Skip,
    Json(Value),
}

pub(crate) fn parse_sse_data_line(line: &str) -> AppResult<SseData> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(SseData::Skip);
    };
    let data = data.strip_prefix(' ').unwrap_or(data).trim();
    if data.is_empty() {
        return Ok(SseData::Skip);
    }
    if data == "[DONE]" {
        return Ok(SseData::Done);
    }
    serde_json::from_str(data)
        .map(SseData::Json)
        .map_err(|error| AppError::Llm(format!("invalid SSE JSON: {error}")))
}

pub(crate) fn message_tool_calls(message: &ChatMessage) -> AppResult<Vec<ToolCall>> {
    message
        .tool_calls
        .as_ref()
        .map(|calls| {
            serde_json::from_value::<Vec<ToolCall>>(calls.clone())
                .map(|mut calls| {
                    normalize_tool_call_arguments(&mut calls);
                    calls
                })
                .map_err(|error| AppError::Llm(format!("stored tool calls are invalid: {error}")))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(crate) fn wrap_provider_response_item(provider: &str, item: Value) -> Value {
    serde_json::json!({
        "_weiagent_provider": provider,
        "item": item,
    })
}

pub(crate) fn unwrap_provider_response_item<'a>(
    value: &'a Value,
    provider: &str,
) -> Option<&'a Value> {
    (value["_weiagent_provider"].as_str() == Some(provider)).then(|| &value["item"])
}

pub(crate) fn parse_json_object(value: &str, label: &str) -> AppResult<Value> {
    match serde_json::from_str::<Value>(value) {
        Ok(Value::Object(map)) => Ok(Value::Object(map)),
        Ok(_) => Err(AppError::Llm(format!("{label} must be a JSON object"))),
        Err(error) => Err(AppError::Llm(format!("{label} are invalid: {error}"))),
    }
}

pub(crate) fn validate_tool_calls(tool_calls: Vec<ToolCall>) -> AppResult<Vec<ToolCall>> {
    if let Some(call) = tool_calls
        .iter()
        .find(|call| call.id.is_empty() || call.function.name.is_empty())
    {
        return Err(AppError::Llm(format!(
            "upstream returned an invalid tool call: id={:?}, name={:?}",
            call.id, call.function.name
        )));
    }
    Ok(tool_calls)
}

pub(crate) fn append_role_items(
    items: &mut Vec<Value>,
    role: &str,
    field: &str,
    values: Vec<Value>,
) {
    if let Some(last) = items.last_mut()
        && last["role"] == role
        && let Some(existing) = last[field].as_array_mut()
    {
        existing.extend(values);
        return;
    }
    items.push(serde_json::json!({"role": role, field: values}));
}

pub(crate) fn merge_extra_body(
    mut body: Map<String, Value>,
    extra_body: &Value,
    reserved: &[&str],
) -> AppResult<Value> {
    match extra_body {
        Value::Null => {}
        Value::Object(extra) => {
            for (key, value) in extra {
                if !reserved.contains(&key.as_str()) {
                    body.insert(key.clone(), value.clone());
                }
            }
        }
        _ => {
            return Err(AppError::BadRequest(
                "extra_body must be a JSON object".to_string(),
            ));
        }
    }
    Ok(Value::Object(body))
}

pub(crate) fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn add_query_parameter(base: &str, key: &str, value: &str) -> AppResult<String> {
    let mut url = reqwest::Url::parse(base).map_err(|error| AppError::Llm(error.to_string()))?;
    url.query_pairs_mut().append_pair(key, value);
    Ok(url.to_string())
}

pub(crate) fn default_models(value: &Value, models: &mut Vec<String>) {
    models.extend(
        value["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["id"].as_str())
            .filter(|id| !id.trim().is_empty())
            .map(ToString::to_string),
    );
}

pub(crate) fn bearer_post(
    http: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> RequestBuilder {
    let request = http.post(url).json(body);
    if api_key.trim().is_empty() {
        request
    } else {
        request.bearer_auth(api_key)
    }
}

pub(crate) fn bearer_get(http: &reqwest::Client, url: &str, api_key: &str) -> RequestBuilder {
    let request = http.get(url);
    if api_key.trim().is_empty() {
        request
    } else {
        request.bearer_auth(api_key)
    }
}

pub(crate) fn request_json_for_log(request: &Value) -> String {
    let mut redacted = request.clone();
    redact_log_payloads(&mut redacted);
    serde_json::to_string_pretty(&redacted).unwrap_or_else(|_| redacted.to_string())
}

fn redact_log_payloads(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "encrypted_content" && value.as_str().is_some() {
                    let length = value.as_str().map_or(0, str::len);
                    *value =
                        Value::String(format!("[encrypted reasoning omitted: {length} chars]"));
                } else if key == "data" && value.as_str().is_some_and(|data| data.len() > 256) {
                    let length = value.as_str().map_or(0, str::len);
                    *value = Value::String(format!("[base64 image omitted: {length} chars]"));
                } else {
                    redact_log_payloads(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_log_payloads(value);
            }
        }
        Value::String(text) if text.starts_with("data:image/") && text.len() > 256 => {
            let length = text.len();
            *text = format!("[data URL image omitted: {length} chars]");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_logs_redact_encrypted_reasoning() {
        let logged = request_json_for_log(&serde_json::json!({
            "input": [{
                "type": "reasoning",
                "encrypted_content": "opaque-state"
            }]
        }));
        assert!(!logged.contains("opaque-state"));
        assert!(logged.contains("encrypted reasoning omitted: 12 chars"));
    }
}
