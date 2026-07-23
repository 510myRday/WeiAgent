use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use reqwest::RequestBuilder;
use serde_json::{Map, Value, json};

use crate::{
    domain::{Agent, ChatMessage, ChatRole, ImageSource, ModelProvider, ThinkingLevel, TokenUsage},
    error::{AppError, AppResult},
    llm::{
        cache::{PromptCacheMode, apply_provider_extra, build_context, effective_capabilities},
        client::{ChatClient, ChatStream, RequestOptions},
        http::StreamLineParser,
        protocol::common::{
            ProtocolAdapter, ProtocolClient, add_query_parameter, default_models, merge_extra_body,
            message_tool_calls, parse_json_object, validate_tool_calls,
        },
        types::{
            ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition,
            select_context_messages,
        },
    },
    storage::LlmLogger,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Clone)]
pub struct AnthropicMessagesClient {
    inner: ProtocolClient<AnthropicAdapter>,
}

impl AnthropicMessagesClient {
    pub fn new() -> Self {
        Self {
            inner: ProtocolClient::new(AnthropicAdapter),
        }
    }

    pub fn with_logger(mut self, logger: Arc<LlmLogger>) -> Self {
        self.inner = self.inner.with_logger(logger);
        self
    }
}

impl Default for AnthropicMessagesClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChatClient for AnthropicMessagesClient {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatCompletionOutput> {
        self.inner
            .complete(agent, provider, messages, system_prompt, options, tools)
            .await
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
        self.inner
            .stream(agent, provider, messages, system_prompt, options, tools)
            .await
    }

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>> {
        self.inner.list_models(provider).await
    }
}

#[derive(Clone, Copy)]
struct AnthropicAdapter;

impl ProtocolAdapter for AnthropicAdapter {
    fn completion_url(&self, provider: &ModelProvider, _agent: &Agent, _stream: bool) -> String {
        format!("{}/messages", provider.base_url.trim_end_matches('/'))
    }

    fn build_request(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        stream: bool,
        system_prompt: &str,
        extra_body: &Value,
        tools: Option<Vec<ToolDefinition>>,
        thinking_level: Option<ThinkingLevel>,
    ) -> AppResult<Value> {
        build_anthropic_request(
            agent,
            provider,
            messages,
            stream,
            system_prompt,
            extra_body,
            tools,
            thinking_level,
        )
    }

    fn authenticated_post(
        &self,
        http: &reqwest::Client,
        url: &str,
        api_key: &str,
        body: &Value,
    ) -> RequestBuilder {
        http.post(url)
            .json(body)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
    }

