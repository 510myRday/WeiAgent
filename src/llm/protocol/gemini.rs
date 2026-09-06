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
            ProtocolAdapter, SseData, add_query_parameter, append_role_items, encode_path_segment,
            merge_extra_body, message_tool_calls, parse_json_object, parse_sse_data_line,
            unwrap_provider_response_item, validate_tool_calls, wrap_provider_response_item,
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
                let replayed_parts = message
                    .openai_response_items
                    .iter()
                    .filter_map(|item| unwrap_provider_response_item(item, "gemini"))
                    .cloned()
                    .collect::<Vec<_>>();
                if replayed_parts.is_empty() {
                    if !message.content.is_empty() {
                        parts.push(json!({"text": message.content}));
                    }
                    for call in calls {
                        let args = parse_json_object(
                            &call.function.arguments,
                            "stored tool call arguments",
                        )?;
                        parts.push(json!({
                            "functionCall": {
                                "name": call.function.name,
                                "args": args,
                            }
                        }));
                    }
                } else {
                    parts.extend(replayed_parts);
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
        append_role_items(&mut contents, role, "parts", parts);
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
    if let Some(level) = thinking_level.filter(|level| *level != ThinkingLevel::Off)
        && let Some(thinking_config) = gemini_thinking_config(model_id, level)
    {
        let mut merged_thinking_config = generation_config
            .remove("thinkingConfig")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        if let Some(thinking_config) = thinking_config.as_object() {
            merged_thinking_config.extend(thinking_config.clone());
        }
        generation_config.insert(
            "thinkingConfig".to_string(),
            Value::Object(merged_thinking_config),
        );
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
                        "parameters": sanitize_gemini_parameters(&tool.function.parameters),
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

fn gemini_thinking_config(model_id: &str, level: ThinkingLevel) -> Option<Value> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("2.5") {
        return Some(json!({
            "includeThoughts": true,
            "thinkingBudget": match level {
                ThinkingLevel::Low => 1024,
                ThinkingLevel::Medium => 4096,
                ThinkingLevel::High => 8192,
                ThinkingLevel::XHigh => 16384,
                ThinkingLevel::Off => unreachable!(),
            }
        }));
    }

    let major = lower
        .strip_prefix("gemini-")
        .and_then(|version| {
            let digits: String = version.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty()).then_some(digits)
        })
        .and_then(|version| version.parse::<u32>().ok());
    (major >= Some(3)).then(|| {
        json!({
            "includeThoughts": true,
            "thinkingLevel": match level {
                ThinkingLevel::Low => "MINIMAL",
                ThinkingLevel::Medium => "LOW",
                ThinkingLevel::High => "MEDIUM",
                ThinkingLevel::XHigh => "HIGH",
                ThinkingLevel::Off => unreachable!(),
            }
        })
    })
}

/// Gemini's native functionDeclarations schema is an OpenAPI 3.0 subset.
/// Tool definitions originate from JSON Schema, so unsupported keywords are
/// removed while union branches are made self-contained Gemini object schemas.
fn sanitize_gemini_schema(schema: &Value) -> Value {
    sanitize_gemini_schema_with_inherited_properties(schema)
}

fn sanitize_gemini_parameters(schema: &Value) -> Value {
    let sanitized = sanitize_gemini_schema(schema);
    let Value::Object(mut object) = sanitized else {
        return json!({"type": "object", "properties": {}});
    };
    if !object.contains_key("type") && !object.contains_key("anyOf") {
        object.insert("type".to_string(), json!("object"));
    }
    Value::Object(object)
}

