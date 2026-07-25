use serde::{Deserialize, Serialize};

use crate::domain::{
    Agent, ChatMessage, ChatRole, ImageAttachment, ImageSource, ThinkingLevel, TokenUsage,
};

const MAX_CONTEXT_MESSAGES: usize = 200;
const MAX_CONTEXT_MESSAGE_BYTES: usize = 44 * 1024 * 1024;

// ─── Tool Calling Types ───

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ChatCompletionOutput {
    pub content: String,
    pub reasoning_content: String,
    pub openai_response_items: Vec<serde_json::Value>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
}

// ─── Structured Stream Events ───

#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
    OpenAiResponseItem(serde_json::Value),
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallArgsDelta {
        index: usize,
        arguments: String,
    },
    Usage(TokenUsage),
    FinishedWithToolCalls,
    Finished,
}

// ─── Request / Response Types ───

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

pub fn build_chat_request(
    agent: &Agent,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    tools: Option<Vec<ToolDefinition>>,
    thinking_level: Option<ThinkingLevel>,
) -> ChatCompletionRequest {
    let mut request_messages = Vec::with_capacity(messages.len() + 1);
    request_messages.push(OpenAiMessage {
        role: "system".to_string(),
        content: serde_json::Value::String(system_prompt.to_string()),
        tool_calls: None,
        tool_call_id: None,
    });
    request_messages.extend(select_context_messages(messages).iter().map(|message| {
        OpenAiMessage {
            role: message.role.as_openai_role().to_string(),
            content: openai_message_content(message),
            tool_calls: message.tool_calls.as_ref().map(strip_tool_call_ui_fields),
            tool_call_id: if message.role.as_openai_role() == "tool" {
                message.tool_call_id.clone()
            } else {
                None
            },
        }
    }));

    ChatCompletionRequest {
        model: agent.model_id.clone(),
        messages: request_messages,
        stream,
        tools,
        reasoning_effort: thinking_level
            .and_then(thinking_effort_str)
            .map(ToString::to_string),
        stream_options: stream.then(|| serde_json::json!({"include_usage": true})),
    }
}

/// OpenAI-style reasoning `effort` string; `None` when thinking is off.
pub(crate) fn thinking_effort_str(level: ThinkingLevel) -> Option<&'static str> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Low => Some("low"),
        ThinkingLevel::Medium => Some("medium"),
        ThinkingLevel::High => Some("high"),
        ThinkingLevel::XHigh => Some("xhigh"),
    }
}

/// Strip UI-only fields (e.g. permission_mode) before sending tool_calls to the model.
/// DB/history may keep them for the frontend; the model request must stay clean for
/// prompt-cache stability and protocol compatibility.
pub(crate) fn strip_tool_call_ui_fields(tool_calls: &serde_json::Value) -> serde_json::Value {
    let Some(calls) = tool_calls.as_array() else {
        return tool_calls.clone();
    };
    let cleaned = calls
        .iter()
        .map(|call| {
            let Some(obj) = call.as_object() else {
                return call.clone();
            };
            let mut cleaned = obj.clone();
            cleaned.remove("permission_mode");
            serde_json::Value::Object(cleaned)
        })
        .collect();
    serde_json::Value::Array(cleaned)
}

fn openai_message_content(message: &ChatMessage) -> serde_json::Value {
    if message.role != ChatRole::User || message.attachments.is_empty() {
        return serde_json::Value::String(message.content.clone());
    }
    let mut content = Vec::new();
    if !message.content.is_empty() {
        content.push(serde_json::json!({
            "type": "text",
            "text": message.content,
        }));
    }
    content.extend(message.attachments.iter().map(|attachment| {
        serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": openai_image_url(attachment),
            },
        })
    }));
    serde_json::Value::Array(content)
}

pub(crate) fn openai_image_url(attachment: &ImageAttachment) -> String {
    match &attachment.source {
        ImageSource::Base64 { data } => {
            format!("data:{};base64,{data}", attachment.media_type)
        }
        ImageSource::Url { url } => url.clone(),
    }
}

