use std::collections::HashMap;

use reqwest::RequestBuilder;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    domain::{Agent, ChatMessage, ChatRole, ImageSource, ModelProvider, ThinkingLevel, TokenUsage},
    error::{AppError, AppResult},
    llm::{
        cache::{PromptCacheMode, effective_capabilities},
        http::StreamLineParser,
        protocol::common::{
            ProtocolAdapter, SseData, add_query_parameter, encode_path_segment, merge_extra_body,
            message_tool_calls, parse_json_object, parse_sse_data_line, validate_tool_calls,
        },
        types::{
            ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition,
            select_context_messages,
        },
    },
};

#[derive(Clone, Copy)]
pub(crate) struct GeminiAdapter;

impl ProtocolAdapter for GeminiAdapter {
    fn completion_url(&self, provider: &ModelProvider, agent: &Agent, stream: bool) -> String {
        let base = provider.base_url.trim_end_matches('/');
        if stream {
            format!(
                "{base}/models/{}:streamGenerateContent?alt=sse",
                encode_path_segment(&agent.model_id)
            )
        } else {
            format!(
                "{base}/models/{}:generateContent",
                encode_path_segment(&agent.model_id)
            )
        }
    }

    fn build_request(
        &self,
        _agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        _stream: bool,
        system_prompt: &str,
        extra_body: &Value,
        tools: Option<Vec<ToolDefinition>>,
        thinking_level: Option<ThinkingLevel>,
    ) -> AppResult<Value> {
        build_gemini_request(
            &_agent.model_id,
            provider,
            messages,
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
        let request = http.post(url).json(body);
        if api_key.trim().is_empty() {
            request
        } else {
            request.header("x-goog-api-key", api_key)
        }
    }

    fn authenticated_get(
        &self,
        http: &reqwest::Client,
        url: &str,
        api_key: &str,
    ) -> RequestBuilder {
        let request = http.get(url);
        if api_key.trim().is_empty() {
            request
        } else {
            request.header("x-goog-api-key", api_key)
        }
    }

    fn parse_response(&self, value: &Value) -> AppResult<ChatCompletionOutput> {
        parse_gemini_output(value)
    }

    fn stream_parser(&self) -> Box<dyn StreamLineParser> {
        Box::new(GeminiStreamParser::new())
    }

    fn append_models(&self, value: &Value, models: &mut Vec<String>) {
        models.extend(
            value["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|entry| {
                    entry["supportedGenerationMethods"]
                        .as_array()
                        .is_none_or(|methods| {
                            methods.iter().any(|method| {
                                matches!(
                                    method.as_str(),
                                    Some("generateContent" | "streamGenerateContent")
                                )
                            })
                        })
                })
                .filter_map(|entry| entry["name"].as_str())
                .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string()),
        );
    }

    fn next_models_url(
        &self,
        provider: &ModelProvider,
        value: &Value,
        pages: u8,
    ) -> AppResult<Option<String>> {
        if pages >= 20 {
            return Ok(None);
        }
        let Some(token) = value["nextPageToken"]
            .as_str()
            .filter(|token| !token.is_empty())
        else {
            return Ok(None);
        };
        add_query_parameter(&self.models_url(provider), "pageToken", token).map(Some)
    }
}

