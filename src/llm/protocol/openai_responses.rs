use std::collections::HashSet;

use reqwest::RequestBuilder;
use serde_json::{Map, Value, json};

use crate::{
    domain::{Agent, ChatMessage, ChatRole, ModelProvider, ThinkingLevel},
    error::{AppError, AppResult},
    llm::{
        cache::{
            PromptCacheMode, apply_openai_fields, apply_responses_prompt_cache_breakpoints,
            build_context, effective_capabilities,
        },
        http::StreamLineParser,
        protocol::common::{
            ProtocolAdapter, SseData, bearer_get, bearer_post, default_models, merge_extra_body,
            message_tool_calls, parse_sse_data_line, validate_tool_calls,
        },
        types::{
            ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition,
            openai_image_url, select_context_messages, thinking_effort_str,
        },
    },
};

#[derive(Clone, Copy)]
pub(crate) struct OpenAiResponsesAdapter;

impl ProtocolAdapter for OpenAiResponsesAdapter {
    fn completion_url(&self, provider: &ModelProvider, _agent: &Agent, _stream: bool) -> String {
        format!("{}/responses", provider.base_url.trim_end_matches('/'))
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
        build_openai_responses_request(
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
        _provider: &ModelProvider,
    ) -> RequestBuilder {
        bearer_post(http, url, api_key, body)
    }

    fn authenticated_get(
        &self,
        http: &reqwest::Client,
        url: &str,
        api_key: &str,
    ) -> RequestBuilder {
        bearer_get(http, url, api_key)
    }

    fn parse_response(&self, value: &Value) -> AppResult<ChatCompletionOutput> {
        parse_openai_responses_output(value)
    }

    fn stream_parser(&self) -> Box<dyn StreamLineParser> {
        Box::new(OpenAiResponsesStreamParser::new())
    }

    fn append_models(&self, value: &Value, models: &mut Vec<String>) {
        default_models(value, models);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_openai_responses_request(
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
        "openai_responses",
        system_prompt,
        tools.as_deref(),
    );
    let mut input = Vec::new();
    for message in select_context_messages(messages) {
        match message.role {
            ChatRole::User => {
                let mut content = Vec::new();
                if !message.content.is_empty() || message.attachments.is_empty() {
                    content.push(json!({
                        "type": "input_text",
                        "text": message.content,
                    }));
                }
                content.extend(message.attachments.iter().map(|attachment| {
                    json!({
                        "type": "input_image",
                        "image_url": openai_image_url(attachment),
                    })
                }));
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": content,
                }));
            }
            ChatRole::Assistant => {
                input.extend(
                    message
                        .openai_response_items
                        .iter()
                        .filter(|item| {
                            item["type"] == "reasoning"
                                && item["encrypted_content"]
                                    .as_str()
                                    .is_some_and(|content| !content.is_empty())
                        })
                        .cloned(),
                );
                if !message.content.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": message.content,
                        }],
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

    let capabilities = effective_capabilities(provider);
    let cache_enabled = !matches!(provider.prompt_cache.mode, PromptCacheMode::Disabled)
        && capabilities.automatic_prefix_cache
        && (!matches!(provider.prompt_cache.mode, PromptCacheMode::Explicit)
            || capabilities.explicit_breakpoints);
    // GPT-5.6+ no longer falls back to the longest common unmarked prefix.
    // Mark stable user content so multi-turn agent loops can still hit cache.
    if cache_enabled && context.explicit_breakpoints {
        apply_responses_prompt_cache_breakpoints(&mut input);
    }
    let replays_reasoning = input.iter().any(|item| {
        item["type"] == "reasoning"
            && item["encrypted_content"]
                .as_str()
                .is_some_and(|content| !content.is_empty())
    });
    let mut body = Map::new();
    let prompt_cache_key = cache_enabled.then(|| {
        if provider.prompt_cache.use_stable_key && capabilities.stable_cache_key {
            context
                .cache_key
                .clone()
                .unwrap_or_else(|| agent.id.to_string())
        } else {
            messages
                .last()
                .map(|message| message.conversation_id.to_string())
                .unwrap_or_else(|| agent.id.to_string())
        }
    });
    body.insert("model".to_string(), json!(agent.model_id));
    body.insert("instructions".to_string(), json!(system_prompt));
    body.insert("input".to_string(), Value::Array(input));
    body.insert("stream".to_string(), Value::Bool(stream));
    let effort = thinking_level.and_then(thinking_effort_str);
    if effort.is_some() || replays_reasoning {
        let mut reasoning = Map::new();
        if let Some(effort) = effort {
            reasoning.insert("effort".to_string(), json!(effort));
            reasoning.insert("summary".to_string(), Value::String("auto".to_string()));
        }
        if replays_reasoning {
            reasoning.insert(
                "context".to_string(),
                Value::String("all_turns".to_string()),
            );
        }
        body.insert("reasoning".to_string(), Value::Object(reasoning));
    }
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
    apply_openai_fields(&mut body, provider, &context);
    // The app conversation is the session and reuses this key for
    // every turn, independent of optional provider cache settings.
    body.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    if let Some(prompt_cache_key) = prompt_cache_key {
        body.insert("prompt_cache_key".to_string(), json!(prompt_cache_key));
    }
    body.insert("store".to_string(), Value::Bool(false));
    body.insert("tool_choice".to_string(), Value::String("auto".to_string()));
    body.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    merge_extra_body(
        body,
        extra_body,
        &[
            "model",
            "instructions",
            "input",
            "stream",
            "tools",
            "reasoning",
            "include",
            "prompt_cache_key",
            "prompt_cache_retention",
            "prompt_cache_options",
            "store",
            "tool_choice",
            "parallel_tool_calls",
        ],
    )
}

