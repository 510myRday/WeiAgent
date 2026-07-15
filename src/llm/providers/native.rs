use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    domain::{Agent, ChatMessage, ChatRole, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        types::{
            ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition,
            select_context_messages,
        },
    },
    storage::LlmLogger,
};

const MAX_LLM_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_LLM_LOG_BYTES: usize = 1024 * 1024;
const MAX_SSE_LINE_BUFFER_BYTES: usize = 1024 * 1024;
const DEFAULT_MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(30);
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeProtocol {
    OpenAiResponses,
    AnthropicMessages,
    GoogleGemini,
}

#[derive(Clone)]
pub struct NativeChatClient {
    protocol: NativeProtocol,
    http: reqwest::Client,
    llm_logger: Option<Arc<LlmLogger>>,
}

impl NativeChatClient {
    pub fn new(protocol: NativeProtocol) -> Self {
        Self {
            protocol,
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

    fn completion_url(&self, provider: &ModelProvider, agent: &Agent, stream: bool) -> String {
        let base = provider.base_url.trim_end_matches('/');
        match self.protocol {
            NativeProtocol::OpenAiResponses => format!("{base}/responses"),
            NativeProtocol::AnthropicMessages => format!("{base}/messages"),
            NativeProtocol::GoogleGemini if stream => {
                format!(
                    "{base}/models/{}:streamGenerateContent?alt=sse",
                    encode_path_segment(&agent.model_id)
                )
            }
            NativeProtocol::GoogleGemini => {
                format!(
                    "{base}/models/{}:generateContent",
                    encode_path_segment(&agent.model_id)
                )
            }
        }
    }

    fn models_url(&self, provider: &ModelProvider) -> String {
        format!("{}/models", provider.base_url.trim_end_matches('/'))
    }

    fn authenticated_post(&self, url: &str, api_key: &str, body: &Value) -> RequestBuilder {
        let request = self.http.post(url).json(body);
        match self.protocol {
            NativeProtocol::OpenAiResponses => request.bearer_auth(api_key),
            NativeProtocol::AnthropicMessages => request
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            NativeProtocol::GoogleGemini => request.header("x-goog-api-key", api_key),
        }
    }

    fn authenticated_get(&self, url: &str, api_key: &str) -> RequestBuilder {
        let request = self.http.get(url);
        match self.protocol {
            NativeProtocol::OpenAiResponses => request.bearer_auth(api_key),
            NativeProtocol::AnthropicMessages => request
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            NativeProtocol::GoogleGemini => request.header("x-goog-api-key", api_key),
        }
    }

    async fn retry_send<F>(&self, options: RequestOptions, request: F) -> AppResult<Response>
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
}

#[async_trait]
impl ChatClient for NativeChatClient {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatCompletionOutput> {
        let request = build_request(
            self.protocol,
            agent,
            messages,
            false,
            system_prompt,
            &provider.extra_body,
            tools,
        )?;
        let request_json =
            serde_json::to_string_pretty(&request).unwrap_or_else(|_| request.to_string());
        let api_key = self.api_key(provider)?.to_string();
        let url = self.completion_url(provider, agent, false);
        let log_enabled = self
            .llm_logger
            .as_ref()
            .is_some_and(|logger| logger.is_enabled());

        let response = match self
            .retry_send(options, || {
                self.authenticated_post(&url, &api_key, &request)
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
        let body = read_response_body_limited(response).await?;
        if !status.is_success() {
            let error = format!("upstream returned {status}: {body}");
            Self::spawn_log(
                &self.llm_logger,
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&error),
            );
            return Err(AppError::Llm(error));
        }

        let parsed = serde_json::from_str::<Value>(&body)
            .map_err(|error| AppError::Llm(format!("invalid upstream JSON: {error}")))
            .and_then(|value| parse_response(self.protocol, &value));
        match parsed {
            Ok(output) if !output.content.is_empty() || !output.tool_calls.is_empty() => {
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
                Ok(output)
            }
            Ok(_) => {
                let error = AppError::Llm("empty model response".to_string());
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
                Err(error)
            }
            Err(error) => {
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
        let request = build_request(
            self.protocol,
            agent,
            messages,
            true,
            system_prompt,
            &provider.extra_body,
            tools,
        )?;
        let request_json =
            serde_json::to_string_pretty(&request).unwrap_or_else(|_| request.to_string());
        let api_key = self.api_key(provider)?.to_string();
        let url = self.completion_url(provider, agent, true);
        let log_enabled = self
            .llm_logger
            .as_ref()
            .is_some_and(|logger| logger.is_enabled());
        let response = match self
            .retry_send(options, || {
                self.authenticated_post(&url, &api_key, &request)
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
            let error = format!("upstream returned {status}: {body}");
            Self::spawn_log(
                &self.llm_logger,
                log_enabled,
                &agent.name,
                &agent.model_id,
                &provider.name,
                &request_json,
                Some(&body),
                Some(status.as_u16()),
                Some(&error),
            );
            return Err(AppError::Llm(error));
        }

        Ok(stream_response(
            response,
            NativeStreamParser::new(self.protocol),
            options,
            self.llm_logger.clone(),
            log_enabled,
            agent.name.clone(),
            agent.model_id.clone(),
            provider.name.clone(),
            request_json,
            status.as_u16(),
        ))
    }

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>> {
        let api_key = self.api_key(provider)?.to_string();
        let mut url = self.models_url(provider);
        let mut models = Vec::new();
        let mut pages = 0_u8;

        loop {
            pages += 1;
            let response = tokio::time::timeout(
                DEFAULT_MODEL_LIST_TIMEOUT,
                self.authenticated_get(&url, &api_key).send(),
            )
            .await
            .map_err(|_| AppError::Llm("model list request timed out after 30s".to_string()))?
            .map_err(|error| AppError::Llm(error.to_string()))?;
            let status = response.status();
            let body = read_response_body_limited(response).await?;
            if !status.is_success() {
                return Err(AppError::Llm(format!("upstream returned {status}: {body}")));
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|error| AppError::Llm(format!("invalid model list JSON: {error}")))?;

            match self.protocol {
                NativeProtocol::OpenAiResponses | NativeProtocol::AnthropicMessages => {
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
                NativeProtocol::GoogleGemini => {
                    models.extend(
                        value["models"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|entry| {
                                entry["supportedGenerationMethods"].as_array().is_none_or(
                                    |methods| {
                                        methods.iter().any(|method| {
                                            matches!(
                                                method.as_str(),
                                                Some("generateContent" | "streamGenerateContent")
                                            )
                                        })
                                    },
                                )
                            })
                            .filter_map(|entry| entry["name"].as_str())
                            .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string()),
                    );
                }
            }

            if self.protocol != NativeProtocol::GoogleGemini || pages >= 20 {
                break;
            }
            let Some(token) = value["nextPageToken"]
                .as_str()
                .filter(|token| !token.is_empty())
            else {
                break;
            };
            url = add_query_parameter(&self.models_url(provider), "pageToken", token)?;
        }

        models.sort();
        models.dedup();
        Ok(models)
    }
}

fn build_request(
    protocol: NativeProtocol,
    agent: &Agent,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<Value> {
    match protocol {
        NativeProtocol::OpenAiResponses => build_openai_responses_request(
            agent,
            messages,
            stream,
            system_prompt,
            extra_body,
            tools,
        ),
        NativeProtocol::AnthropicMessages => {
            build_anthropic_request(agent, messages, stream, system_prompt, extra_body, tools)
        }
        NativeProtocol::GoogleGemini => {
            build_gemini_request(messages, system_prompt, extra_body, tools)
        }
    }
}

fn build_openai_responses_request(
    agent: &Agent,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<Value> {
    let mut input = Vec::new();
    for message in select_context_messages(messages) {
        match message.role {
            ChatRole::User => input.push(json!({
                "role": "user",
                "content": message.content,
            })),
            ChatRole::Assistant => {
                if !message.content.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": message.content,
                    }));
                }
                for call in message_tool_calls(message)? {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.function.name,
                        "arguments": call.function.arguments,
                    }));
                }
            }
            ChatRole::Tool => {
                let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                    AppError::Llm("stored tool message is missing tool_call_id".to_string())
                })?;
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": message.content,
                }));
            }
        }
    }

    let mut body = Map::new();
    body.insert("model".to_string(), json!(agent.model_id));
    body.insert("instructions".to_string(), json!(system_prompt));
    body.insert("input".to_string(), Value::Array(input));
    body.insert("stream".to_string(), Value::Bool(stream));
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        body.insert(
            "tools".to_string(),
            Value::Array(
                tools
                    .into_iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "name": tool.function.name,
                            "description": tool.function.description,
                            "parameters": tool.function.parameters,
                        })
                    })
                    .collect(),
            ),
        );
    }
    merge_extra_body(
        body,
        extra_body,
        &["model", "instructions", "input", "stream", "tools"],
    )
}

