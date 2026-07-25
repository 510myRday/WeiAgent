use std::sync::Arc;

use async_trait::async_trait;
use reqwest::RequestBuilder;
use serde_json::Value;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider, ThinkingLevel},
    error::{AppError, AppResult},
    llm::{
        cache::{apply_openai_fields, apply_provider_extra, build_context},
        client::{ChatClient, ChatStream, RequestOptions},
        http::StreamLineParser,
        protocol::common::{
            ProtocolAdapter, ProtocolClient, bearer_get, bearer_post, default_models,
        },
        sse::{meaningful_finish_reason, openai_finish_event, parse_sse_line},
        types::{
            ChatCompletionOutput, ChatCompletionResponse, StreamEvent, ToolDefinition,
            build_chat_request,
        },
    },
    storage::LlmLogger,
};

#[derive(Clone)]
pub struct OpenAiChatClient {
    inner: ProtocolClient<OpenAiChatAdapter>,
}

impl OpenAiChatClient {
    pub fn new() -> Self {
        Self {
            inner: ProtocolClient::new(OpenAiChatAdapter),
        }
    }

    pub fn with_logger(mut self, logger: Arc<LlmLogger>) -> Self {
        self.inner = self.inner.with_logger(logger);
        self
    }
}

impl Default for OpenAiChatClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChatClient for OpenAiChatClient {
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
struct OpenAiChatAdapter;

impl ProtocolAdapter for OpenAiChatAdapter {
    fn completion_url(&self, provider: &ModelProvider, _agent: &Agent, _stream: bool) -> String {
        format!(
            "{}/chat/completions",
            provider.base_url.trim_end_matches('/')
        )
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
        build_openai_chat_request(
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
        parse_openai_chat_output(value)
    }

    fn stream_parser(&self) -> Box<dyn StreamLineParser> {
        Box::new(OpenAiChatStreamParser)
    }

    fn append_models(&self, value: &Value, models: &mut Vec<String>) {
        default_models(value, models);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_openai_chat_request(
    agent: &Agent,
    provider: &ModelProvider,
    messages: &[ChatMessage],
    stream: bool,
    system_prompt: &str,
    extra_body: &Value,
    tools: Option<Vec<ToolDefinition>>,
    thinking_level: Option<ThinkingLevel>,
) -> AppResult<Value> {
    let mut request = build_chat_request(
        agent,
        messages,
        stream,
        system_prompt,
        extra_body,
        tools,
        thinking_level,
    );
    let context = build_context(
        provider,
        &agent.model_id,
        agent.id,
        "openai_chat",
        system_prompt,
        request.tools.as_deref(),
    );
    if let Value::Object(mut extra) = request.extra_body {
        apply_openai_fields(&mut extra, provider, &context);
        apply_provider_extra(&mut extra, &context);
        request.extra_body = Value::Object(extra);
    }
    serde_json::to_value(&request).map_err(|error| AppError::Llm(error.to_string()))
}

fn parse_openai_chat_output(value: &Value) -> AppResult<ChatCompletionOutput> {
    let response: ChatCompletionResponse =
        serde_json::from_value(value.clone()).map_err(|error| AppError::Llm(error.to_string()))?;
    let usage = response
        .usage
        .as_ref()
        .map(crate::llm::types::OpenAiUsage::normalized)
        .unwrap_or_default();
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Llm("empty model response".to_string()))?;

    if let Some(reason) = meaningful_finish_reason(choice.finish_reason.as_deref()) {
        openai_finish_event(reason)?;
    }

    Ok(ChatCompletionOutput {
        content: choice.message.content.unwrap_or_default(),
        reasoning_content: choice.message.reasoning_content.unwrap_or_default(),
        tool_calls: choice.message.tool_calls.unwrap_or_default(),
        usage,
    })
}

/// Stateless parser over the shared OpenAI-compatible SSE line format
/// (including SenseNova / ModelScope quirks handled in `parse_sse_line`).
struct OpenAiChatStreamParser;

impl StreamLineParser for OpenAiChatStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AppResult<StreamEvent>> {
        parse_sse_line(line)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{ChatMessageStatus, ChatRole};

    #[test]
    fn builds_openai_chat_completion_request() {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes and reviews code.".to_string(),
            model_provider_id: Uuid::new_v4(),
            model_id: "gpt-4o-mini".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: Uuid::new_v4(),
            role: ChatRole::User,
            content: "hello".to_string(),
            reasoning_content: Some("saved-reasoning-must-not-be-sent".to_string()),
            status: ChatMessageStatus::Done,
            created_at: now,
            tool_calls: None,
            tool_call_id: None,
            attachments: vec![crate::domain::ImageAttachment {
                id: "image-1".to_string(),
                name: "sample.png".to_string(),
                media_type: "image/png".to_string(),
                source: crate::domain::ImageSource::Base64 {
                    data: "aGVsbG8=".to_string(),
                },
            }],
            usage: None,
        }];

        let request = build_chat_request(
            &agent,
            &messages,
            true,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
            Some(crate::domain::ThinkingLevel::High),
        );

        assert_eq!(request.model, "gpt-4o-mini");
        assert!(request.stream);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            request.messages[1].content[1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert_eq!(
            request.stream_options.as_ref().unwrap()["include_usage"],
            true
        );
        assert!(
            !serde_json::to_string(&request)
                .unwrap()
                .contains("saved-reasoning-must-not-be-sent")
        );
    }

    #[test]
    fn omits_reasoning_effort_when_thinking_is_off() {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes and reviews code.".to_string(),
            model_provider_id: Uuid::new_v4(),
            model_id: "gpt-4o-mini".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: Uuid::new_v4(),
            role: ChatRole::User,
            content: "hello".to_string(),
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: now,
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }];