fn parse_openai_responses_output(value: &Value) -> AppResult<ChatCompletionOutput> {
    validate_openai_responses_status(value)?;

    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut openai_response_items = Vec::new();
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
            Some("reasoning") => {
                for summary in item["summary"].as_array().into_iter().flatten() {
                    if let Some(text) = summary["text"].as_str() {
                        reasoning_content.push_str(text);
                    }
                }
                if item["encrypted_content"]
                    .as_str()
                    .is_some_and(|content| !content.is_empty())
                {
                    openai_response_items.push(item.clone());
                }
            }
            _ => {}
        }
    }
    if content.is_empty()
        && let Some(output_text) = value["output_text"].as_str()
    {
        content.push_str(output_text);
    }
    let usage = serde_json::from_value::<crate::llm::types::OpenAiUsage>(value["usage"].clone())
        .map(|usage| usage.normalized())
        .unwrap_or_default();
    validate_tool_calls(tool_calls).map(|tool_calls| ChatCompletionOutput {
        content,
        reasoning_content,
        openai_response_items,
        tool_calls,
        usage,
    })
}

fn validate_openai_responses_status(value: &Value) -> AppResult<()> {
    match value["status"].as_str() {
        None | Some("completed") => Ok(()),
        Some("failed") => {
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("OpenAI Responses request failed");
            Err(AppError::Llm(message.to_string()))
        }
        Some("incomplete") => {
            let reason = value["incomplete_details"]["reason"]
                .as_str()
                .unwrap_or("unknown reason");
            Err(AppError::Llm(format!(
                "OpenAI Responses request was incomplete: {reason}"
            )))
        }
        Some(status) => Err(AppError::Llm(format!(
            "OpenAI Responses returned unsupported status {status}"
        ))),
    }
}

struct OpenAiResponsesStreamParser {
    saw_tool_calls: bool,
    finished: bool,
    started_calls: HashSet<usize>,
    calls_with_arg_deltas: HashSet<usize>,
}

impl OpenAiResponsesStreamParser {
    fn new() -> Self {
        Self {
            saw_tool_calls: false,
            finished: false,
            started_calls: HashSet::new(),
            calls_with_arg_deltas: HashSet::new(),
        }
    }