fn sanitize_gemini_schema_with_inherited_properties(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };

    let (mut schema_type, nullable_from_type) = normalized_gemini_type(map.get("type"));
    if schema_type.is_none() {
        schema_type = if map.get("properties").is_some_and(Value::is_object)
            || map.get("required").is_some()
            || map.get("propertyOrdering").is_some()
        {
            Some("OBJECT")
        } else if map.get("items").is_some() {
            Some("ARRAY")
        } else if map.get("enum").is_some() || map.get("const").and_then(Value::as_str).is_some() {
            Some("STRING")
        } else {
            None
        };
    }

    let mut out = Map::new();
    if let Some(schema_type) = schema_type {
        out.insert(
            "type".to_string(),
            Value::String(schema_type.to_ascii_lowercase()),
        );
    }
    if nullable_from_type
        || map
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        out.insert("nullable".to_string(), Value::Bool(true));
    }

    if let Some(Value::Object(properties)) = map.get("properties") {
        let sanitized = properties
            .iter()
            .map(|(name, property)| (name.clone(), sanitize_gemini_schema(property)))
            .collect();
        out.insert("properties".to_string(), Value::Object(sanitized));
    }
    if let Some(Value::Array(required)) = map.get("required")
        && schema_type == Some("OBJECT")
    {
        let properties = out.get("properties").and_then(Value::as_object);
        let required: Vec<Value> = required
            .iter()
            .filter(|name| {
                name.as_str().is_some_and(|name| {
                    properties.is_none_or(|properties| properties.contains_key(name))
                })
            })
            .cloned()
            .collect();
        if !required.is_empty() {
            out.insert("required".to_string(), Value::Array(required));
        }
    }

    if let Some(branches) = map
        .get("oneOf")
        .or_else(|| map.get("anyOf"))
        .and_then(Value::as_array)
    {
        let parent_properties = map.get("properties").and_then(Value::as_object);
        let branches = branches
            .iter()
            .map(|branch| {
                let branch = prepare_gemini_union_branch(branch, parent_properties);
                sanitize_gemini_schema_with_inherited_properties(&branch)
            })
            .collect();
        out.insert("anyOf".to_string(), Value::Array(branches));
    }

    if let Some(Value::Array(entries)) = map.get("items") {
        if let Some(item) = entries.first() {
            out.insert("items".to_string(), sanitize_gemini_schema(item));
        }
    } else if let Some(item) = map.get("items") {
        out.insert("items".to_string(), sanitize_gemini_schema(item));
    }

    for key in ["description", "title", "format", "default", "example"] {
        if let Some(value) = map.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    if let Some(Value::Array(enum_values)) = map.get("enum")
        && schema_type == Some("STRING")
        && enum_values.iter().all(Value::is_string)
    {
        out.insert("enum".to_string(), Value::Array(enum_values.clone()));
    } else if let Some(Value::String(const_value)) = map.get("const")
        && schema_type == Some("STRING")
        && !out.contains_key("enum")
    {
        out.insert("enum".to_string(), json!([const_value]));
    }

    if schema_type == Some("ARRAY") {
        copy_schema_keyword(&mut out, map, "minItems");
        copy_schema_keyword(&mut out, map, "maxItems");
    }
    if schema_type == Some("OBJECT") {
        copy_schema_keyword(&mut out, map, "minProperties");
        copy_schema_keyword(&mut out, map, "maxProperties");
        copy_schema_keyword(&mut out, map, "propertyOrdering");
    }
    if schema_type == Some("STRING") {
        copy_schema_keyword(&mut out, map, "minLength");
        copy_schema_keyword(&mut out, map, "maxLength");
        copy_schema_keyword(&mut out, map, "pattern");
    }
    if matches!(schema_type, Some("INTEGER" | "NUMBER")) {
        copy_schema_keyword(&mut out, map, "minimum");
        copy_schema_keyword(&mut out, map, "maximum");
    }
    Value::Object(out)
}

fn normalized_gemini_type(value: Option<&Value>) -> (Option<&'static str>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    let types: Vec<&str> = match value {
        Value::String(value) => vec![value.as_str()],
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let nullable = types.iter().any(|value| value.eq_ignore_ascii_case("null"));
    let main = types
        .iter()
        .find(|value| !value.eq_ignore_ascii_case("null"))
        .and_then(|value| match value.to_ascii_uppercase().as_str() {
            "STRING" => Some("STRING"),
            "NUMBER" => Some("NUMBER"),
            "INTEGER" => Some("INTEGER"),
            "BOOLEAN" => Some("BOOLEAN"),
            "ARRAY" => Some("ARRAY"),
            "OBJECT" => Some("OBJECT"),
            "NULL" => Some("NULL"),
            _ => None,
        });
    (main, nullable)
}