        let off_request = build_chat_request(
            &agent,
            &messages,
            false,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
            Some(crate::domain::ThinkingLevel::Off),
        );
        assert!(off_request.reasoning_effort.is_none());
        let serialized = serde_json::to_string(&off_request).unwrap();
        assert!(!serialized.contains("reasoning_effort"));

        let none_request = build_chat_request(
            &agent,
            &messages,
            false,
            "You write Rust.",
            &serde_json::Value::Null,
            None,
            None,
        );
        assert!(none_request.reasoning_effort.is_none());
        let serialized = serde_json::to_string(&none_request).unwrap();
        assert!(!serialized.contains("reasoning_effort"));
    }

    #[test]
    fn parses_nullable_content_for_tool_calls() {
        let parsed: ChatCompletionResponse = serde_json::from_str(
            r#"{
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": "run", "arguments": "{}"}
                        }]
                    }
                }]
            }"#,
        )
        .unwrap();

        let choice = parsed.choices.into_iter().next().unwrap();
        assert_eq!(choice.message.content.unwrap_or_default(), "");
        assert_eq!(choice.message.tool_calls.unwrap()[0].id, "call-1");
    }

    #[test]
    fn adapter_request_matches_hand_built_request() {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes code".to_string(),
            model_provider_id: Uuid::new_v4(),
            model_id: "gpt-4o-mini".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let provider = ModelProvider {
            id: Uuid::new_v4(),
            provider_type: crate::domain::ProviderType::OpenAiCompatible,
            name: "test".into(),
            base_url: "http://localhost/v1".into(),
            api_key: "key".into(),
            models: vec!["gpt-4o-mini".into()],
            model_configs: vec![],
            extra_body: serde_json::json!({"temperature": 0.2}),
            prompt_cache: Default::default(),
            cache_capabilities: None,
            created_at: now,
            updated_at: now,
        };
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: Uuid::new_v4(),
            role: ChatRole::User,
            content: "hello".to_string(),
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: now,
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }];

        let request = build_openai_chat_request(
            &agent,
            &provider,
            &messages,
            true,
            "You write Rust.",
            &provider.extra_body,
            None,
            Some(crate::domain::ThinkingLevel::High),
        )
        .unwrap();
        assert_eq!(request["model"], "gpt-4o-mini");
        assert_eq!(request["stream"], true);
        assert_eq!(request["reasoning_effort"], "high");
        assert_eq!(request["temperature"], 0.2);
        assert_eq!(request["stream_options"]["include_usage"], true);
        assert_eq!(
            OpenAiChatAdapter.completion_url(&provider, &agent, true),
            "http://localhost/v1/chat/completions"
        );
    }

    #[test]
    fn parse_output_keeps_reasoning_and_rejects_abnormal_finish() {
        let output = parse_openai_chat_output(&serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "", "reasoning_content": "thinking"}
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }))
        .unwrap();
        assert_eq!(output.reasoning_content, "thinking");
        assert_eq!(output.usage.input_tokens, 10);

        let error = parse_openai_chat_output(&serde_json::json!({
            "choices": [{
                "finish_reason": "length",
                "message": {"role": "assistant", "content": "cut"}
            }]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("token limit"));

        let error = parse_openai_chat_output(&serde_json::json!({"choices": []})).unwrap_err();
        assert!(error.to_string().contains("empty model response"));
    }
}