pub(crate) fn select_context_messages(messages: &[ChatMessage]) -> Vec<&ChatMessage> {
    let groups = context_message_groups(messages);
    let mut selected_groups = Vec::new();
    let mut bytes = 0_usize;
    let mut count = 0_usize;

    for group in groups.iter().rev() {
        let group_len = group.len();
        let group_bytes = group
            .iter()
            .map(|message| context_message_bytes(message))
            .sum();
        if count.saturating_add(group_len) > MAX_CONTEXT_MESSAGES
            || bytes.saturating_add(group_bytes) > MAX_CONTEXT_MESSAGE_BYTES
        {
            break;
        }
        bytes += group_bytes;
        count += group_len;
        selected_groups.push(group);
    }

    let mut selected = Vec::new();
    for group in selected_groups.into_iter().rev() {
        selected.extend(group.iter().copied());
    }

    if let Some(first_user) = selected
        .iter()
        .position(|message| message.role.as_openai_role() == "user")
    {
        selected.drain(..first_user);
    }
    selected
}

fn context_message_groups(messages: &[ChatMessage]) -> Vec<Vec<&ChatMessage>> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if message.role.as_openai_role() == "assistant" && message.tool_calls.is_some() {
            let mut group = vec![message];
            index += 1;
            while index < messages.len() && messages[index].role.as_openai_role() == "tool" {
                group.push(&messages[index]);
                index += 1;
            }
            groups.push(group);
        } else if message.role.as_openai_role() == "tool" {
            index += 1;
        } else {
            groups.push(vec![message]);
            index += 1;
        }
    }
    groups
}