fn build_anthropic_request(
    agent: &Agent,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<Value> {
    let selected = select_context_messages(messages);
    let mut call_names = HashMap::new();
    for message in &selected {
        for call in message_tool_calls(message)? {
            call_names.insert(call.id, call.function.name);
        }
    }

    let mut anthropic_messages: Vec<Value> = Vec::new();
    for message in selected {
        let (role, blocks) = match message.role {
            ChatRole::User => (
                "user",
                vec![json!({"type": "text", "text": message.content})],
            ),
            ChatRole::Assistant => {
                let mut blocks = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": message.content}));
                }
                for call in message_tool_calls(message)? {
                    let input =
                        parse_json_object(&call.function.arguments, "stored tool call arguments")?;
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.function.name,
                        "input": input,
                    }));
                }
                ("assistant", blocks)
            }
            ChatRole::Tool => {
                let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                    AppError::Llm("stored tool message is missing tool_call_id".to_string())
                })?;
                let _ = call_names.get(call_id).ok_or_else(|| {
                    AppError::Llm(format!(
                        "stored tool result references unknown call id {call_id}"
                    ))
                })?;
                (
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": message.content,
                    })],
                )
            }
        };
        if blocks.is_empty() {
            continue;
        }
        append_content_blocks(&mut anthropic_messages, role, blocks);
    }

    let mut body = Map::new();
    body.insert("model".to_string(), json!(agent.model_id));
    body.insert("max_tokens".to_string(), json!(4096));
    body.insert("system".to_string(), json!(system_prompt));
    body.insert("messages".to_string(), Value::Array(anthropic_messages));
    body.insert("stream".to_string(), Value::Bool(stream));
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        body.insert(
            "tools".to_string(),
            Value::Array(
                tools
                    .into_iter()
                    .map(|tool| {
                        json!({
                            "name": tool.function.name,
                            "description": tool.function.description,
                            "input_schema": tool.function.parameters,
                        })
                    })
                    .collect(),
            ),
        );
    }
    merge_extra_body(
        body,
        extra_body,
        &["model", "system", "messages", "stream", "tools"],
    )
}