    fn authenticated_get(
        &self,
        http: &reqwest::Client,
        url: &str,
        api_key: &str,
    ) -> RequestBuilder {
        http.get(url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
    }

    fn parse_response(&self, value: &Value) -> AppResult<ChatCompletionOutput> {
        parse_anthropic_output(value)
    }

    fn stream_parser(&self) -> Box<dyn StreamLineParser> {
        Box::new(AnthropicStreamParser::new())
    }

    fn append_models(&self, value: &Value, models: &mut Vec<String>) {
        default_models(value, models);
    }

    fn next_models_url(
        &self,
        provider: &ModelProvider,
        value: &Value,
        pages: u8,
    ) -> AppResult<Option<String>> {
        if !value["has_more"].as_bool().unwrap_or(false) {
            return Ok(None);
        }
        if pages >= 100 {
            return Err(AppError::Llm(
                "Anthropic model list exceeded 100 pages".to_string(),
            ));
        }
        let cursor = value["last_id"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
            .ok_or_else(|| {
                AppError::Llm(
                    "Anthropic model list indicated another page without last_id".to_string(),
                )
            })?;
        add_query_parameter(&self.models_url(provider), "after_id", cursor).map(Some)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_anthropic_request(
    agent: &Agent,
    provider: &ModelProvider,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
    thinking_level: Option<ThinkingLevel>,
) -> AppResult<Value> {
    let context = build_context(
        provider,
        &agent.model_id,
        agent.id,
        "anthropic_messages",
        system_prompt,
        tools.as_deref(),
    );
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
            ChatRole::User => {
                let mut blocks = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": message.content}));
                }
                blocks.extend(message.attachments.iter().map(
                    |attachment| match &attachment.source {
                        ImageSource::Base64 { data } => json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": attachment.media_type,
                                "data": data,
                            }
                        }),
                        ImageSource::Url { url } => json!({
                            "type": "image",
                            "source": {
                                "type": "url",
                                "url": url,
                            }
                        }),
                    },
                ));
                ("user", blocks)
            }
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
    body.insert(
        "max_tokens".to_string(),
        json!(
            if thinking_level.is_some_and(|level| level != ThinkingLevel::Off) {
                32768
            } else {
                4096
            }
        ),
    );
    let capabilities = effective_capabilities(provider);
    let cache_enabled = match provider.prompt_cache.mode {
        PromptCacheMode::Disabled => false,
        PromptCacheMode::Automatic => capabilities.automatic_prefix_cache,
        PromptCacheMode::Explicit => capabilities.explicit_breakpoints,
    };
    if cache_enabled {
        let mut block = json!({"type": "text", "text": system_prompt});
        block["cache_control"] = provider.prompt_cache.anthropic_ttl.as_ref().map_or_else(
            || json!({"type": "ephemeral"}),
            |ttl| json!({"type": "ephemeral", "ttl": ttl}),
        );
        body.insert("system".to_string(), json!([block]));
    } else {
        body.insert("system".to_string(), json!(system_prompt));
    }
    body.insert("messages".to_string(), Value::Array(anthropic_messages));
    body.insert("stream".to_string(), Value::Bool(stream));
    if let Some(level) = thinking_level.filter(|level| *level != ThinkingLevel::Off) {
        if supports_adaptive_thinking(&agent.model_id) {
            body.insert("thinking".to_string(), json!({"type": "adaptive"}));
            body.insert(
                "output_config".to_string(),
                json!({
                    "effort": match level {
                        ThinkingLevel::Low => "low",
                        ThinkingLevel::Medium => "medium",
                        ThinkingLevel::High => "high",
                        ThinkingLevel::XHigh => "max",
                        ThinkingLevel::Off => unreachable!(),
                    }
                }),
            );
        } else {
            body.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": match level {
                        ThinkingLevel::Low => 1024,
                        ThinkingLevel::Medium => 4096,
                        ThinkingLevel::High => 8192,
                        ThinkingLevel::XHigh => 16384,
                        ThinkingLevel::Off => unreachable!(),
                    }
                }),
            );
        }
    }
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
    if cache_enabled
        && context.explicit_breakpoints
        && let Some(last) = body["tools"]
            .as_array_mut()
            .and_then(|tools| tools.last_mut())
    {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
    apply_provider_extra(&mut body, &context);
    merge_extra_body(
        body,
        extra_body,
        &[
            "model",
            "system",
            "messages",
            "stream",
            "tools",
            "thinking",
            "output_config",
        ],
    )
}

fn supports_adaptive_thinking(model_id: &str) -> bool {
    let normalized = model_id.to_ascii_lowercase();
    normalized.contains("4-6") || normalized.contains("4.6")
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

fn parse_anthropic_output(value: &Value) -> AppResult<ChatCompletionOutput> {
    validate_anthropic_stop_reason(value["stop_reason"].as_str())?;

    let mut content = String::new();
    let mut reasoning_content = String::new();
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
            Some("thinking") => {
                if let Some(thinking) = block["thinking"].as_str() {
                    reasoning_content.push_str(thinking);
                }
            }
            _ => {}
        }
    }
    validate_tool_calls(tool_calls).map(|tool_calls| ChatCompletionOutput {
        content,
        reasoning_content,
        tool_calls,
        usage: parse_anthropic_usage(&value["usage"]),
    })
}