fn context_message_bytes(message: &ChatMessage) -> usize {
    message.content.len()
        + message
            .openai_response_items
            .iter()
            .map(|item| item.to_string().len())
            .sum::<usize>()
        + message
            .attachments
            .iter()
            .map(|attachment| match &attachment.source {
                ImageSource::Base64 { data } => data.len(),
                ImageSource::Url { url } => url.len(),
            })
            .sum::<usize>()
        + message
            .tool_call_id
            .as_ref()
            .map_or(0, |tool_call_id| tool_call_id.len())
        + message
            .tool_calls
            .as_ref()
            .map_or(0, |tool_calls| tool_calls.to_string().len())
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatCompletionChoice>,
    #[serde(default)]
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChoice {
    pub message: ChatCompletionMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    #[serde(default)]
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StreamDelta {
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub struct StreamToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub function: Option<StreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub struct StreamFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenAiUsage {
    #[serde(default, alias = "input_tokens")]
    pub prompt_tokens: u64,
    #[serde(default, alias = "output_tokens")]
    pub completion_tokens: u64,
    #[serde(default, alias = "input_tokens_details")]
    pub prompt_tokens_details: Option<OpenAiInputTokenDetails>,
    #[serde(default, alias = "cache_hit_tokens")]
    pub prompt_cache_hit_tokens: u64,
    #[serde(default)]
    pub prompt_cache_miss_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenAiInputTokenDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

impl OpenAiUsage {
    pub fn normalized(&self) -> TokenUsage {
        let cached = self
            .prompt_tokens_details
            .as_ref()
            .map_or(0, |details| details.cached_tokens)
            .max(self.prompt_cache_hit_tokens)
            .max(self.cache_read_tokens)
            .max(self.cached_tokens);
        let uncached = if self.prompt_cache_miss_tokens > 0 {
            self.prompt_cache_miss_tokens
        } else {
            self.prompt_tokens.saturating_sub(cached)
        };
        TokenUsage {
            input_tokens: uncached,
            uncached_input_tokens: uncached,
            total_prompt_tokens: self.prompt_tokens.max(uncached.saturating_add(cached)),
            output_tokens: self.completion_tokens,
            cache_write_tokens: 0,
            cache_read_tokens: cached,
            cache_hit_ratio: ratio(cached, uncached),
        }
    }
}

fn ratio(hit: u64, uncached: u64) -> f64 {
    let denominator = hit.saturating_add(uncached);
    if denominator == 0 {
        0.0
    } else {
        hit as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{Agent, ChatMessageStatus, ChatRole};

    #[test]
    fn deepseek_cache_usage_normalizes_hit_and_miss_tokens() {
        let usage: OpenAiUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1800,
            "completion_tokens": 20,
            "prompt_cache_hit_tokens": 1200,
            "prompt_cache_miss_tokens": 600
        }))
        .unwrap();
        let normalized = usage.normalized();
        assert_eq!(normalized.input_tokens, 600);
        assert_eq!(normalized.cache_read_tokens, 1200);
        assert_eq!(normalized.total_prompt_tokens, 1800);
        assert!((normalized.cache_hit_ratio - (2.0 / 3.0)).abs() < f64::EPSILON);
    }

    fn message(role: ChatRole, content: impl Into<String>) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            role,
            content: content.into(),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }
    }

    #[test]
    fn context_budget_counts_tool_calls_and_keeps_valid_start() {
        let user = message(ChatRole::User, "old request");
        let mut assistant = message(ChatRole::Assistant, "");
        assistant.tool_calls = Some(json!([{
            "id": "call-1",
            "type": "function",
            "function": {
                "name": "run",
                "arguments": "x".repeat(MAX_CONTEXT_MESSAGE_BYTES)
            }
        }]));
        let mut tool = message(ChatRole::Tool, "ok");
        tool.tool_call_id = Some("call-1".to_string());
        let latest = message(ChatRole::User, "latest request");

        let messages = vec![user.clone(), assistant, tool, latest.clone()];
        let selected = select_context_messages(&messages);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, latest.id);
    }

    #[test]
    fn context_selection_keeps_assistant_tool_group_together() {
        let user = message(ChatRole::User, "run it");
        let mut assistant = message(ChatRole::Assistant, "");
        assistant.tool_calls = Some(json!([{
            "id": "call-1",
            "type": "function",
            "function": {"name": "run", "arguments": "{}"}
        }]));
        let mut tool = message(ChatRole::Tool, "ok");
        tool.tool_call_id = Some("call-1".to_string());

        let messages = vec![user.clone(), assistant.clone(), tool.clone()];
        let selected = select_context_messages(&messages);

        assert_eq!(selected.len(), 3);
        assert_eq!(selected[0].id, user.id);
        assert_eq!(selected[1].id, assistant.id);
        assert_eq!(selected[2].id, tool.id);
    }

    #[test]
    fn strips_permission_mode_from_tool_calls_sent_to_model() {
        let user = message(ChatRole::User, "run it");
        let mut assistant = message(ChatRole::Assistant, "");
        assistant.tool_calls = Some(json!([{
            "id": "call-1",
            "type": "function",
            "permission_mode": "request_approval",
            "function": {"name": "execute_terminal", "arguments": "{}"}
        }]));
        let mut tool = message(ChatRole::Tool, "ok");
        tool.tool_call_id = Some("call-1".to_string());
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "a".into(),
            description: "d".into(),
            model_provider_id: Uuid::new_v4(),
            model_id: "m".into(),
            is_builtin: false,
            avatar_icon: "bot".into(),
            avatar_color: "#000".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let request = build_chat_request(
            &agent,
            &[user, assistant, tool],
            false,
            "system",
            None,
            None,
        );
        let tool_calls = request.messages[2].tool_calls.as_ref().unwrap();
        assert!(tool_calls[0].get("permission_mode").is_none());
        assert_eq!(tool_calls[0]["id"], "call-1");
        assert_eq!(tool_calls[0]["function"]["name"], "execute_terminal");
    }

    #[test]
    fn strip_tool_call_ui_fields_preserves_core_shape() {
        let cleaned = strip_tool_call_ui_fields(&json!([{
            "id": "c",
            "type": "function",
            "permission_mode": "full_access",
            "function": {"name": "web_search", "arguments": "{\"query\":\"x\"}"}
        }]));
        assert_eq!(
            cleaned,
            json!([{
                "id": "c",
                "type": "function",
                "function": {"name": "web_search", "arguments": "{\"query\":\"x\"}"}
            }])
        );
    }
}
