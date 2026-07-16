use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use reqwest::RequestBuilder;
use serde_json::{Map, Value, json};

use crate::{
    domain::{Agent, ChatMessage, ChatRole, ModelProvider},
    error::{AppError, AppResult},
    llm::{
        client::{ChatClient, ChatStream, RequestOptions},
        http::StreamLineParser,
        protocol::common::{
            ProtocolAdapter, ProtocolClient, default_models, merge_extra_body, message_tool_calls,
            parse_json_object, validate_tool_calls,
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
        messages: &[ChatMessage],
        stream: bool,
        system_prompt: &str,
        extra_body: &Value,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<Value> {
        build_anthropic_request(agent, messages, stream, system_prompt, extra_body, tools)
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
}

impl AnthropicStreamParser {
    fn new() -> Self {
        Self {
            saw_tool_calls: false,
            finished: false,
            stop_reason: None,
        }
    }

    fn parse_event(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
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
        vec![Ok(if self.saw_tool_calls {
            StreamEvent::FinishedWithToolCalls
        } else {
            StreamEvent::Finished
        })]
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
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: Some(json!([{
                    "id": "call-1",
                    "type": "function",
                    "function": {"name": "run", "arguments": "{}"}
                }])),
                tool_call_id: None,
            },
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Tool,
                content: "ok".to_string(),
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: None,
                tool_call_id: Some("call-1".to_string()),
            },
        ];
        (agent, messages)
    }

    #[test]
    fn request_maps_tools_and_allows_max_tokens_override() {
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
                .any(|message| message["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|block| block["type"] == "tool_result"))
        );
    }

    #[test]
    fn parses_output_and_stream_events() {
        let output = parse_anthropic_output(&json!({
            "content": [
                {"type":"text","text":"hi"},
                {"type":"tool_use","id":"call-a","name":"run","input":{"x":1}}
            ]
        }))
        .unwrap();
        assert_eq!(output.content, "hi");
        assert_eq!(output.tool_calls[0].function.arguments, "{\"x\":1}");

        let mut parser = AnthropicStreamParser::new();
        let events = parser.parse_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}"#,
        );
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::ToolCallArgsDelta { index: 0, .. }
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
}