fn parse_anthropic_usage(value: &Value) -> TokenUsage {
    let input = value["input_tokens"].as_u64().unwrap_or(0);
    let write = value["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let read = value["cache_read_input_tokens"].as_u64().unwrap_or(0);
    TokenUsage {
        input_tokens: input,
        output_tokens: value["output_tokens"].as_u64().unwrap_or(0),
        cache_write_tokens: write,
        cache_read_tokens: read,
        total_prompt_tokens: input.saturating_add(write).saturating_add(read),
        uncached_input_tokens: input,
        cache_hit_ratio: if input.saturating_add(read) == 0 {
            0.0
        } else {
            read as f64 / input.saturating_add(read) as f64
        },
    }
}

fn validate_anthropic_stop_reason(reason: Option<&str>) -> AppResult<()> {
    match reason {
        None | Some("end_turn" | "tool_use" | "stop_sequence") => Ok(()),
        Some("max_tokens") => Err(AppError::Llm(
            "Anthropic response stopped because the token limit was reached".to_string(),
        )),
        Some("pause_turn") => Err(AppError::Llm(
            "Anthropic response paused and requires a continuation turn".to_string(),
        )),
        Some("refusal") => Err(AppError::Llm(
            "Anthropic response was stopped by a refusal".to_string(),
        )),
        Some("model_context_window_exceeded") => Err(AppError::Llm(
            "Anthropic response exceeded the model context window".to_string(),
        )),
        Some(other) => Err(AppError::Llm(format!(
            "Anthropic response ended with unsupported stop_reason {other}"
        ))),
    }
}

struct AnthropicStreamParser {
    saw_tool_calls: bool,
    finished: bool,
    stop_reason: Option<String>,
    usage: TokenUsage,
}

impl AnthropicStreamParser {
    fn new() -> Self {
        Self {
            saw_tool_calls: false,
            finished: false,
            stop_reason: None,
            usage: TokenUsage::default(),
        }
    }

    fn parse_event(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
        match value["type"].as_str() {
            Some("message_start") => {
                self.usage = parse_anthropic_usage(&value["message"]["usage"]);
                Vec::new()
            }
            Some("content_block_start") => {
                let block = &value["content_block"];
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                match block["type"].as_str() {
                    Some("text") => block["text"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(|text| vec![Ok(StreamEvent::TextDelta(text.to_string()))])
                        .unwrap_or_default(),
                    Some("thinking") => block["thinking"]
                        .as_str()
                        .filter(|thinking| !thinking.is_empty())
                        .map(|thinking| vec![Ok(StreamEvent::ReasoningDelta(thinking.to_string()))])
                        .unwrap_or_default(),
                    Some("tool_use") => {
                        self.saw_tool_calls = true;
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
                Some("thinking_delta") => value["delta"]["thinking"]
                    .as_str()
                    .filter(|thinking| !thinking.is_empty())
                    .map(|thinking| vec![Ok(StreamEvent::ReasoningDelta(thinking.to_string()))])
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
                    vec![Ok(StreamEvent::ToolCallArgsDelta {
                        index,
                        arguments: arguments.to_string(),
                    })]
                }
                _ => Vec::new(),
            },
            Some("message_delta") => {
                if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
                let usage = parse_anthropic_usage(&value["usage"]);
                if usage.output_tokens > 0 {
                    self.usage.output_tokens = usage.output_tokens;
                }
                Vec::new()
            }
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

    fn finish(&mut self) -> Vec<AppResult<StreamEvent>> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        if let Err(error) = validate_anthropic_stop_reason(self.stop_reason.as_deref()) {
            return vec![Err(error)];
        }
        vec![
            Ok(StreamEvent::Usage(self.usage.clone())),
            Ok(if self.saw_tool_calls {
                StreamEvent::FinishedWithToolCalls
            } else {
                StreamEvent::Finished
            }),
        ]
    }
}

impl StreamLineParser for AnthropicStreamParser {
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
        self.parse_event(&value)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::domain::ChatMessageStatus;

    fn fixture() -> (Agent, Vec<ChatMessage>) {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes code".to_string(),
            model_provider_id: Uuid::new_v4(),
            model_id: "test-model".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let conversation_id = Uuid::new_v4();
        let messages = vec![
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Assistant,
                content: String::new(),
                reasoning_content: None,
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: Some(json!([{
                    "id": "call-1",
                    "type": "function",
                    "function": {"name": "run", "arguments": "{}"}
                }])),
                tool_call_id: None,
                attachments: Vec::new(),
                usage: None,
            },
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Tool,
                content: "ok".to_string(),
                reasoning_content: None,
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: None,
                tool_call_id: Some("call-1".to_string()),
                attachments: Vec::new(),
                usage: None,
            },
        ];
        (agent, messages)
    }

    fn test_provider() -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type: crate::domain::ProviderType::AnthropicMessages,
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_key: "key".into(),
            models: vec!["test-model".into()],
            model_configs: vec![],
            extra_body: Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn request_maps_tools_and_allows_max_tokens_override() {
        let (agent, mut messages) = fixture();
        let provider = test_provider();
        messages[0].reasoning_content = Some("saved-reasoning-must-not-be-sent".to_string());
        messages.insert(
            0,
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id: messages[0].conversation_id,
                role: ChatRole::User,
                content: "inspect".to_string(),
                reasoning_content: None,
                status: ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: None,
                tool_call_id: None,
                attachments: vec![crate::domain::ImageAttachment {
                    id: "image-1".to_string(),
                    name: "sample.png".to_string(),
                    media_type: "image/png".to_string(),
                    source: ImageSource::Base64 {
                        data: "aGVsbG8=".to_string(),
                    },
                }],
                usage: None,
            },
        );
        let request = build_anthropic_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &json!({"max_tokens": 8192}),
            None,
            Some(ThinkingLevel::High),
        )
        .unwrap();
        assert_eq!(request["max_tokens"], 8192);
        assert_eq!(request["thinking"]["type"], "enabled");
        assert_eq!(request["thinking"]["budget_tokens"], 8192);
        assert!(
            !request
                .to_string()
                .contains("saved-reasoning-must-not-be-sent")
        );
        assert!(
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["content"].as_array().unwrap().iter().any(|block| {
                        block["type"] == "image" && block["source"]["type"] == "base64"
                    })
                })
        );
        assert!(
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|block| block["type"] == "tool_result"))
        );
    }

    #[test]
    fn omits_thinking_when_level_is_off() {
        let (agent, messages) = fixture();
        let provider = test_provider();
        let request = build_anthropic_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            Some(ThinkingLevel::Off),
        )
        .unwrap();
        assert!(request.get("thinking").is_none());
    }

    #[test]
    fn parses_output_and_stream_events() {
        let output = parse_anthropic_output(&json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 30,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 20
            },
            "content": [
                {"type":"thinking","thinking":"considering"},
                {"type":"text","text":"hi"},
                {"type":"tool_use","id":"call-a","name":"run","input":{"x":1}}
            ]
        }))
        .unwrap();
        assert_eq!(output.content, "hi");
        assert_eq!(output.reasoning_content, "considering");
        assert_eq!(output.tool_calls[0].function.arguments, "{\"x\":1}");
        assert_eq!(output.usage.total_tokens(), 160);

        let mut parser = AnthropicStreamParser::new();
        let events = parser.parse_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}"#,
        );
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::ToolCallArgsDelta { index: 0, .. }
        ));

        let events = parser.parse_line(
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
        );
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::ReasoningDelta(text) if text == "plan"
        ));
    }

    #[test]
    fn max_tokens_stop_reason_is_an_error() {
        let error = parse_anthropic_output(&json!({
            "stop_reason": "max_tokens",
            "content": [{"type":"text","text":"partial"}]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("token limit"));

        let mut parser = AnthropicStreamParser::new();
        assert!(
            parser
                .parse_line(
                    r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#
                )
                .is_empty()
        );
        let events = parser.parse_line(r#"data: {"type":"message_stop"}"#);
        assert!(events.into_iter().next().unwrap().is_err());
    }

    #[test]
    fn model_list_follows_anthropic_cursor_pagination() {
        let provider = ModelProvider {
            id: Uuid::new_v4(),
            provider_type: crate::domain::ProviderType::AnthropicMessages,
            name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            api_key: "test-key".to_string(),
            models: Vec::new(),
            model_configs: Vec::new(),
            extra_body: Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let adapter = AnthropicAdapter;

        let next = adapter
            .next_models_url(
                &provider,
                &json!({
                    "data": [{"id": "claude-a"}],
                    "has_more": true,
                    "last_id": "claude/a"
                }),
                1,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            next,
            "https://api.anthropic.com/v1/models?after_id=claude%2Fa"
        );
        assert!(
            adapter
                .next_models_url(
                    &provider,
                    &json!({"data": [], "has_more": false, "last_id": null}),
                    2,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            adapter
                .next_models_url(
                    &provider,
                    &json!({"data": [], "has_more": true, "last_id": null}),
                    2,
                )
                .is_err()
        );
    }
}