fn build_gemini_request(
    messages: &[ChatMessage],
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<Value> {
    let selected = select_context_messages(messages);
    let mut call_names = HashMap::new();
    for message in &selected {
        for call in message_tool_calls(message)? {
            call_names.insert(call.id, call.function.name);
        }
    }

    let mut contents: Vec<Value> = Vec::new();
    for message in selected {
        let (role, parts) = match message.role {
            ChatRole::User => ("user", vec![json!({"text": message.content})]),
            ChatRole::Assistant => {
                let mut parts = Vec::new();
                if !message.content.is_empty() {
                    parts.push(json!({"text": message.content}));
                }
                for call in message_tool_calls(message)? {
                    let args =
                        parse_json_object(&call.function.arguments, "stored tool call arguments")?;
                    parts.push(json!({
                        "functionCall": {
                            "name": call.function.name,
                            "args": args,
                        }
                    }));
                }
                ("model", parts)
            }
            ChatRole::Tool => {
                let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                    AppError::Llm("stored tool message is missing tool_call_id".to_string())
                })?;
                let name = call_names.get(call_id).ok_or_else(|| {
                    AppError::Llm(format!(
                        "stored tool result references unknown call id {call_id}"
                    ))
                })?;
                let response = normalize_gemini_tool_result(&message.content);
                (
                    "user",
                    vec![json!({
                        "functionResponse": {
                            "name": name,
                            "response": response,
                        }
                    })],
                )
            }
        };
        if parts.is_empty() {
            continue;
        }
        append_parts(&mut contents, role, parts);
    }

    let mut body = Map::new();
    body.insert(
        "systemInstruction".to_string(),
        json!({"parts": [{"text": system_prompt}]}),
    );
    body.insert("contents".to_string(), Value::Array(contents));
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        body.insert(
            "tools".to_string(),
            json!([{
                "functionDeclarations": tools
                    .into_iter()
                    .map(|tool| json!({
                        "name": tool.function.name,
                        "description": tool.function.description,
                        "parameters": tool.function.parameters,
                    }))
                    .collect::<Vec<_>>()
            }]),
        );
    }
    merge_extra_body(
        body,
        extra_body,
        &["systemInstruction", "contents", "tools"],
    )
}

fn parse_response(protocol: NativeProtocol, value: &Value) -> AppResult<ChatCompletionOutput> {
    match protocol {
        NativeProtocol::OpenAiResponses => parse_openai_responses_output(value),
        NativeProtocol::AnthropicMessages => parse_anthropic_output(value),
        NativeProtocol::GoogleGemini => parse_gemini_output(value),
    }
}

