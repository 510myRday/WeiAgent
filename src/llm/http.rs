use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response};

use crate::{
    error::{AppError, AppResult},
    llm::{
        client::{ChatStream, RequestOptions},
        types::StreamEvent,
    },
    storage::LlmLogger,
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

    pub async fn retry_send<F>(&self, options: RequestOptions, request: F) -> AppResult<Response>
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
                tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::Llm("request failed".to_string())))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_log(
        &self,
        enabled: bool,
        agent_name: &str,
        model_id: &str,
        provider_name: &str,
        raw_request: &str,
        raw_response: Option<&str>,
        raw_status: Option<u16>,
        error: Option<&str>,
    ) {
        spawn_log(
            &self.logger,
            enabled,
            agent_name,
            model_id,
            provider_name,
            raw_request,
            raw_response,
            raw_status,
            error,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_log(
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
        let raw_response = raw_response.map(ToString::to_string);
        let error = error.map(ToString::to_string);
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

pub async fn read_response_body_limited(response: Response) -> AppResult<String> {
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

pub trait StreamLineParser: Send {
    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>>;
}

pub struct StreamLogContext {
    pub logger: Option<Arc<LlmLogger>>,
    pub enabled: bool,
    pub agent_name: String,
    pub model_id: String,
    pub provider_name: String,
    pub request_json: String,
    pub status: u16,
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

pub fn stream_response(
    response: Response,
    mut parser: Box<dyn StreamLineParser>,
    options: RequestOptions,
    context: StreamLogContext,
) -> ChatStream {
    let mut bytes = response.bytes_stream();
    let stream = async_stream::stream! {
        let mut buffer = String::new();
        let mut raw_lines = Vec::new();
        let mut raw_bytes = 0_usize;
        let mut raw_truncated = false;

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
                        &context.model_id,
                        &context.provider_name,
                        &context.request_json,
                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                        Some(context.status),
                        Some(&error.to_string()),
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
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(index) = buffer.find('\n') {
                        let line = buffer[..index].trim_end_matches('\r').to_string();
                        buffer = buffer[index + 1..].to_string();
                        capture_log_line(
                            &line,
                            &mut raw_lines,
                            &mut raw_bytes,
                            &mut raw_truncated,
                        );
                        for event in parser.parse_line(&line) {
                            match event {
                                Ok(event) => yield Ok(event),
                                Err(error) => {
                                    spawn_log(
                                        &context.logger,
                                        context.enabled,
                                        &context.agent_name,
                                        &context.model_id,
                                        &context.provider_name,
                                        &context.request_json,
                                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                                        Some(context.status),
                                        Some(&error.to_string()),
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
                        &context.model_id,
                        &context.provider_name,
                        &context.request_json,
                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                        Some(context.status),
                        Some(&error.to_string()),
                    );
                    yield Err(error);
                    return;
                }
            }
        }
        if !buffer.trim().is_empty() {
            let line = buffer.trim_end_matches('\r').to_string();
            capture_log_line(
                &line,
                &mut raw_lines,
                &mut raw_bytes,
                &mut raw_truncated,
            );
            for event in parser.parse_line(&line) {
                yield event;
            }
        }
        spawn_log(
            &context.logger,
            context.enabled,
            &context.agent_name,
            &context.model_id,
            &context.provider_name,
            &context.request_json,
            Some(&format_stream_log(&raw_lines, raw_truncated)),
            Some(context.status),
            None,
        );
    };
    Box::pin(stream)
}