fn prepare_gemini_union_branch(
    branch: &Value,
    inherited_properties: Option<&Map<String, Value>>,
) -> Value {
    let Some(branch) = branch.as_object() else {
        return branch.clone();
    };
    let mut prepared = branch.clone();
    let branch_is_object = branch
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("object"));
    let needs_object_shape =
        branch_is_object || branch.get("properties").is_some() || branch.get("required").is_some();
    if needs_object_shape {
        prepared
            .entry("type".to_string())
            .or_insert_with(|| json!("object"));
    }
    if needs_object_shape && let Some(inherited) = inherited_properties {
        let mut properties = inherited.clone();
        if let Some(Value::Object(branch_properties)) = branch.get("properties") {
            for (name, property) in branch_properties {
                let merged = properties
                    .get(name)
                    .map(|parent| merge_schema_defaults(parent, property))
                    .unwrap_or_else(|| property.clone());
                properties.insert(name.clone(), merged);
            }
        }
        prepared.insert("properties".to_string(), Value::Object(properties));
    }
    Value::Object(prepared)
}

fn merge_schema_defaults(base: &Value, override_value: &Value) -> Value {
    let (Some(base), Some(override_value)) = (base.as_object(), override_value.as_object()) else {
        return override_value.clone();
    };
    let mut merged = base.clone();
    for (key, value) in override_value {
        merged.insert(key.clone(), value.clone());
    }
    Value::Object(merged)
}

fn copy_schema_keyword(out: &mut Map<String, Value>, map: &Map<String, Value>, key: &str) {
    if let Some(value) = map.get(key) {
        out.insert(key.to_string(), value.clone());
    }
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
    let response_items = candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .map(|part| wrap_provider_response_item("gemini", part))
        .collect();
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
        openai_response_items: response_items,
        tool_calls,
        usage: parse_gemini_usage(value),
    })
}