fn parse_openai_responses_output(value: &Value) -> AppResult<ChatCompletionOutput> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    for item in value["output"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("message") => {
                for block in item["content"].as_array().into_iter().flatten() {
                    if block["type"] == "output_text"
                        && let Some(text) = block["text"].as_str()
                    {
                        content.push_str(text);
                    }
                }
            }
            Some("function_call") => {
                tool_calls.push(ToolCall {
                    id: item["call_id"]
                        .as_str()
                        .or_else(|| item["id"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                    kind: "function".to_string(),
                    function: ToolCallFunction {
                        name: item["name"].as_str().unwrap_or_default().to_string(),
                        arguments: item["arguments"].as_str().unwrap_or("{}").to_string(),
                    },
                });
            }
            _ => {}
        }
    }
    if content.is_empty()
        && let Some(output_text) = value["output_text"].as_str()
    {
        content.push_str(output_text);
    }
    validate_tool_calls(tool_calls).map(|tool_calls| ChatCompletionOutput {
        content,
        tool_calls,
    })
}

fn parse_anthropic_output(value: &Value) -> AppResult<ChatCompletionOutput> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    for block in value["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str() {
                    content.push_str(text);
                }
            }
            Some("tool_use") => tool_calls.push(ToolCall {
                id: block["id"].as_str().unwrap_or_default().to_string(),
                kind: "function".to_string(),
                function: ToolCallFunction {
                    name: block["name"].as_str().unwrap_or_default().to_string(),
                    arguments: serde_json::to_string(&block["input"])
                        .unwrap_or_else(|_| "{}".to_string()),
                },
            }),
            _ => {}
        }
    }
    validate_tool_calls(tool_calls).map(|tool_calls| ChatCompletionOutput {
        content,
        tool_calls,
    })
}

fn parse_gemini_output(value: &Value) -> AppResult<ChatCompletionOutput> {
    let candidate = value["candidates"]
        .as_array()
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| {
            let reason = value["promptFeedback"]["blockReason"]
                .as_str()
                .unwrap_or("no candidates");
            AppError::Llm(format!("Gemini returned no candidates: {reason}"))
        })?;
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    for part in candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(text) = part["text"].as_str() {
            content.push_str(text);
        }
        if let Some(call) = part.get("functionCall") {
            tool_calls.push(ToolCall {
                id: call["id"]
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("gemini-call-{}", Uuid::new_v4())),
                kind: "function".to_string(),
                function: ToolCallFunction {
                    name: call["name"].as_str().unwrap_or_default().to_string(),
                    arguments: serde_json::to_string(&call["args"])
                        .unwrap_or_else(|_| "{}".to_string()),
                },
            });
        }
    }
    validate_tool_calls(tool_calls).map(|tool_calls| ChatCompletionOutput {
        content,
        tool_calls,
    })
}

struct NativeStreamParser {
    protocol: NativeProtocol,
    saw_tool_calls: bool,
    finished: bool,
    started_calls: HashSet<usize>,
    calls_with_arg_deltas: HashSet<usize>,
    next_tool_index: usize,
}