fn build_gemini_request(
    model_id: &str,
    provider: &ModelProvider,
    messages: &[ChatMessage],
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
    thinking_level: Option<ThinkingLevel>,
) -> AppResult<Value> {
    let selected = select_context_messages(messages);
    let message_calls = selected
        .iter()
        .map(|message| message_tool_calls(message))
        .collect::<AppResult<Vec<_>>>()?;
    let call_names: HashMap<&str, &str> = message_calls
        .iter()
        .flatten()
        .map(|call| (call.id.as_str(), call.function.name.as_str()))
        .collect();

    let mut contents: Vec<Value> = Vec::new();
    for (message, calls) in selected.iter().zip(&message_calls) {
        let (role, parts) = match message.role {
            ChatRole::User => {
                let mut parts = Vec::new();
                if !message.content.is_empty() {
                    parts.push(json!({"text": message.content}));
                }
                parts.extend(message.attachments.iter().map(
                    |attachment| match &attachment.source {
                        ImageSource::Base64 { data } => json!({
                            "inlineData": {
                                "mimeType": attachment.media_type,
                                "data": data,
                            }
                        }),
                        ImageSource::Url { url } => json!({
                            "fileData": {
                                "mimeType": attachment.media_type,
                                "fileUri": url,
                            }
                        }),
                        ImageSource::Local { path } => json!({
                            "fileData": { "mimeType": attachment.media_type, "fileUri": path },
                        }),
                    },
                ));
                ("user", parts)
            }
            ChatRole::Assistant => {
                let mut parts = Vec::new();
                if !message.content.is_empty() {
                    parts.push(json!({"text": message.content}));
                }
                for call in calls {
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
    if matches!(provider.prompt_cache.mode, PromptCacheMode::Explicit)
        && effective_capabilities(provider).explicit_breakpoints
        && let Some(cached_content) = provider.prompt_cache.gemini_cached_content.as_ref()
    {
        body.insert("cachedContent".to_string(), json!(cached_content));
    }
    let mut generation_config = extra_body
        .get("generationConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(level) = thinking_level.filter(|level| *level != ThinkingLevel::Off) {
        let thinking_config = if model_id.to_ascii_lowercase().contains("2.5") {
            json!({
                "thinkingBudget": match level {
                    ThinkingLevel::Low => 1024,
                    ThinkingLevel::Medium => 4096,
                    ThinkingLevel::High => 8192,
                    ThinkingLevel::XHigh => 16384,
                    ThinkingLevel::Off => unreachable!(),
                }
            })
        } else {
            json!({
                "thinkingLevel": match level {
                    ThinkingLevel::Low => "MINIMAL",
                    ThinkingLevel::Medium => "LOW",
                    ThinkingLevel::High => "MEDIUM",
                    ThinkingLevel::XHigh => "HIGH",
                    ThinkingLevel::Off => unreachable!(),
                }
            })
        };
        generation_config.insert("thinkingConfig".to_string(), thinking_config);
    }
    if !generation_config.is_empty() {
        body.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        body.insert(
            "tools".to_string(),
            json!([{
                "functionDeclarations": tools
                    .into_iter()
                    .map(|tool| json!({
                        "name": tool.function.name,
                        "description": tool.function.description,
                        "parameters": sanitize_gemini_schema(&tool.function.parameters),
                    }))
                    .collect::<Vec<_>>()
            }]),
        );
    }
    merge_extra_body(
        body,
        extra_body,
        &["systemInstruction", "contents", "tools", "generationConfig"],
    )
}

/// Gemini's native functionDeclarations schema is an OpenAPI 3.0 subset whose
/// protobuf parser rejects unknown fields outright, so full JSON Schema tool
/// definitions must be downgraded before sending: `oneOf` becomes the
/// supported `anyOf`, `const` becomes a single-value `enum`,
/// `type: [T, "null"]` becomes `type: T` plus `nullable`, tuple-form `items`
/// keeps its first entry, and every keyword outside the documented allowlist
/// (`additionalProperties`, `allOf`, `$ref`, exclusive bounds, ...) is
/// dropped. Constraints are loosened, never tightened, so any call that was
/// valid under the original schema still validates after the downgrade.
fn sanitize_gemini_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    for (key, value) in map {
        match key.as_str() {
            "oneOf" | "anyOf" => {
                if let Value::Array(branches) = value {
                    out.insert(
                        "anyOf".to_string(),
                        Value::Array(branches.iter().map(sanitize_gemini_schema).collect()),
                    );
                }
            }
            "const" => {
                // An explicit sibling `enum` wins over the derived one.
                out.entry("enum".to_string())
                    .or_insert_with(|| Value::Array(vec![value.clone()]));
            }
            "properties" => {
                if let Value::Object(properties) = value {
                    let sanitized: Map<String, Value> = properties
                        .iter()
                        .map(|(name, property)| (name.clone(), sanitize_gemini_schema(property)))
                        .collect();
                    out.insert("properties".to_string(), Value::Object(sanitized));
                }
            }
            "items" => {
                let item = if let Value::Array(entries) = value {
                    entries.first().cloned().unwrap_or(Value::Null)
                } else {
                    value.clone()
                };
                if !item.is_null() {
                    out.insert("items".to_string(), sanitize_gemini_schema(&item));
                }
            }
            "type" => match value {
                Value::Array(types) => {
                    if let Some(main) = types.iter().find(|entry| entry.as_str() != Some("null")) {
                        out.insert("type".to_string(), main.clone());
                    }
                    if types.iter().any(|entry| entry.as_str() == Some("null")) {
                        out.insert("nullable".to_string(), Value::Bool(true));
                    }
                }
                other => {
                    out.insert("type".to_string(), other.clone());
                }
            },
            "format" | "title" | "description" | "nullable" | "enum" | "maxItems" | "minItems"
            | "required" | "minProperties" | "maxProperties" | "minLength" | "maxLength"
            | "pattern" | "example" | "propertyOrdering" | "default" | "minimum" | "maximum" => {
                out.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    Value::Object(out)
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

fn normalize_gemini_tool_result(content: &str) -> Value {
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(value) => json!({"result": value}),
        Err(_) => json!({"result": content}),
    }
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
    validate_gemini_finish_reason(candidate["finishReason"].as_str())?;

    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut tool_calls = Vec::new();
    for part in candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(text) = part["text"].as_str() {
            if part["thought"].as_bool().unwrap_or(false) {
                reasoning_content.push_str(text);
            } else {
                content.push_str(text);
            }
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
        reasoning_content,
        openai_response_items: Vec::new(),
        tool_calls,
        usage: parse_gemini_usage(&value["usageMetadata"]),
    })
}

fn parse_gemini_usage(value: &Value) -> TokenUsage {
    let cached = value["cachedContentTokenCount"].as_u64().unwrap_or(0);
    let prompt = value["promptTokenCount"].as_u64().unwrap_or(0);
    TokenUsage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: value["candidatesTokenCount"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(value["thoughtsTokenCount"].as_u64().unwrap_or(0)),
        cache_write_tokens: 0,
        cache_read_tokens: cached,
        total_prompt_tokens: prompt,
        uncached_input_tokens: prompt.saturating_sub(cached),
        cache_hit_ratio: if prompt == 0 {
            0.0
        } else {
            cached as f64 / prompt as f64
        },
    }
}

fn validate_gemini_finish_reason(reason: Option<&str>) -> AppResult<()> {
    match reason {
        None | Some("STOP") => Ok(()),
        Some("MAX_TOKENS") => Err(AppError::Llm(
            "Gemini response stopped because the token limit was reached".to_string(),
        )),
        Some("SAFETY") => Err(AppError::Llm(
            "Gemini response was blocked by safety filters".to_string(),
        )),
        Some("RECITATION") => Err(AppError::Llm(
            "Gemini response was blocked for recitation".to_string(),
        )),
        Some("LANGUAGE") => Err(AppError::Llm(
            "Gemini response was blocked for unsupported language".to_string(),
        )),
        Some("BLOCKLIST") => Err(AppError::Llm(
            "Gemini response was blocked by blocklist".to_string(),
        )),
        Some("PROHIBITED_CONTENT") => Err(AppError::Llm(
            "Gemini response was blocked for prohibited content".to_string(),
        )),
        Some("SPII") => Err(AppError::Llm(
            "Gemini response was blocked for sensitive personal information".to_string(),
        )),
        Some("MALFORMED_FUNCTION_CALL") => Err(AppError::Llm(
            "Gemini response ended with a malformed function call".to_string(),
        )),
        Some("FINISH_REASON_UNSPECIFIED") => Err(AppError::Llm(
            "Gemini response ended with an unspecified finish reason".to_string(),
        )),
        Some(other) => Err(AppError::Llm(format!(
            "Gemini response ended with unsupported finishReason {other}"
        ))),
    }
}

struct GeminiStreamParser {
    saw_tool_calls: bool,
    finished: bool,
    next_tool_index: usize,
    usage: TokenUsage,
}

impl GeminiStreamParser {
    fn new() -> Self {
        Self {
            saw_tool_calls: false,
            finished: false,
            next_tool_index: 0,
            usage: TokenUsage::default(),
        }
    }

    fn parse_chunk(&mut self, value: &Value) -> Vec<AppResult<StreamEvent>> {
        if value.get("usageMetadata").is_some() {
            self.usage = parse_gemini_usage(&value["usageMetadata"]);
        }
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
                events.push(Ok(if part["thought"].as_bool().unwrap_or(false) {
                    StreamEvent::ReasoningDelta(text.to_string())
                } else {
                    StreamEvent::TextDelta(text.to_string())
                }));
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
        if let Some(reason) = candidate["finishReason"].as_str() {
            match validate_gemini_finish_reason(Some(reason)) {
                Ok(()) => events.extend(self.finish()),
                Err(error) => events.push(Err(error)),
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<AppResult<StreamEvent>> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
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

impl StreamLineParser for GeminiStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>> {
        match parse_sse_data_line(line) {
            Ok(SseData::Skip) => Vec::new(),
            Ok(SseData::Done) => self.finish(),
            Ok(SseData::Json(value)) => self.parse_chunk(&value),
            Err(error) => vec![Err(error)],
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::domain::ChatMessageStatus;

    fn fixture() -> Vec<ChatMessage> {
        let now = Utc::now();
        let agent_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        vec![
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id,
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
                agent_id,
                conversation_id,
                role: ChatRole::Tool,
                content: "{\"status\":\"ok\"}".to_string(),
                reasoning_content: None,
                openai_response_items: Vec::new(),
                status: ChatMessageStatus::Done,
                created_at: now,
                tool_calls: None,
                tool_call_id: Some("call-1".to_string()),
                attachments: Vec::new(),
                usage: None,
            },
        ]
    }

    fn test_provider() -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type: crate::domain::ProviderType::GoogleGemini,
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_keys: vec!["key".into()],
            key_strategy: Default::default(),
            models: vec!["gemini-3-pro".into()],
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
    fn request_maps_function_response() {
        let mut messages = fixture();
        let provider = test_provider();
        messages[0].reasoning_content = Some("saved-reasoning-must-not-be-sent".to_string());
        messages.insert(
            0,
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: messages[0].agent_id,
                conversation_id: messages[0].conversation_id,
                role: ChatRole::User,
                content: "inspect".to_string(),
                reasoning_content: None,
                openai_response_items: Vec::new(),
                status: ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: None,
                tool_call_id: None,
                attachments: vec![crate::domain::ImageAttachment {
                    id: "image-1".to_string(),
                    name: "sample.png".to_string(),
                    media_type: "image/png".to_string(),
                    is_mask: false,
                    source: ImageSource::Url {
                        url: "https://example.test/sample.png".to_string(),
                    },
                }],
                usage: None,
            },
        );
        let request = build_gemini_request(
            "gemini-3-pro",
            &provider,
            &messages,
            "system",
            &Value::Null,
            None,
            Some(ThinkingLevel::XHigh),
        )
        .unwrap();
        assert_eq!(
            request["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "HIGH"
        );
        assert!(
            request["contents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| part.get("functionResponse").is_some()))
        );
        assert!(
            request["contents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["parts"].as_array().unwrap().iter().any(|part| {
                        part["fileData"]["fileUri"] == "https://example.test/sample.png"
                    })
                })
        );
        assert!(
            !request
                .to_string()
                .contains("saved-reasoning-must-not-be-sent")
        );
    }

    #[test]
    fn omits_thinking_config_when_level_is_off() {
        let conversation_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id,
            conversation_id,
            role: ChatRole::User,
            content: "hi".to_string(),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }];
        let request = build_gemini_request(
            "gemini-3-pro",
            &test_provider(),
            &messages,
            "system",
            &Value::Null,
            None,
            Some(ThinkingLevel::Off),
        )
        .unwrap();
        assert!(
            request
                .get("generationConfig")
                .and_then(|cfg| cfg.get("thinkingConfig"))
                .is_none()
        );
    }
    #[test]
    fn parses_output_and_stream_events() {
        let output = parse_gemini_output(&json!({
            "usageMetadata": {
                "promptTokenCount": 120,
                "candidatesTokenCount": 30,
                "thoughtsTokenCount": 10,
                "cachedContentTokenCount": 20
            },
            "candidates": [{"content":{"parts":[
                {"thought":true,"text":"considering"},
                {"text":"hey"},
                {"functionCall":{"id":"call-g","name":"run","args":{"x":2}}}
            ]}}]
        }))
        .unwrap();
        assert_eq!(output.content, "hey");
        assert_eq!(output.reasoning_content, "considering");
        assert_eq!(output.tool_calls[0].id, "call-g");
        assert_eq!(output.usage.input_tokens, 100);
        assert_eq!(output.usage.output_tokens, 40);

        let mut parser = GeminiStreamParser::new();
        let events = parser.parse_line(
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
    fn abnormal_finish_reason_is_an_error() {
        let error = parse_gemini_output(&json!({
            "candidates": [{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"partial"}]}}]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("token limit"));

        let mut parser = GeminiStreamParser::new();
        let events = parser.parse_line(
            r#"data: {"candidates":[{"content":{"parts":[{"text":"partial"}]},"finishReason":"SAFETY"}]}"#,
        );
        assert!(matches!(
            events.first().unwrap().as_ref().unwrap(),
            StreamEvent::TextDelta(text) if text == "partial"
        ));
        assert!(events.last().unwrap().is_err());
    }

    #[test]
    fn schema_downgrade_covers_unsupported_keywords() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": {"const": "fast"},
                "value": {"type": ["string", "null"], "maxLength": 10},
                "ops": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {"a": {"type": "string"}}
                            },
                            {"type": "object", "properties": {"b": {"const": 2}}}
                        ]
                    }
                },
                "tuple": {"type": "array", "items": [{"type": "string"}, {"type": "integer"}]}
            },
            "required": ["mode"]
        });

        let sanitized = sanitize_gemini_schema(&schema);
        let text = sanitized.to_string();
        assert!(!text.contains("additionalProperties"));
        assert!(!text.contains("oneOf"));
        assert!(!text.contains("const"));
        assert_eq!(sanitized["properties"]["mode"]["enum"], json!(["fast"]));
        assert_eq!(sanitized["properties"]["value"]["type"], "string");
        assert_eq!(sanitized["properties"]["value"]["nullable"], true);
        assert_eq!(sanitized["properties"]["value"]["maxLength"], 10);
        let branches = sanitized["properties"]["ops"]["items"]["anyOf"]
            .as_array()
            .expect("oneOf branches downgraded to anyOf");
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[1]["properties"]["b"]["enum"], json!([2]));
        assert_eq!(sanitized["properties"]["tuple"]["items"]["type"], "string");
        assert_eq!(sanitized["required"], json!(["mode"]));
    }

    #[test]
    fn request_downgrades_builtin_tool_schemas() {
        let provider = test_provider();
        let mut config = crate::domain::AgentToolsConfig::default();
        config.terminal.enabled = true;
        let tools = crate::tools::registry::ToolRegistry::with_builtin_tools()
            .definitions(&config)
            .expect("builtin tools available");

        let request = build_gemini_request(
            "gemini-3-pro",
            &provider,
            &fixture(),
            "system",
            &Value::Null,
            Some(tools),
            None,
        )
        .unwrap();

        let declarations = request["tools"][0]["functionDeclarations"].to_string();
        assert!(declarations.contains("multi_edit_file"));
        assert!(declarations.contains("anyOf"));
        assert!(!declarations.contains("oneOf"));
        assert!(!declarations.contains("additionalProperties"));
        assert!(!declarations.contains("\"const\""));
    }
}
