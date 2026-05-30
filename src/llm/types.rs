use serde::{Deserialize, Serialize};

use crate::domain::{Agent, ChatMessage};

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

impl ToolDefinition {
    pub fn execute_terminal() -> Self {
        Self {
            kind: "function".to_string(),
            function: FunctionDefinition {
                name: "execute_terminal".to_string(),
                description: "Execute a shell command in the terminal.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command to execute" },
                        "cwd": { "type": "string", "description": "Optional working directory" }
                    },
                    "required": ["command"]
                }),
            },
        }
    }
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
    request_messages.extend(messages.iter().map(|message| OpenAiMessage {
        role: message.role.as_openai_role().to_string(),
        content: message.content.clone(),
        tool_calls: message.tool_calls.clone(),
        tool_call_id: if message.role.as_openai_role() == "tool" {
            message.tool_call_id.clone()
        } else {
            None
        },
    }));

    ChatCompletionRequest {
        model: agent.model_id.clone(),
        messages: request_messages,
        stream,
        tools,
        extra_body: extra_body.clone(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChoice {
    pub message: ChatCompletionMessage,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionMessage {
    #[serde(default)]
    pub content: String,
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