impl NativeStreamParser {
    fn new(protocol: NativeProtocol) -> Self {
        Self {
            protocol,
            saw_tool_calls: false,
            finished: false,
            started_calls: HashSet::new(),
            calls_with_arg_deltas: HashSet::new(),
            next_tool_index: 0,
        }
    }

    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>> {
        let Some(data) = line.strip_prefix("data:") else {
            return Vec::new();
        };
        let data = data.strip_prefix(' ').unwrap_or(data).trim();
        if data.is_empty() {
            return Vec::new();
        }
        if data == "[DONE]" {
            return self.finish();
        }
        let value: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(error) => return vec![Err(AppError::Llm(format!("invalid SSE JSON: {error}")))],
        };
        match self.protocol {
            NativeProtocol::OpenAiResponses => self.parse_openai_event(&value),
            NativeProtocol::AnthropicMessages => self.parse_anthropic_event(&value),
            NativeProtocol::GoogleGemini => self.parse_gemini_chunk(&value),
        }
    }

    fn parse_openai_event(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
        match value["type"].as_str() {
            Some("response.output_text.delta") => value["delta"]
                .as_str()
                .filter(|delta| !delta.is_empty())
                .map(|delta| vec![Ok(StreamEvent::TextDelta(delta.to_string()))])
                .unwrap_or_default(),
            Some("response.output_item.added") => self.openai_function_item(value, false),
            Some("response.output_item.done") => self.openai_function_item(value, true),
            Some("response.function_call_arguments.delta") => {
                let index = value["output_index"].as_u64().unwrap_or(0) as usize;
                let Some(arguments) = value["delta"].as_str().filter(|value| !value.is_empty())
                else {
                    return Vec::new();
                };
                self.saw_tool_calls = true;
                self.calls_with_arg_deltas.insert(index);
                vec![Ok(StreamEvent::ToolCallArgsDelta {
                    index,
                    arguments: arguments.to_string(),
                })]
            }
            Some("response.completed") => self.finish(),
            Some("response.failed" | "response.incomplete" | "error") => {
                let message = value["error"]["message"]
                    .as_str()
                    .or_else(|| value["message"].as_str())
                    .unwrap_or("OpenAI Responses stream failed");
                vec![Err(AppError::Llm(message.to_string()))]
            }
            _ => Vec::new(),
        }
    }

    fn openai_function_item(
        &mut self,
        value: &Value,
        include_arguments_if_missing: bool,
    ) -> Vec<AppResult<StreamEvent>> {
        let item = &value["item"];
        if item["type"] != "function_call" {
            return Vec::new();
        }
        let index = value["output_index"].as_u64().unwrap_or(0) as usize;
        let mut events = Vec::new();
        self.saw_tool_calls = true;
        if self.started_calls.insert(index) {
            events.push(Ok(StreamEvent::ToolCallStart {
                index,
                id: item["call_id"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .unwrap_or_default()
                    .to_string(),
                name: item["name"].as_str().unwrap_or_default().to_string(),
            }));
        }
        if include_arguments_if_missing
            && !self.calls_with_arg_deltas.contains(&index)
            && let Some(arguments) = item["arguments"].as_str().filter(|value| !value.is_empty())
        {
            events.push(Ok(StreamEvent::ToolCallArgsDelta {
                index,
                arguments: arguments.to_string(),
            }));
        }
        events
    }

    fn parse_anthropic_event(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
        match value["type"].as_str() {
            Some("content_block_start") => {
                let block = &value["content_block"];
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                match block["type"].as_str() {
                    Some("text") => block["text"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(|text| vec![Ok(StreamEvent::TextDelta(text.to_string()))])
                        .unwrap_or_default(),
                    Some("tool_use") => {
                        self.saw_tool_calls = true;
                        self.started_calls.insert(index);
                        let mut events = vec![Ok(StreamEvent::ToolCallStart {
                            index,
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                        })];
                        if block["input"]
                            .as_object()
                            .is_some_and(|input| !input.is_empty())
                        {
                            events.push(Ok(StreamEvent::ToolCallArgsDelta {
                                index,
                                arguments: block["input"].to_string(),
                            }));
                            self.calls_with_arg_deltas.insert(index);
                        }
                        events
                    }
                    _ => Vec::new(),
                }
            }
            Some("content_block_delta") => match value["delta"]["type"].as_str() {
                Some("text_delta") => value["delta"]["text"]
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .map(|text| vec![Ok(StreamEvent::TextDelta(text.to_string()))])
                    .unwrap_or_default(),
                Some("input_json_delta") => {
                    let index = value["index"].as_u64().unwrap_or(0) as usize;
                    let Some(arguments) = value["delta"]["partial_json"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                    else {
                        return Vec::new();
                    };
                    self.saw_tool_calls = true;
                    self.calls_with_arg_deltas.insert(index);
                    vec![Ok(StreamEvent::ToolCallArgsDelta {
                        index,
                        arguments: arguments.to_string(),
                    })]
                }
                _ => Vec::new(),
            },
            Some("message_stop") => self.finish(),
            Some("error") => {
                let message = value["error"]["message"]
                    .as_str()
                    .unwrap_or("Anthropic Messages stream failed");
                vec![Err(AppError::Llm(message.to_string()))]
            }
            _ => Vec::new(),
        }
    }

    fn parse_gemini_chunk(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
        let Some(candidate) = value["candidates"]
            .as_array()
            .and_then(|candidates| candidates.first())
        else {
            if let Some(reason) = value["promptFeedback"]["blockReason"].as_str() {
                return vec![Err(AppError::Llm(format!(
                    "Gemini blocked the request: {reason}"
                )))];
            }
            return Vec::new();
        };
        let mut events = Vec::new();
        for part in candidate["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(text) = part["text"].as_str().filter(|text| !text.is_empty()) {
                events.push(Ok(StreamEvent::TextDelta(text.to_string())));
            }
            if let Some(call) = part.get("functionCall") {
                let index = self.next_tool_index;
                self.next_tool_index += 1;
                self.saw_tool_calls = true;
                events.push(Ok(StreamEvent::ToolCallStart {
                    index,
                    id: call["id"]
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("gemini-call-{}", Uuid::new_v4())),
                    name: call["name"].as_str().unwrap_or_default().to_string(),
                }));
                events.push(Ok(StreamEvent::ToolCallArgsDelta {
                    index,
                    arguments: serde_json::to_string(&call["args"])
                        .unwrap_or_else(|_| "{}".to_string()),
                }));
            }
        }
        if candidate.get("finishReason").is_some() {
            events.extend(self.finish());
        }
        events
    }

    fn finish(&mut self) -> Vec<AppResult<StreamEvent>> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        vec![Ok(if self.saw_tool_calls {
            StreamEvent::FinishedWithToolCalls
        } else {
            StreamEvent::Finished
        })]
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_response(
    response: Response,
    mut parser: NativeStreamParser,
    options: RequestOptions,
    logger: Option<Arc<LlmLogger>>,
    log_enabled: bool,
    agent_name: String,
    model_id: String,
    provider_name: String,
    request_json: String,
    status: u16,
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
                    NativeChatClient::spawn_log(
                        &logger,
                        log_enabled,
                        &agent_name,
                        &model_id,
                        &provider_name,
                        &request_json,
                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                        Some(status),
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
                                    NativeChatClient::spawn_log(
                                        &logger,
                                        log_enabled,
                                        &agent_name,
                                        &model_id,
                                        &provider_name,
                                        &request_json,
                                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                                        Some(status),
                                        Some(&error.to_string()),
                                    );
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
                        yield Err(error);
                        return;
                    }
                }
                Err(error) => {
                    let error = AppError::Llm(error.to_string());
                    NativeChatClient::spawn_log(
                        &logger,
                        log_enabled,
                        &agent_name,
                        &model_id,
                        &provider_name,
                        &request_json,
                        Some(&format_stream_log(&raw_lines, raw_truncated)),
                        Some(status),
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
        NativeChatClient::spawn_log(
            &logger,
            log_enabled,
            &agent_name,
            &model_id,
            &provider_name,
            &request_json,
            Some(&format_stream_log(&raw_lines, raw_truncated)),
            Some(status),
            None,
        );
    };
    Box::pin(stream)
}

fn append_content_blocks(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = messages.last_mut()
        && last["role"] == role
        && let Some(content) = last["content"].as_array_mut()
    {
        content.extend(blocks);
        return;
    }
    messages.push(json!({"role": role, "content": blocks}));
}

fn append_parts(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if let Some(last) = contents.last_mut()
        && last["role"] == role
        && let Some(existing) = last["parts"].as_array_mut()
    {
        existing.extend(parts);
        return;
    }
    contents.push(json!({"role": role, "parts": parts}));
}

fn message_tool_calls(message: &ChatMessage) -> AppResult<Vec<ToolCall>> {
    message
        .tool_calls
        .as_ref()
        .map(|calls| {
            serde_json::from_value(calls.clone())
                .map_err(|error| AppError::Llm(format!("stored tool calls are invalid: {error}")))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse_json_object(value: &str, label: &str) -> AppResult<Value> {
    match serde_json::from_str::<Value>(value) {
        Ok(Value::Object(map)) => Ok(Value::Object(map)),
        Ok(_) => Err(AppError::Llm(format!("{label} must be a JSON object"))),
        Err(error) => Err(AppError::Llm(format!("{label} are invalid: {error}"))),
    }
}

fn normalize_gemini_tool_result(content: &str) -> Value {
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(value) => json!({"result": value}),
        Err(_) => json!({"result": content}),
    }
}

fn validate_tool_calls(tool_calls: Vec<ToolCall>) -> AppResult<Vec<ToolCall>> {
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

fn merge_extra_body(
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

fn encode_path_segment(value: &str) -> String {
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

fn add_query_parameter(base: &str, key: &str, value: &str) -> AppResult<String> {
    let mut url = reqwest::Url::parse(base).map_err(|error| AppError::Llm(error.to_string()))?;
    url.query_pairs_mut().append_pair(key, value);
    Ok(url.to_string())
}

fn capture_log_line(
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

fn format_stream_log(raw_lines: &[String], truncated: bool) -> String {
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

async fn read_response_body_limited(response: Response) -> AppResult<String> {
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
    use std::sync::Mutex;

    use axum::{
        Router,
        body::{Body, Bytes},
        extract::State,
        http::{HeaderMap, Method, Uri},
        response::Response,
        routing::any,
    };
    use chrono::Utc;

    use super::*;
    use crate::domain::{ChatMessageStatus, ModelProvider, ProviderType};

    fn fixture() -> (Agent, Vec<ChatMessage>) {
        let now = Utc::now();
        let provider_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes code".to_string(),
            model_provider_id: provider_id,
            model_id: "test-model".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let messages = vec![
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::User,
                content: "run it".to_string(),
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Assistant,
                content: String::new(),
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: Some(json!([{
                    "id": "call-1",
                    "type": "function",
                    "function": {
                        "name": "execute_terminal",
                        "arguments": "{\"command\":\"pwd\"}"
                    }
                }])),
                tool_call_id: None,
            },
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Tool,
                content: "{\"status\":\"completed\",\"stdout\":\"ok\"}".to_string(),
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: None,
                tool_call_id: Some("call-1".to_string()),
            },
        ];
        (agent, messages)
    }

    #[test]
    fn responses_request_maps_tool_history() {
        let (agent, messages) = fixture();
        let request =
            build_openai_responses_request(&agent, &messages, true, "system", &Value::Null, None)
                .unwrap();
        assert_eq!(request["model"], "test-model");
        assert_eq!(request["stream"], true);
        assert!(
            request["input"].as_array().unwrap().iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call-1"
            })
        );
    }

    #[test]
    fn anthropic_request_maps_tools_and_allows_max_tokens_override() {
        let (agent, messages) = fixture();
        let request = build_anthropic_request(
            &agent,
            &messages,
            false,
            "system",
            &json!({"max_tokens": 8192}),
            None,
        )
        .unwrap();
        assert_eq!(request["max_tokens"], 8192);
        assert!(
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["content"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|block| block["type"] == "tool_result")
                })
        );
    }

    #[test]
    fn gemini_request_maps_function_response() {
        let (_, messages) = fixture();
        let request = build_gemini_request(&messages, "system", &Value::Null, None).unwrap();
        assert!(
            request["contents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|part| part.get("functionResponse").is_some())
                })
        );
    }

    #[test]
    fn parses_all_native_non_streaming_outputs() {
        let responses = parse_openai_responses_output(&json!({
            "output": [
                {"type":"message","content":[{"type":"output_text","text":"hello"}]},
                {"type":"function_call","call_id":"call-r","name":"run","arguments":"{}"}
            ]
        }))
        .unwrap();
        assert_eq!(responses.content, "hello");
        assert_eq!(responses.tool_calls[0].id, "call-r");

        let anthropic = parse_anthropic_output(&json!({
            "content": [
                {"type":"text","text":"hi"},
                {"type":"tool_use","id":"call-a","name":"run","input":{"x":1}}
            ]
        }))
        .unwrap();
        assert_eq!(anthropic.content, "hi");
        assert_eq!(anthropic.tool_calls[0].function.arguments, "{\"x\":1}");

        let gemini = parse_gemini_output(&json!({
            "candidates": [{"content":{"parts":[
                {"text":"hey"},
                {"functionCall":{"id":"call-g","name":"run","args":{"x":2}}}
            ]}}]
        }))
        .unwrap();
        assert_eq!(gemini.content, "hey");
        assert_eq!(gemini.tool_calls[0].id, "call-g");
    }

    #[test]
    fn parses_native_stream_events() {
        let mut responses = NativeStreamParser::new(NativeProtocol::OpenAiResponses);
        let events =
            responses.parse_line(r#"data: {"type":"response.output_text.delta","delta":"hello"}"#);
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::TextDelta(text) if text == "hello"
        ));
        let starts = responses.parse_line(
            r#"data: {"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call-r","name":"run","arguments":""}}"#,
        );
        assert!(matches!(
            starts.into_iter().next().unwrap().unwrap(),
            StreamEvent::ToolCallStart { index: 1, .. }
        ));

        let mut anthropic = NativeStreamParser::new(NativeProtocol::AnthropicMessages);
        let events = anthropic.parse_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}"#,
        );
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::ToolCallArgsDelta { index: 0, .. }
        ));

        let mut gemini = NativeStreamParser::new(NativeProtocol::GoogleGemini);
        let events = gemini.parse_line(
            r#"data: {"candidates":[{"content":{"parts":[{"text":"gem"}]},"finishReason":"STOP"}]}"#,
        );
        assert!(matches!(
            events.first().unwrap().as_ref().unwrap(),
            StreamEvent::TextDelta(text) if text == "gem"
        ));
        assert!(matches!(
            events.last().unwrap().as_ref().unwrap(),
            StreamEvent::Finished
        ));
    }

    #[test]
    fn provider_type_defaults_to_openai_chat_and_round_trips_new_modes() {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            provider_type: ProviderType,
        }
        let defaulted: Wrapper = serde_json::from_str("{}").unwrap();
        assert_eq!(defaulted.provider_type, ProviderType::OpenAiCompatible);
        for value in ["openai_responses", "anthropic_messages", "google_gemini"] {
            let parsed: ProviderType =
                serde_json::from_value(Value::String(value.to_string())).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
    }

    fn provider(protocol: ProviderType, base_url: impl Into<String>) -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type: protocol,
            name: "test".to_string(),
            base_url: base_url.into(),
            api_key: "key".to_string(),
            models: vec!["model".to_string()],
            extra_body: Value::Null,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[derive(Clone)]
    struct UpstreamState {
        protocol: NativeProtocol,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    #[derive(Debug)]
    struct CapturedRequest {
        method: Method,
        path: String,
        headers: HeaderMap,
        body: Value,
    }

    async fn test_upstream(
        State(state): State<UpstreamState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let body_json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        state.requests.lock().unwrap().push(CapturedRequest {
            method: method.clone(),
            path: uri.path().to_string(),
            headers,
            body: body_json.clone(),
        });

        if method == Method::GET {
            let payload = match state.protocol {
                NativeProtocol::OpenAiResponses | NativeProtocol::AnthropicMessages => {
                    json!({"data": [{"id": "native-model"}]})
                }
                NativeProtocol::GoogleGemini => json!({
                    "models": [{
                        "name": "models/native-model",
                        "supportedGenerationMethods": ["generateContent"]
                    }]
                }),
            };
            return json_response(payload);
        }

        let streaming =
            body_json["stream"] == true || uri.path().ends_with(":streamGenerateContent");
        if streaming {
            let payload = match state.protocol {
                NativeProtocol::OpenAiResponses => concat!(
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"streamed\"}\n\n",
                    "data: {\"type\":\"response.completed\"}\n\n"
                ),
                NativeProtocol::AnthropicMessages => concat!(
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"streamed\"}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n"
                ),
                NativeProtocol::GoogleGemini => concat!(
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"streamed\"}]},\"finishReason\":\"STOP\"}]}\n\n"
                ),
            };
            return Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(Body::from(payload))
                .unwrap();
        }

        let payload = match state.protocol {
            NativeProtocol::OpenAiResponses => json!({
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": "complete"}]
                }]
            }),
            NativeProtocol::AnthropicMessages => {
                json!({"content": [{"type": "text", "text": "complete"}]})
            }
            NativeProtocol::GoogleGemini => json!({
                "candidates": [{"content": {"parts": [{"text": "complete"}]}}]
            }),
        };
        json_response(payload)
    }

    fn json_response(value: Value) -> Response {
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap()
    }

    async fn run_http_protocol_case(
        protocol: NativeProtocol,
        provider_type: ProviderType,
        expected_complete_path: &str,
        expected_stream_path: &str,
        auth_header: &str,
        auth_value: &str,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = UpstreamState {
            protocol,
            requests: requests.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(any(test_upstream)).with_state(state),
            )
            .await
            .unwrap();
        });

        let (agent, messages) = fixture();
        let provider = provider(provider_type, format!("http://{address}/v1"));
        let client = NativeChatClient::new(protocol);
        let options = RequestOptions {
            timeout: Duration::from_secs(5),
            retry_attempts: 1,
        };
        let complete = client
            .complete(&agent, &provider, &messages, "system", options, None)
            .await
            .unwrap();
        assert_eq!(complete.content, "complete");

        let mut stream = client
            .stream(&agent, &provider, &messages, "system", options, None)
            .await
            .unwrap();
        let mut streamed = String::new();
        while let Some(event) = stream.next().await {
            if let StreamEvent::TextDelta(delta) = event.unwrap() {
                streamed.push_str(&delta);
            }
        }
        assert_eq!(streamed, "streamed");
        assert_eq!(
            client.list_models(&provider).await.unwrap(),
            ["native-model"]
        );

        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0].method, Method::POST);
        assert_eq!(captured[0].path, expected_complete_path);
        assert_eq!(captured[1].path, expected_stream_path);
        assert_eq!(captured[2].method, Method::GET);
        assert_eq!(captured[2].path, "/v1/models");
        for request in captured.iter() {
            assert_eq!(
                request.headers.get(auth_header).unwrap().to_str().unwrap(),
                auth_value
            );
        }
        assert!(!captured[0].body.is_null());
        server.abort();
    }

    #[tokio::test]
    async fn native_clients_send_real_http_requests_with_protocol_auth_and_sse() {
        run_http_protocol_case(
            NativeProtocol::OpenAiResponses,
            ProviderType::OpenAiResponses,
            "/v1/responses",
            "/v1/responses",
            "authorization",
            "Bearer key",
        )
        .await;
        run_http_protocol_case(
            NativeProtocol::AnthropicMessages,
            ProviderType::AnthropicMessages,
            "/v1/messages",
            "/v1/messages",
            "x-api-key",
            "key",
        )
        .await;
        run_http_protocol_case(
            NativeProtocol::GoogleGemini,
            ProviderType::GoogleGemini,
            "/v1/models/test-model:generateContent",
            "/v1/models/test-model:streamGenerateContent",
            "x-goog-api-key",
            "key",
        )
        .await;
    }
}
