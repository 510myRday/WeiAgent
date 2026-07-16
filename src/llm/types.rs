use serde::{Deserialize, Serialize};

use crate::domain::{Agent, ChatMessage};

const MAX_CONTEXT_MESSAGES: usize = 200;
const MAX_CONTEXT_MESSAGE_BYTES: usize = 2 * 1024 * 1024;

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
    pub tool_calls: Vec<ToolCall>,
}

// ─── Structured Stream Events ───

#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallArgsDelta {
        index: usize,
        arguments: String,
    },
    FinishedWithToolCalls,
    Finished,
}

// ─── Request / Response Types ───

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(flatten)]
    pub extra_body: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
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
    extra_body: &serde_json::Value,
    tools: Option<Vec<ToolDefinition>>,
) -> ChatCompletionRequest {
    let mut request_messages = Vec::with_capacity(messages.len() + 1);
    request_messages.push(OpenAiMessage {
        role: "system".to_string(),
        content: system_prompt.to_string(),
        tool_calls: None,
        tool_call_id: None,
    });
    request_messages.extend(select_context_messages(messages).iter().map(|message| {
        OpenAiMessage {
            role: message.role.as_openai_role().to_string(),
            content: message.content.clone(),
            tool_calls: message.tool_calls.clone(),
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
        extra_body: extra_body.clone(),
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
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StreamDelta {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub struct StreamToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    #[allow(dead_code)] // Deserialized from OpenAI response but not directly inspected
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: Option<StreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub struct StreamFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ModelsListResponse {
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{ChatMessageStatus, ChatRole};

    fn message(role: ChatRole, content: impl Into<String>) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            role,
            content: content.into(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
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
}
