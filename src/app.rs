use std::{convert::Infallible, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{Method, StatusCode},
    response::sse::{Event, Sse},
    routing::{get, post, put},
};
use chrono::Utc;
use futures_util::{Stream, StreamExt};
use rust_embed::Embed;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{
        AgentResponse, AgentWorkspace, AppSettings, ChatMessage, ChatRole, CreateAgentRequest,
        CreateConversationRequest, CreateModelProviderRequest, ModelProviderResponse,
        SendMessageRequest, SendMessageResponse, UpdateAgentRequest, UpdateAgentWorkspaceRequest,
        UpdateModelProviderRequest,
    },
    openai::{DynChatClient, OpenAiChatClient},
    storage::JsonStore,
};

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<JsonStore>,
    pub llm: DynChatClient,
}

pub fn build_router(state: AppState) -> Router {
    let cors = cors_layer();

    async fn serve_frontend(uri: axum::http::Uri) -> axum::response::Response {
        let path = uri.path().trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };

        match FrontendAssets::get(path) {
            Some(file) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", mime.as_ref())
                    .body(axum::body::Body::from(file.data))
                    .unwrap()
            }
            None => match FrontendAssets::get("index.html") {
                Some(file) => axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/html")
                    .body(axum::body::Body::from(file.data))
                    .unwrap(),
                None => axum::response::Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(axum::body::Body::from("Not Found"))
                    .unwrap(),
            },
        }
    }

    api_router()
        .fallback(|uri: axum::http::Uri| async move { serve_frontend(uri).await })
        .layer(cors)
        .with_state(state)
}

pub fn build_router_dev(state: AppState) -> Router {
    api_router().layer(cors_layer()).with_state(state)
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any)
}

fn api_router() -> Router<AppState> {
    Router::new()
        .route("/api/settings", get(read_settings).put(update_settings))
        .route(
            "/api/model-providers",
            get(list_model_providers).post(create_model_provider),
        )
        .route(
            "/api/model-providers/{provider_id}",
            put(update_model_provider).delete(delete_model_provider),
        )
        .route(
            "/api/model-providers/{provider_id}/models",
            get(list_provider_models),
        )
        .route("/api/agents", get(list_agents).post(create_agent))
        .route(
            "/api/agents/{agent_id}",
            put(update_agent).delete(delete_agent),
        )
        .route(
            "/api/agents/{agent_id}/workspace",
            get(read_agent_workspace).put(update_agent_workspace),
        )
        .route(
            "/api/agents/{agent_id}/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/api/conversations/{conversation_id}",
            axum::routing::delete(delete_conversation),
        )
        .route(
            "/api/conversations/{conversation_id}/messages",
            get(read_conversation_messages).post(send_message_complete),
        )
        .route(
            "/api/conversations/{conversation_id}/messages/stream",
            post(send_message_stream),
        )
}

async fn read_settings(State(state): State<AppState>) -> AppResult<Json<AppSettings>> {
    Ok(Json(state.store.read_settings().await?))
}

async fn update_settings(
    State(state): State<AppState>,
    Json(request): Json<AppSettings>,
) -> AppResult<Json<AppSettings>> {
    Ok(Json(state.store.write_settings(request).await?))
}

async fn list_model_providers(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ModelProviderResponse>>> {
    Ok(Json(state.store.list_model_providers().await?))
}

async fn create_model_provider(
    State(state): State<AppState>,
    Json(request): Json<CreateModelProviderRequest>,
) -> AppResult<Json<ModelProviderResponse>> {
    Ok(Json(state.store.create_model_provider(request).await?))
}

async fn update_model_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<UpdateModelProviderRequest>,
) -> AppResult<Json<ModelProviderResponse>> {
    Ok(Json(
        state
            .store
            .update_model_provider(provider_id, request)
            .await?,
    ))
}

async fn delete_model_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> AppResult<()> {
    state.store.delete_model_provider(provider_id).await
}

async fn list_provider_models(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> AppResult<Json<Vec<String>>> {
    let provider = state.store.get_model_provider(provider_id).await?;
    let client = OpenAiChatClient::new();
    let models = client.list_models(&provider).await?;
    Ok(Json(models))
}

async fn list_agents(State(state): State<AppState>) -> AppResult<Json<Vec<AgentResponse>>> {
    Ok(Json(state.store.list_agents().await?))
}

async fn create_agent(
    State(state): State<AppState>,
    Json(request): Json<CreateAgentRequest>,
) -> AppResult<Json<AgentResponse>> {
    Ok(Json(state.store.create_agent(request).await?))
}

async fn update_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<UpdateAgentRequest>,
) -> AppResult<Json<AgentResponse>> {
    Ok(Json(state.store.update_agent(agent_id, request).await?))
}

async fn delete_agent(State(state): State<AppState>, Path(agent_id): Path<Uuid>) -> AppResult<()> {
    state.store.delete_agent(agent_id).await
}