fn parse_gemini_usage(value: &Value) -> TokenUsage {
    let usage = value
        .get("usageMetadata")
        .or_else(|| value.get("usage"))
        .unwrap_or(value);
    let cached = gemini_usage_number(
        usage,
        &[
            "cachedContentTokenCount",
            "cached_content_token_count",
            "cached_tokens",
            "total_cached_tokens",
            "cache_read_tokens",
        ],
    );
    let prompt = gemini_usage_number(
        usage,
        &[
            "promptTokenCount",
            "prompt_token_count",
            "prompt_tokens",
            "inputTokenCount",
            "input_tokens",
            "total_prompt_tokens",
        ],
    );
    TokenUsage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: gemini_usage_number(
            usage,
            &[
                "candidatesTokenCount",
                "candidates_token_count",
                "outputTokenCount",
                "output_tokens",
            ],
        )
        .saturating_add(gemini_usage_number(
            usage,
            &["thoughtsTokenCount", "thoughts_token_count"],
        )),
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

fn gemini_usage_number(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| {
            let value = value.get(*key)?;
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
                .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
                .or_else(|| value.as_str()?.parse().ok())
        })
        .unwrap_or(0)
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
        if value.get("usageMetadata").is_some() || value.get("usage").is_some() {
            self.usage = parse_gemini_usage(value);
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
            events.push(Ok(StreamEvent::OpenAiResponseItem(
                wrap_provider_response_item("gemini", part.clone()),
            )));
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
    use crate::tools::registry::AgentTool;

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
                shortcut_actions: Vec::new(),
                system_event: None,
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
                shortcut_actions: Vec::new(),
                system_event: None,
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
            api_key_labels: vec![],
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
                shortcut_actions: Vec::new(),
                system_event: None,
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
            shortcut_actions: Vec::new(),
            system_event: None,
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
    fn requests_thought_summaries_when_thinking_is_enabled() {
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
            shortcut_actions: Vec::new(),
            system_event: None,
        }];
        let request = build_gemini_request(
            "gemini-2.5-flash",
            &test_provider(),
            &messages,
            "system",
            &json!({"generationConfig": {"temperature": 0.2}}),
            None,
            Some(ThinkingLevel::High),
        )
        .unwrap();
        assert_eq!(request["generationConfig"]["temperature"], 0.2);
        assert_eq!(
            request["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
        assert_eq!(
            request["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            8192
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
        assert_eq!(output.usage.cache_read_tokens, 20);

        let mut parser = GeminiStreamParser::new();
        let events = parser.parse_line(
            r#"data: {"candidates":[{"content":{"parts":[{"thought":true,"text":"plan"}]}}]}"#,
        );
        assert!(matches!(
            events.first().unwrap().as_ref().unwrap(),
            StreamEvent::ReasoningDelta(text) if text == "plan"
        ));
        let events = parser.parse_line(
            r#"data: {"candidates":[{"content":{"parts":[{"text":"gem"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":30,"thoughtsTokenCount":10,"cachedContentTokenCount":20}}"#,
        );
        assert!(matches!(
            events.first().unwrap().as_ref().unwrap(),
            StreamEvent::TextDelta(text) if text == "gem"
        ));
        assert!(events.iter().any(|event| matches!(
            event.as_ref().unwrap(),
            StreamEvent::Usage(usage) if usage.cache_read_tokens == 20
        )));
        assert!(matches!(
            events.last().unwrap().as_ref().unwrap(),
            StreamEvent::Finished
        ));
    }

    #[test]
    fn parses_interactions_style_usage_aliases() {
        let output = parse_gemini_usage(&json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 12,
                "total_cached_tokens": 60
            }
        }));
        assert_eq!(output.input_tokens, 40);
        assert_eq!(output.output_tokens, 12);
        assert_eq!(output.cache_read_tokens, 60);
        assert_eq!(output.total_prompt_tokens, 100);
    }

    #[test]
    fn replays_raw_parts_with_thought_signature() {
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            role: ChatRole::Assistant,
            content: "done".to_string(),
            reasoning_content: Some("plan".to_string()),
            openai_response_items: vec![
                wrap_provider_response_item("gemini", json!({"thought":true,"text":"plan"})),
                wrap_provider_response_item(
                    "gemini",
                    json!({
                        "functionCall": {
                            "name": "run",
                            "args": {"x": 1}
                        },
                        "thoughtSignature": "SIG-G"
                    }),
                ),
            ],
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: Some(json!([{
                "id": "call-g",
                "type": "function",
                "function": {"name": "run", "arguments": "{\"x\":1}"}
            }])),
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
            shortcut_actions: Vec::new(),
            system_event: None,
        }];
        let request = build_gemini_request(
            "gemini-3-pro",
            &test_provider(),
            &messages,
            "system",
            &Value::Null,
            None,
            None,
        )
        .unwrap();
        let parts = request["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["thought"], true);
        assert_eq!(parts[1]["thoughtSignature"], "SIG-G");
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
        assert!(branches[1]["properties"]["b"]["enum"].is_null());
        assert_eq!(sanitized["properties"]["tuple"]["items"]["type"], "string");
        assert_eq!(sanitized["required"], json!(["mode"]));
    }

    #[test]
    fn task_planning_union_branches_are_gemini_objects() {
        let definition = crate::tools::task_planning::TaskPlanningTool
            .definition(&crate::domain::AgentToolsConfig::default());
        let sanitized = sanitize_gemini_parameters(&definition.function.parameters);
        let branches = sanitized["anyOf"]
            .as_array()
            .expect("task planning oneOf downgraded to anyOf");

        assert_eq!(branches[0]["type"], "object");
        assert_eq!(branches[0]["properties"]["explanation"]["type"], "string");
        assert_eq!(branches[0]["properties"]["plan"]["type"], "array");
        assert_eq!(
            branches[0]["required"],
            json!(["action", "explanation", "plan"])
        );
        assert_eq!(branches[1]["type"], "object");
        assert_eq!(branches[1]["properties"]["planning_id"]["type"], "string");
        assert_eq!(
            branches[1]["required"],
            json!(["action", "planning_id", "item", "status"])
        );
    }

    #[test]
    fn thinking_config_is_only_sent_to_supported_model_families() {
        assert!(gemini_thinking_config("gemini-2.5-flash", ThinkingLevel::High).is_some());
        assert!(gemini_thinking_config("gemini-3-pro", ThinkingLevel::High).is_some());
        assert!(gemini_thinking_config("gemini-2.0-flash", ThinkingLevel::High).is_none());
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
        assert!(declarations.contains("multi_file_operation"));
        assert!(declarations.contains("anyOf"));
        assert!(!declarations.contains("oneOf"));
        assert!(!declarations.contains("additionalProperties"));
        assert!(!declarations.contains("\"const\""));
    }
}
