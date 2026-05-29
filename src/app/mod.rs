mod chat;
pub mod handlers;
mod router;
mod state;
mod static_assets;

pub use router::{build_router, build_router_dev};
pub use state::AppState;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
        },
        storage::JsonStore,
    };

    struct MockChatClient;

    #[async_trait]
    impl ChatClient for MockChatClient {
        async fn complete(
            &self,
            agent: &Agent,
            provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
        ) -> AppResult<String> {
            Ok(format!("{} via {}", agent.model_id, provider.name))
        }

        async fn stream(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
            _system_prompt: &str,
        ) -> AppResult<ChatStream> {
            Ok(Box::pin(stream::iter(vec![
                Ok("mock ".to_string()),
                Ok("stream".to_string()),
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
        let store = Arc::new(JsonStore::new(temp_dir).await.unwrap());
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                provider_type: ProviderType::OpenAiCompatible,
                name: "Test Provider".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "test-key".to_string(),
                models: vec!["test-model".to_string(), "other-model".to_string()],
                extra_body: serde_json::Value::Null,
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Test Agent".to_string(),
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
        assert!(text.contains("event: delta"));
        assert!(text.contains("data: mock "));
        assert!(text.contains("data: stream"));
        assert!(text.contains("event: done"));
    }
}
