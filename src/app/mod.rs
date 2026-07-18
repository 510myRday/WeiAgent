mod agent_executor;
mod chat;
pub mod handlers;
mod router;
mod run_manager;
mod state;
mod static_assets;

pub use router::{build_router, build_router_dev};
pub use run_manager::RunManager;
pub use state::AppState;

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use async_trait::async_trait;
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use futures_util::stream;
    use serde_json::json;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::{
        domain::{
            Agent, ChatMessage, CreateAgentRequest, CreateConversationRequest,
            CreateModelProviderRequest, ModelProvider, ProviderType,
        },
        error::AppResult,
        llm::{
            client::{ChatClient, ChatStream, DynChatClient},
            registry::LlmRegistry,
            types::{ChatCompletionOutput, StreamEvent, ToolDefinition},
        },
        storage::{JsonStore, LlmLogger},
        tools::{
            approval::ApprovalManager, registry::ToolRegistry, runtime::CodexRuntimeManager,
            terminal::TerminalManager,
        },
    };

    fn test_codex_runtime(root: &std::path::Path) -> Arc<CodexRuntimeManager> {
        Arc::new(CodexRuntimeManager::new_available_for_tests(root).unwrap())
    }

    struct MockChatClient;

    #[async_trait]
    impl ChatClient for MockChatClient {
        async fn complete(
            &self,
            agent: &Agent,
            provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
            _options: crate::llm::client::RequestOptions,
            _tools: Option<Vec<ToolDefinition>>,
        ) -> AppResult<ChatCompletionOutput> {
            Ok(ChatCompletionOutput {
                content: format!("{} via {}", agent.model_id, provider.name),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            })
        }

        async fn stream(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
            _options: crate::llm::client::RequestOptions,
            _tools: Option<Vec<ToolDefinition>>,
        ) -> AppResult<ChatStream> {
            Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::TextDelta("mock\n\n".to_string())),
                Ok(StreamEvent::TextDelta("stream".to_string())),
                Ok(StreamEvent::Finished),
            ])))
        }

        async fn list_models(&self, _provider: &ModelProvider) -> AppResult<Vec<String>> {
            Ok(vec!["mock-model-a".to_string(), "mock-model-b".to_string()])
        }
    }

    struct MockLlmRegistry {
        client: DynChatClient,
    }

    impl LlmRegistry for MockLlmRegistry {
        fn client_for(&self, _provider: &ModelProvider) -> AppResult<DynChatClient> {
            Ok(self.client.clone())
        }
    }

    async fn test_app() -> (Router, Uuid, Uuid, Uuid) {
        let temp_dir = tempfile::tempdir().unwrap().keep();
        let store = Arc::new(JsonStore::new(&temp_dir).await.unwrap());
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                provider_type: ProviderType::OpenAiCompatible,
                name: "Test Provider".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "test-key".to_string(),
                models: vec!["test-model".to_string(), "other-model".to_string()],
                model_configs: Vec::new(),
                extra_body: serde_json::Value::Null,
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Test Agent".to_string(),
                description: "Handles test conversations.".to_string(),
                model_provider_id: provider.id,
                model_id: "test-model".to_string(),
                avatar_icon: Some("bot".to_string()),
                avatar_color: Some("#22c55e".to_string()),
            })
            .await
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let app = build_router(AppState {
            store,
            llm_registry: Arc::new(MockLlmRegistry {
                client: Arc::new(MockChatClient),
            }),
            llm_logger: Arc::new(LlmLogger::new(tempfile::tempdir().unwrap().path(), false).unwrap()),
            terminal_manager: TerminalManager::new_for_tests(),
            approval_manager: Arc::new(ApprovalManager::new()),
            codex_runtime: test_codex_runtime(&temp_dir),
            tool_registry: Arc::new(ToolRegistry::with_builtin_tools()),
            run_manager: Arc::new(crate::app::run_manager::RunManager::new()),
        });
        (app, agent.id, provider.id, conversation.id)
    }

    #[tokio::test]
    async fn sends_non_streaming_message_and_persists_chat() {
        let (app, _, _, conversation_id) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": "hello" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["assistant"]["content"],
            "test-model via Test Provider"
        );
        assert_eq!(
            value["assistant"]["conversation_id"],
            conversation_id.to_string()
        );
    }

    #[tokio::test]
    async fn restarting_from_user_message_replaces_it_and_following_history() {
        let (app, _, _, conversation_id) = test_app().await;
        let kept_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": "keep this" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(kept_response.status(), StatusCode::OK);
        let kept_body = to_bytes(kept_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let kept: serde_json::Value = serde_json::from_slice(&kept_body).unwrap();
        let kept_user_id = kept["user"]["id"].as_str().unwrap();
        let kept_assistant_id = kept["assistant"]["id"].as_str().unwrap();

        let first_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": "original" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first_response.status(), StatusCode::OK);
        let first_body = to_bytes(first_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let first: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let original_user_id = first["user"]["id"].as_str().unwrap();
        let original_assistant_id = first["assistant"]["id"].as_str().unwrap();

        let restart_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "content": "edited",
                            "restart_from_message_id": original_user_id,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restart_response.status(), StatusCode::OK);

        let history_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(history_response.status(), StatusCode::OK);
        let history_body = to_bytes(history_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let history: Vec<serde_json::Value> = serde_json::from_slice(&history_body).unwrap();

        assert_eq!(history.len(), 4);
        assert_eq!(history[0]["role"], "user");
        assert_eq!(history[0]["content"], "keep this");
        assert_eq!(history[1]["role"], "assistant");
        assert_eq!(history[2]["role"], "user");
        assert_eq!(history[2]["content"], "edited");
        assert_eq!(history[3]["role"], "assistant");
        assert_eq!(history[0]["id"], kept_user_id);
        assert_eq!(history[1]["id"], kept_assistant_id);
        assert!(
            history
                .iter()
                .all(|message| message["id"] != original_user_id)
        );
        assert!(
            history
                .iter()
                .all(|message| message["id"] != original_assistant_id)
        );
    }

    #[tokio::test]
    async fn lists_model_providers_without_secret() {
        let (app, _, provider_id, _) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/model-providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let provider = value
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["id"] == provider_id.to_string())
            .unwrap();
        assert_eq!(provider["provider_type"], "openai_compatible");
        assert_eq!(provider["has_api_key"], true);
        assert!(provider["api_key"].is_null());
    }

    #[tokio::test]
    async fn lists_provider_models_through_registry() {
        let (app, _, provider_id, _) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/model-providers/{provider_id}/models"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value, json!(["mock-model-a", "mock-model-b"]));
    }

    #[tokio::test]
    async fn fetches_models_from_provider_draft() {
        let (app, _, _, _) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/model-providers/models")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "provider_type": "openai_compatible",
                            "base_url": "https://example.test/v1",
                            "api_key": "draft-key"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value, json!(["mock-model-a", "mock-model-b"]));
    }

    #[tokio::test]
    async fn streams_message_chunks_as_sse() {
        let (app, _, _, conversation_id) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{conversation_id}/messages/stream"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": "hello" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: assistant"));
        assert!(text.contains(r#"data: {"text":"mock\n\n"}"#));
        assert!(text.contains(r#""content":"mock\n\nstream""#));
        assert!(text.contains("event: done"));
    }

    #[tokio::test]
    async fn message_run_completes_and_replays_events_without_an_initial_subscriber() {
        let (app, _, _, conversation_id) = test_app().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/runs"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "content": "keep running", "stream": true }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let run: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let run_id = run["id"].as_str().unwrap();

        let mut status = String::new();
        for _ in 0..50 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/runs/{run_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let run: serde_json::Value = serde_json::from_slice(&body).unwrap();
            status = run["status"].as_str().unwrap_or_default().to_string();
            if status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(status, "completed");

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/runs/{run_id}/events?after=0"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: delta"));
        assert!(text.contains(r#"data: {"text":"mock\n\n"}"#));
        assert!(text.contains(r#""content":"mock\n\nstream""#));
        assert!(text.contains("event: done"));
        assert!(text.contains("id: "));
    }

    struct ScriptedChatClient {
        outputs: std::sync::Mutex<std::collections::VecDeque<ChatCompletionOutput>>,
        complete_calls: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedChatClient {
        fn new(outputs: Vec<ChatCompletionOutput>) -> Self {
            Self {
                outputs: std::sync::Mutex::new(outputs.into()),
                complete_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ChatClient for ScriptedChatClient {
        async fn complete(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
            _options: crate::llm::client::RequestOptions,
            tools: Option<Vec<ToolDefinition>>,
        ) -> AppResult<ChatCompletionOutput> {
            if let Some(list) = &tools {
                assert_eq!(list.len(), 2);
            }
            self.complete_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| crate::error::AppError::Llm("script exhausted".to_string()))
        }

        async fn stream(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
            _options: crate::llm::client::RequestOptions,
            _tools: Option<Vec<ToolDefinition>>,
        ) -> AppResult<ChatStream> {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Finished)])))
        }

        async fn list_models(&self, _provider: &ModelProvider) -> AppResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    fn scripted_tool_call(
        id: &str,
        name: &str,
        arguments: serde_json::Value,
    ) -> crate::llm::types::ToolCall {
        crate::llm::types::ToolCall {
            id: id.to_string(),
            kind: "function".to_string(),
            function: crate::llm::types::ToolCallFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    async fn scripted_test_app(
        outputs: Vec<ChatCompletionOutput>,
    ) -> (
        Router,
        AppState,
        Uuid,
        std::path::PathBuf,
        Arc<ScriptedChatClient>,
    ) {
        let root = tempfile::tempdir().unwrap().keep();
        let store = Arc::new(JsonStore::new(&root).await.unwrap());
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                provider_type: ProviderType::OpenAiCompatible,
                name: "Scripted Provider".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "test-key".to_string(),
                models: vec!["test-model".to_string()],
                model_configs: Vec::new(),
                extra_body: serde_json::Value::Null,
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Tool Agent".to_string(),
                description: "Runs tools in tests.".to_string(),
                model_provider_id: provider.id,
                model_id: "test-model".to_string(),
                avatar_icon: Some("bot".to_string()),
                avatar_color: Some("#22c55e".to_string()),
            })
            .await
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let mut tools_config = crate::domain::AgentToolsConfig::default();
        tools_config.terminal.enabled = true;
        tools_config.terminal.permission_mode = crate::domain::TerminalPermissionMode::FullAccess;
        tools_config.terminal.allowed_cwd_roots = vec![root.to_string_lossy().to_string()];
        store
            .update_agent_tools_config(agent.id, tools_config)
            .await
            .unwrap();

        let client = Arc::new(ScriptedChatClient::new(outputs));
        let state = AppState {
            store,
            llm_registry: Arc::new(MockLlmRegistry {
                client: client.clone(),
            }),
            llm_logger: Arc::new(LlmLogger::new(&root, false).unwrap()),
            terminal_manager: TerminalManager::new_for_tests(),
            approval_manager: Arc::new(ApprovalManager::new()),
            codex_runtime: test_codex_runtime(&root),
            tool_registry: Arc::new(ToolRegistry::with_builtin_tools()),
            run_manager: Arc::new(crate::app::run_manager::RunManager::new()),
        };
        (
            build_router(state.clone()),
            state,
            conversation.id,
            root,
            client,
        )
    }

    fn delayed_echo_command() -> String {
        if cfg!(windows) {
            "ping -n 2 127.0.0.1 >NUL && echo background-finished".to_string()
        } else {
            "sleep 0.2; echo background-finished".to_string()
        }
    }

    #[tokio::test]
    async fn complete_endpoint_runs_multiple_tool_rounds() {
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-1",
                    "execute_terminal",
                    json!({"command": "echo first"}),
                )],
            },
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-2",
                    "execute_terminal",
                    json!({"command": "echo second"}),
                )],
            },
            ChatCompletionOutput {
                content: "all tool rounds completed".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, state, conversation_id, _, client) = scripted_test_app(outputs).await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"content": "run tools"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["assistant"]["content"], "all tool rounds completed");
        assert_eq!(
            client
                .complete_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        let messages = state
            .store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        assert_eq!(messages.len(), 6);
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == crate::domain::ChatRole::Tool)
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn llm_error_after_tool_result_is_persisted_as_failed_assistant_message() {
        let outputs = vec![ChatCompletionOutput {
            content: String::new(),
            reasoning_content: String::new(),
            usage: Default::default(),
            tool_calls: vec![scripted_tool_call(
                "call-before-error",
                "execute_terminal",
                json!({"command": "echo before-error"}),
            )],
        }];
        let (app, state, conversation_id, _, _) = scripted_test_app(outputs).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{conversation_id}/messages/stream"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "run then fail", "stream": false}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: assistant"));
        assert!(text.contains("\"status\":\"failed\""));
        assert!(text.contains("Agent run stopped"));
        assert!(text.contains("event: error"));

        let messages = state
            .store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        let failure = messages.last().unwrap();
        assert_eq!(failure.role, crate::domain::ChatRole::Assistant);
        assert_eq!(failure.status, crate::domain::ChatMessageStatus::Failed);
        assert!(failure.content.contains("script exhausted"));
    }

    #[tokio::test]
    async fn invalid_tool_arguments_still_emit_a_tool_card_and_result() {
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![crate::llm::types::ToolCall {
                    id: "invalid-args-call".to_string(),
                    kind: "function".to_string(),
                    function: crate::llm::types::ToolCallFunction {
                        name: "execute_terminal".to_string(),
                        arguments: "{\"command\":".to_string(),
                    },
                }],
            },
            ChatCompletionOutput {
                content: "handled invalid arguments".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, _, conversation_id, _, _) = scripted_test_app(outputs).await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{conversation_id}/messages/stream"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "run invalid tool", "stream": false}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: tool_call"));
        assert!(text.contains("invalid-args-call"));
        assert!(text.contains("event: tool_result"));
        assert!(text.contains("Invalid tool arguments"));
        assert!(text.contains("event: done"));
    }

    #[tokio::test]
    async fn timeout_moves_process_to_background_and_captures_output() {
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-background",
                    "execute_terminal",
                    json!({"command": delayed_echo_command(), "timeout_ms": 10}),
                )],
            },
            ChatCompletionOutput {
                content: "background process started".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, state, conversation_id, _, _) = scripted_test_app(outputs).await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "start background"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let messages = state
            .store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        let tool_message = messages
            .iter()
            .find(|message| message.role == crate::domain::ChatRole::Tool)
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&tool_message.content).unwrap();
        assert_eq!(result["status"], "running");
        let process_id = result["process_id"].as_str().unwrap();
        let agent_id = state
            .store
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .agent_id;

        let mut process = None;
        for _ in 0..40 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/api/tools/processes/{process_id}?agent_id={agent_id}"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if value["status"] != "running" {
                process = Some(value);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let process = process.expect("background process did not finish");
        assert_eq!(process["status"], "completed");
        assert!(
            process["stdout"]
                .as_str()
                .unwrap()
                .contains("background-finished")
        );
    }

    #[tokio::test]
    async fn always_approve_persists_and_skips_future_approval() {
        let command = "echo approve-me";
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-approval-1",
                    "execute_terminal",
                    json!({
                        "command": command,
                        "sandbox_permissions": "require_escalated",
                        "justification": "test full host access"
                    }),
                )],
            },
            ChatCompletionOutput {
                content: "approved once".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-approval-2",
                    "execute_terminal",
                    json!({
                        "command": command,
                        "sandbox_permissions": "require_escalated",
                        "justification": "test remembered full host access"
                    }),
                )],
            },
            ChatCompletionOutput {
                content: "approved permanently".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, state, conversation_id, _, _) = scripted_test_app(outputs).await;
        let agent_id = state
            .store
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .agent_id;
        let mut config = state.store.read_agent_tools_config(agent_id).await.unwrap();
        config.terminal.enabled = true;
        config
            .terminal
            .approval_grants
            .retain(|grant| grant.command != command);
        config.terminal.permission_mode = crate::domain::TerminalPermissionMode::RequestApproval;
        state
            .store
            .update_agent_tools_config(agent_id, config)
            .await
            .unwrap();

        let stream_app = app.clone();
        let first_request = tokio::spawn(async move {
            stream_app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/conversations/{conversation_id}/messages/stream"
                        ))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"content": "approve", "stream": false}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        for _ in 0..100 {
            if state.approval_manager.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(state.approval_manager.pending_count(), 1);
        let approval_id = state
            .approval_manager
            .resolve_for_test_id()
            .expect("missing approval id");
        let duplicate_approval_id = approval_id.clone();
        let approval_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tools/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "approval_id": approval_id,
                            "agent_id": agent_id,
                            "decision": "always_approve"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approval_response.status(), StatusCode::OK);
        let duplicate_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tools/approve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "approval_id": duplicate_approval_id,
                            "agent_id": agent_id,
                            "decision": "always_approve"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(duplicate_response.status(), StatusCode::OK);
        let duplicate_body = to_bytes(duplicate_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let duplicate_value: serde_json::Value = serde_json::from_slice(&duplicate_body).unwrap();
        assert_eq!(duplicate_value["status"], "always_approved");
        let first_response = first_request.await.unwrap();
        let first_body = to_bytes(first_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_text = String::from_utf8(first_body.to_vec()).unwrap();
        assert!(first_text.contains("event: approval_required"));
        assert!(first_text.contains("event: done"));

        let stored = state.store.read_agent_tools_config(agent_id).await.unwrap();
        assert!(stored.terminal.approval_grants.iter().any(|grant| {
            grant.command == command
                && grant.permission_mode == crate::domain::TerminalPermissionMode::RequestApproval
        }));

        let second_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{conversation_id}/messages/stream"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "again", "stream": false}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let second_body = to_bytes(second_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let second_text = String::from_utf8(second_body.to_vec()).unwrap();
        assert!(!second_text.contains("event: approval_required"));
        assert!(second_text.contains("event: done"));
    }
    #[tokio::test]
    async fn invalid_escalation_is_reported_without_starting_a_process() {
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-blocked",
                    "execute_terminal",
                    json!({
                        "command": "echo invalid",
                        "sandbox_permissions": "require_escalated"
                    }),
                )],
            },
            ChatCompletionOutput {
                content: "invalid escalation rejected".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, state, conversation_id, _, _) = scripted_test_app(outputs).await;
        let agent_id = state
            .store
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .agent_id;
        let mut config = state.store.read_agent_tools_config(agent_id).await.unwrap();
        config.terminal.enabled = true;
        config.terminal.permission_mode = crate::domain::TerminalPermissionMode::RequestApproval;
        state
            .store
            .update_agent_tools_config(agent_id, config)
            .await
            .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{conversation_id}/messages/stream"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "unsafe", "stream": false}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: tool_result"));
        assert!(text.contains("justification is required"));
        assert!(state.terminal_manager.list(None).await.is_empty());
    }

    #[tokio::test]
    async fn model_can_start_and_stop_an_explicit_background_process() {
        let command = if cfg!(windows) {
            "ping -n 10 127.0.0.1 >NUL".to_string()
        } else {
            "sleep 5".to_string()
        };
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-explicit-background",
                    "execute_terminal",
                    json!({"command": command, "background": true, "timeout_ms": 5000}),
                )],
            },
            ChatCompletionOutput {
                content: "service started".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, state, conversation_id, _, _) = scripted_test_app(outputs).await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"content": "start service"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let messages = state
            .store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(
            &messages
                .iter()
                .find(|message| message.role == crate::domain::ChatRole::Tool)
                .unwrap()
                .content,
        )
        .unwrap();
        let process_id = result["process_id"].as_str().unwrap();
        let agent_id = state
            .store
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .agent_id;
        let stop_response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/api/tools/processes/{process_id}?agent_id={agent_id}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stop_response.status(), StatusCode::OK);
        let body = to_bytes(stop_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(value["status"], "running");
    }
    struct StreamingToolClient {
        streams: std::sync::Mutex<std::collections::VecDeque<Vec<StreamEvent>>>,
    }

    #[async_trait]
    impl ChatClient for StreamingToolClient {
        async fn complete(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
            _options: crate::llm::client::RequestOptions,
            _tools: Option<Vec<ToolDefinition>>,
        ) -> AppResult<ChatCompletionOutput> {
            Err(crate::error::AppError::Llm(
                "complete was not expected".to_string(),
            ))
        }

        async fn stream(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
            _options: crate::llm::client::RequestOptions,
            tools: Option<Vec<ToolDefinition>>,
        ) -> AppResult<ChatStream> {
            assert_eq!(tools.as_ref().map(Vec::len), Some(2));
            let events = self.streams.lock().unwrap().pop_front().ok_or_else(|| {
                crate::error::AppError::Llm("stream script exhausted".to_string())
            })?;
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
        }

        async fn list_models(&self, _provider: &ModelProvider) -> AppResult<Vec<String>> {
            Ok(Vec::new())
        }
    }

    async fn streaming_test_app(streams: Vec<Vec<StreamEvent>>) -> (Router, Arc<JsonStore>, Uuid) {
        let root = tempfile::tempdir().unwrap().keep();
        let store = Arc::new(JsonStore::new(&root).await.unwrap());
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                provider_type: ProviderType::OpenAiCompatible,
                name: "Streaming Provider".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "test-key".to_string(),
                models: vec!["test-model".to_string()],
                model_configs: Vec::new(),
                extra_body: serde_json::Value::Null,
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Streaming Tool Agent".to_string(),
                description: "Runs streaming tool tests.".to_string(),
                model_provider_id: provider.id,
                model_id: "test-model".to_string(),
                avatar_icon: Some("bot".to_string()),
                avatar_color: Some("#22c55e".to_string()),
            })
            .await
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let mut tools_config = crate::domain::AgentToolsConfig::default();
        tools_config.terminal.enabled = true;
        tools_config.terminal.permission_mode = crate::domain::TerminalPermissionMode::FullAccess;
        tools_config.terminal.allowed_cwd_roots = vec![root.to_string_lossy().to_string()];
        store
            .update_agent_tools_config(agent.id, tools_config)
            .await
            .unwrap();
        let client = Arc::new(StreamingToolClient {
            streams: std::sync::Mutex::new(streams.into()),
        });
        let state = AppState {
            store: store.clone(),
            llm_registry: Arc::new(MockLlmRegistry { client }),
            llm_logger: Arc::new(LlmLogger::new(&root, false).unwrap()),
            terminal_manager: TerminalManager::new_for_tests(),
            approval_manager: Arc::new(ApprovalManager::new()),
            codex_runtime: test_codex_runtime(&root),
            tool_registry: Arc::new(ToolRegistry::with_builtin_tools()),
            run_manager: Arc::new(crate::app::run_manager::RunManager::new()),
        };
        (build_router(state), store, conversation.id)
    }

    #[tokio::test]
    async fn tool_budget_triggers_final_summary_without_tools() {
        let outputs = vec![
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-budget-1",
                    "execute_terminal",
                    json!({"command": "echo one"}),
                )],
            },
            ChatCompletionOutput {
                content: String::new(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: vec![scripted_tool_call(
                    "call-budget-2",
                    "execute_terminal",
                    json!({"command": "echo two"}),
                )],
            },
            ChatCompletionOutput {
                content: "budget summary ready".to_string(),
                reasoning_content: String::new(),
                usage: Default::default(),
                tool_calls: Vec::new(),
            },
        ];
        let (app, state, conversation_id, _, client) = scripted_test_app(outputs).await;
        let agent_id = state
            .store
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .agent_id;
        let mut config = state.store.read_agent_tools_config(agent_id).await.unwrap();
        config.max_llm_iterations = 2;
        state
            .store
            .update_agent_tools_config(agent_id, config)
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/conversations/{conversation_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"content": "hit budget"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["assistant"]["content"], "budget summary ready");
        assert_eq!(
            client
                .complete_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        let messages = state
            .store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == crate::domain::ChatRole::Tool)
                .count(),
            2
        );
        assert_eq!(messages.last().unwrap().content, "budget summary ready");
    }

    #[tokio::test]
    async fn streaming_function_call_runs_tool_and_streams_final_answer() {
        let arguments = json!({"command": "echo streamed-tool"}).to_string();
        let (app, store, conversation_id) = streaming_test_app(vec![
            vec![
                StreamEvent::ToolCallStart {
                    index: 0,
                    id: "stream-call".to_string(),
                    name: "execute_terminal".to_string(),
                },
                StreamEvent::ToolCallArgsDelta {
                    index: 0,
                    arguments,
                },
                StreamEvent::FinishedWithToolCalls,
            ],
            vec![
                StreamEvent::TextDelta("streamed ".to_string()),
                StreamEvent::TextDelta("final".to_string()),
                StreamEvent::Finished,
            ],
        ])
        .await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{}/messages/stream",
                        conversation_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "stream a tool", "stream": true}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: phase"));
        assert!(text.contains("\"phase\":\"waiting_for_model\""));
        assert!(text.contains("\"phase\":\"executing_tool\""));
        assert!(text.contains("event: tool_call"));
        assert!(text.contains("event: tool_result"));
        assert!(text.contains("streamed final"));
        assert!(text.contains("event: done"));
        let messages = store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == crate::domain::ChatRole::Tool)
                .count(),
            1
        );
        assert_eq!(messages.last().unwrap().content, "streamed final");
    }

    #[tokio::test]
    async fn incomplete_llm_stream_is_reported_as_an_error_not_normal_completion() {
        let (app, store, conversation_id) = streaming_test_app(vec![vec![StreamEvent::TextDelta(
            "partial response".to_string(),
        )]])
        .await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/conversations/{conversation_id}/messages/stream"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"content": "drop the stream", "stream": true}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("partial response"));
        assert!(text.contains("event: error"));
        assert!(text.contains("ended before a completion event"));
        assert!(!text.contains("event: done"));

        let messages = store
            .read_conversation_messages(conversation_id)
            .await
            .unwrap();
        assert!(messages.iter().any(|message| {
            message.status == crate::domain::ChatMessageStatus::Failed
                && message.content == "partial response"
        }));
    }
}