    fn parse_event(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
        match value["type"].as_str() {
            Some("response.output_text.delta") => value["delta"]
                .as_str()
                .filter(|delta| !delta.is_empty())
                .map(|delta| vec![Ok(StreamEvent::TextDelta(delta.to_string()))])
                .unwrap_or_default(),
            Some("response.reasoning_summary_text.delta" | "response.reasoning_text.delta") => {
                value["delta"]
                    .as_str()
                    .filter(|delta| !delta.is_empty())
                    .map(|delta| vec![Ok(StreamEvent::ReasoningDelta(delta.to_string()))])
                    .unwrap_or_default()
            }
            Some("response.output_item.added") => self.function_item(value, false),
            Some("response.output_item.done") => {
                let item = &value["item"];
                if item["type"] == "reasoning"
                    && item["encrypted_content"]
                        .as_str()
                        .is_some_and(|content| !content.is_empty())
                {
                    vec![Ok(StreamEvent::OpenAiResponseItem(item.clone()))]
                } else {
                    self.function_item(value, true)
                }
            }
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
            Some("response.completed") => {
                let mut events = Vec::new();
                let usage_value = value
                    .get("response")
                    .and_then(|response| response.get("usage"))
                    .or_else(|| value.get("usage"));
                if let Some(usage_value) = usage_value
                    && let Ok(usage) = serde_json::from_value::<crate::llm::types::OpenAiUsage>(
                        usage_value.clone(),
                    )
                {
                    events.push(Ok(StreamEvent::Usage(usage.normalized())));
                }
                events.extend(self.finish());
                events
            }
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

    fn function_item(
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

impl StreamLineParser for OpenAiResponsesStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>> {
        match parse_sse_data_line(line) {
            Ok(SseData::Skip) => Vec::new(),
            Ok(SseData::Done) => self.finish(),
            Ok(SseData::Json(value)) => self.parse_event(&value),
            Err(error) => vec![Err(error)],
        }
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
                role: ChatRole::User,
                content: "run it".to_string(),
                reasoning_content: None,
                openai_response_items: Vec::new(),
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: None,
                tool_call_id: None,
                attachments: vec![crate::domain::ImageAttachment {
                    id: "image-1".to_string(),
                    name: "sample.png".to_string(),
                    media_type: "image/png".to_string(),
                    is_mask: false,
                    source: crate::domain::ImageSource::Url {
                        url: "https://example.test/sample.png".to_string(),
                    },
                }],
                usage: None,
            },
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Assistant,
                content: String::new(),
                reasoning_content: None,
                openai_response_items: Vec::new(),
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
                openai_response_items: Vec::new(),
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
            provider_type: crate::domain::ProviderType::OpenAiResponses,
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_keys: vec!["key".into()],
            api_key_labels: vec![],
            key_strategy: Default::default(),
            models: vec!["test-model".into()],
            model_configs: vec![],
            extra_body: Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
            proxy_url: None,
            image_generation_url: None,
            image_edit_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn request_maps_tool_history() {
        let (agent, mut messages) = fixture();
        let provider = test_provider();
        messages[1].reasoning_content = Some("saved-reasoning-must-not-be-sent".to_string());
        let encrypted_reasoning = json!({
            "id": "rs-test",
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "opaque-state"
        });
        messages[1].openai_response_items = vec![encrypted_reasoning.clone()];
        let request = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            true,
            "system",
            &Value::Null,
            None,
            Some(ThinkingLevel::XHigh),
        )
        .unwrap();
        assert_eq!(request["model"], "test-model");
        assert_eq!(request["stream"], true);
        assert_eq!(request["reasoning"]["effort"], "xhigh");
        assert_eq!(request["reasoning"]["summary"], "auto");
        assert_eq!(request["reasoning"]["context"], "all_turns");
        assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(
            request["prompt_cache_key"],
            messages[0].conversation_id.to_string()
        );
        assert_eq!(request["store"], false);
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(request["parallel_tool_calls"], true);
        assert!(request["input"].as_array().unwrap().iter().any(|item| {
            item["type"] == "message"
                && item["role"] == "user"
                && item["content"].as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block["type"] == "input_text" && block["text"] == "run it")
                })
        }));
        assert!(request["input"].as_array().unwrap().iter().any(|item| {
            item["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "input_image"
                        && block["image_url"] == "https://example.test/sample.png"
                })
            })
        }));
        assert!(
            request["input"].as_array().unwrap().iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call-1"
            })
        );
        let input = request["input"].as_array().unwrap();
        let reasoning_index = input
            .iter()
            .position(|item| item == &encrypted_reasoning)
            .unwrap();
        let function_index = input
            .iter()
            .position(|item| item["type"] == "function_call")
            .unwrap();
        assert!(reasoning_index < function_index);
        assert!(
            !request
                .to_string()
                .contains("saved-reasoning-must-not-be-sent")
        );
    }

    #[test]
    fn prompt_cache_key_tracks_the_conversation() {
        let (agent, messages) = fixture();
        let provider = test_provider();
        let first = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        let repeated = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            true,
            "system",
            &Value::Null,
            None,
            Some(ThinkingLevel::High),
        )
        .unwrap();
        assert_eq!(first["prompt_cache_key"], repeated["prompt_cache_key"]);

        let mut other_conversation = messages.clone();
        let other_id = Uuid::new_v4();
        for message in &mut other_conversation {
            message.conversation_id = other_id;
        }
        let other = build_openai_responses_request(
            &agent,
            &provider,
            &other_conversation,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert_ne!(first["prompt_cache_key"], other["prompt_cache_key"]);
        assert_eq!(other["prompt_cache_key"], other_id.to_string());
    }

    #[test]
    fn stable_key_is_used_only_when_enabled_and_supported() {
        let (agent, messages) = fixture();
        let mut provider = test_provider();
        provider.prompt_cache.use_stable_key = true;

        let request = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert_ne!(
            request["prompt_cache_key"],
            messages[0].conversation_id.to_string()
        );
        assert!(
            request["prompt_cache_key"]
                .as_str()
                .unwrap()
                .starts_with("weiagent:")
        );

        provider.prompt_cache.mode = PromptCacheMode::Disabled;
        let disabled = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert!(disabled.get("prompt_cache_key").is_none());

        provider.prompt_cache.mode = PromptCacheMode::Explicit;
        let explicit = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert!(explicit.get("prompt_cache_key").is_none());
        // Explicit mode stays disabled until the provider declares support.
        assert!(explicit.get("prompt_cache_options").is_none());
        assert!(
            explicit["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item.get("content").and_then(|c| c.as_array()))
                .flatten()
                .all(|block| block.get("prompt_cache_breakpoint").is_none())
        );

        provider.cache_capabilities = Some(crate::llm::cache::ProviderCacheCapabilities {
            automatic_prefix_cache: true,
            explicit_breakpoints: true,
            stable_cache_key: true,
            supports_cache_retention: false,
        });
        let explicit = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert!(explicit.get("prompt_cache_key").is_some());
    }

    #[test]
    fn gpt56_requests_mark_explicit_breakpoints_and_options() {
        let (mut agent, messages) = fixture();
        agent.model_id = "gpt-5.6".to_string();
        let mut provider = test_provider();
        provider.prompt_cache.retention = Some("24h".to_string());
        provider.cache_capabilities = Some(crate::llm::cache::ProviderCacheCapabilities {
            automatic_prefix_cache: true,
            explicit_breakpoints: true,
            stable_cache_key: true,
            supports_cache_retention: true,
        });
        let request = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert_eq!(request["prompt_cache_options"]["mode"], "explicit");
        assert_eq!(request["prompt_cache_retention"], "24h");
        let user_blocks = request["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "message" && item["role"] == "user")
            .flat_map(|item| item["content"].as_array().into_iter().flatten())
            .collect::<Vec<_>>();
        assert!(
            user_blocks.iter().any(|block| {
                block.get("prompt_cache_breakpoint") == Some(&json!({ "mode": "explicit" }))
            }),
            "expected at least one user content breakpoint: {user_blocks:?}"
        );
    }

    #[test]
    fn omits_reasoning_when_thinking_is_off() {
        let (agent, messages) = fixture();
        let provider = test_provider();
        let request = build_openai_responses_request(
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
        assert!(request.get("reasoning").is_none());

        let request = build_openai_responses_request(
            &agent,
            &provider,
            &messages,
            false,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        assert!(request.get("reasoning").is_none());
    }

    #[test]
    fn parses_output_and_stream_events() {
        let output = parse_openai_responses_output(&json!({
            "usage": {
                "input_tokens": 120,
                "output_tokens": 30,
                "input_tokens_details": {"cached_tokens": 20}
            },
            "output": [
                {
                    "id":"rs-output",
                    "type":"reasoning",
                    "summary":[{"type":"summary_text","text":"considering"}],
                    "encrypted_content":"opaque-output"
                },
                {"type":"message","content":[{"type":"output_text","text":"hello"}]},
                {"type":"function_call","call_id":"call-r","name":"run","arguments":"{}"}
            ]
        }))
        .unwrap();
        assert_eq!(output.content, "hello");
        assert_eq!(output.reasoning_content, "considering");
        assert_eq!(output.openai_response_items.len(), 1);
        assert_eq!(
            output.openai_response_items[0]["encrypted_content"],
            "opaque-output"
        );
        assert_eq!(output.tool_calls[0].id, "call-r");
        assert_eq!(output.usage.input_tokens, 100);
        assert_eq!(output.usage.cache_read_tokens, 20);

        let mut parser = OpenAiResponsesStreamParser::new();
        let events =
            parser.parse_line(r#"data: {"type":"response.output_text.delta","delta":"hello"}"#);
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::TextDelta(text) if text == "hello"
        ));

        let events = parser
            .parse_line(r#"data: {"type":"response.reasoning_summary_text.delta","delta":"plan"}"#);
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::ReasoningDelta(text) if text == "plan"
        ));

        let events = parser.parse_line(
            r#"data: {"type":"response.output_item.done","item":{"id":"rs-stream","type":"reasoning","summary":[],"encrypted_content":"opaque-stream"}}"#,
        );
        assert!(matches!(
            events.into_iter().next().unwrap().unwrap(),
            StreamEvent::OpenAiResponseItem(item)
                if item["encrypted_content"] == "opaque-stream"
        ));
    }

    #[test]
    fn incomplete_response_is_an_error() {
        let error = parse_openai_responses_output(&json!({
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("incomplete"));
    }
}