async fn read_agent_workspace(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentWorkspace>> {
    Ok(Json(state.store.read_agent_workspace(agent_id).await?))
}

async fn update_agent_workspace(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<UpdateAgentWorkspaceRequest>,
) -> AppResult<Json<AgentWorkspace>> {
    Ok(Json(
        state
            .store
            .update_agent_workspace(agent_id, request)
            .await?,
    ))
}

async fn list_conversations(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<Vec<crate::models::Conversation>>> {
    Ok(Json(state.store.list_conversations(agent_id).await?))
}

async fn create_conversation(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<CreateConversationRequest>,
) -> AppResult<Json<crate::models::Conversation>> {
    Ok(Json(
        state.store.create_conversation(agent_id, request).await?,
    ))
}

async fn delete_conversation(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<()> {
    state.store.delete_conversation(conversation_id).await
}

async fn read_conversation_messages(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<Vec<ChatMessage>>> {
    Ok(Json(
        state
            .store
            .read_conversation_messages(conversation_id)
            .await?,
    ))
}

async fn send_message_complete(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Json<SendMessageResponse>> {
    if request.content.trim().is_empty() {
        return Err(AppError::BadRequest(
            "message content cannot be empty".to_string(),
        ));
    }

    let conversation = state.store.get_conversation(conversation_id).await?;
    let mut agent = state.store.get_agent(conversation.agent_id).await?;
    agent.system_prompt = state.store.build_agent_system_prompt(&agent).await?;
    let provider = state
        .store
        .get_model_provider(agent.model_provider_id)
        .await?;
    let history = state
        .store
        .read_conversation_messages(conversation_id)
        .await?;
    let user_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::User,
        content: request.content.trim().to_string(),
        created_at: Utc::now(),
    };
    let mut messages_for_llm = history;
    messages_for_llm.push(user_message.clone());

    let assistant_content = state
        .llm
        .complete(&agent, &provider, &messages_for_llm)
        .await?;
    let assistant_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::Assistant,
        content: assistant_content,
        created_at: Utc::now(),
    };
    state
        .store
        .append_conversation_messages(
            conversation_id,
            &[user_message.clone(), assistant_message.clone()],
        )
        .await?;

    Ok(Json(SendMessageResponse {
        user: user_message,
        assistant: assistant_message,
    }))
}

async fn send_message_stream(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    if request.content.trim().is_empty() {
        return Err(AppError::BadRequest(
            "message content cannot be empty".to_string(),
        ));
    }

    let conversation = state.store.get_conversation(conversation_id).await?;
    let mut agent = state.store.get_agent(conversation.agent_id).await?;
    agent.system_prompt = state.store.build_agent_system_prompt(&agent).await?;
    let provider = state
        .store
        .get_model_provider(agent.model_provider_id)
        .await?;
    let history = state
        .store
        .read_conversation_messages(conversation_id)
        .await?;
    let user_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::User,
        content: request.content.trim().to_string(),
        created_at: Utc::now(),
    };
    let mut messages_for_llm = history;
    messages_for_llm.push(user_message.clone());
    let llm_stream = state
        .llm
        .stream(&agent, &provider, &messages_for_llm)
        .await?;

    let store = state.store.clone();
    let stream = async_stream::stream! {
        let mut assistant_content = String::new();
        let mut llm_stream = llm_stream;

        yield Ok(Event::default().event("user").json_data(&user_message).unwrap_or_else(|_| {
            Event::default().event("error").data("failed to serialize user message")
        }));

        while let Some(chunk) = llm_stream.next().await {
            match chunk {
                Ok(content) => {
                    assistant_content.push_str(&content);
                    yield Ok(Event::default().event("delta").data(content));
                }
                Err(error) => {
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            }
        }

        let assistant_message = ChatMessage {
            id: Uuid::new_v4(),
            agent_id: user_message.agent_id,
            conversation_id,
            role: ChatRole::Assistant,
            content: assistant_content,
            created_at: Utc::now(),
        };

        match store
            .append_conversation_messages(conversation_id, &[user_message, assistant_message.clone()])
            .await
        {
            Ok(()) => {
                yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                    Event::default().event("error").data("failed to serialize assistant message")
                }));
                yield Ok(Event::default().event("done").data("ok"));
            }
            Err(error) => {
                yield Ok(Event::default().event("error").data(error.to_string()));
            }
        }
    };

    Ok(Sse::new(stream))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use futures_util::stream;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        models::{Agent, ModelProvider},
        openai::{ChatClient, ChatStream},
    };

    struct MockChatClient;

    #[async_trait]
    impl ChatClient for MockChatClient {
        async fn complete(
            &self,
            agent: &Agent,
            provider: &ModelProvider,
            _messages: &[ChatMessage],
        ) -> AppResult<String> {
            Ok(format!("{} via {}", agent.model_id, provider.name))
        }

        async fn stream(
            &self,
            _agent: &Agent,
            _provider: &ModelProvider,
            _messages: &[ChatMessage],
        ) -> AppResult<ChatStream> {
            Ok(Box::pin(stream::iter(vec![
                Ok("mock ".to_string()),
                Ok("stream".to_string()),
            ])))
        }
    }

    async fn test_app() -> (Router, Uuid, Uuid, Uuid) {
        let temp_dir = tempfile::tempdir().unwrap().keep();
        let store = Arc::new(JsonStore::new(temp_dir).await.unwrap());
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                name: "Test Provider".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "test-key".to_string(),
                models: vec!["test-model".to_string(), "other-model".to_string()],
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Test Agent".to_string(),
                system_prompt: "Reply briefly.".to_string(),
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
            llm: Arc::new(MockChatClient),
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
        assert_eq!(provider["has_api_key"], true);
        assert!(provider["api_key"].is_null());
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
