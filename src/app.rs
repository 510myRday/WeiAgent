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
        ChatMessage, ChatRole, CreateAgentRequest, ModelListResponse, SendMessageRequest,
        SendMessageResponse, UpdateAgentRequest,
    },
    openai::DynChatClient,
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
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

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
            None => {
                // SPA fallback: serve index.html for unknown paths
                match FrontendAssets::get("index.html") {
                    Some(file) => axum::response::Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/html")
                        .body(axum::body::Body::from(file.data))
                        .unwrap(),
                    None => axum::response::Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(axum::body::Body::from("Not Found"))
                        .unwrap(),
                }
            }
        }
    }

    Router::new()
        .route("/api/models", get(list_models))
        .route("/api/agents", get(list_agents).post(create_agent))
        .route(
            "/api/agents/{agent_id}",
            put(update_agent).delete(delete_agent),
        )
        .route("/api/chats/{agent_id}", get(read_chat))
        .route(
            "/api/chats/{agent_id}/messages",
            post(send_message_complete),
        )
        .route(
            "/api/chats/{agent_id}/messages/stream",
            post(send_message_stream),
        )
        .fallback(
            |uri: axum::http::Uri| async move { serve_frontend(uri).await },
        )
        .layer(cors)
        .with_state(state)
}

pub fn build_router_dev(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

    Router::new()
        .route("/api/models", get(list_models))
        .route("/api/agents", get(list_agents).post(create_agent))
        .route(
            "/api/agents/{agent_id}",
            put(update_agent).delete(delete_agent),
        )
        .route("/api/chats/{agent_id}", get(read_chat))
        .route(
            "/api/chats/{agent_id}/messages",
            post(send_message_complete),
        )
        .route(
            "/api/chats/{agent_id}/messages/stream",
            post(send_message_stream),
        )
        .layer(cors)
        .with_state(state)
}

async fn list_models(State(state): State<AppState>) -> AppResult<Json<ModelListResponse>> {
    Ok(Json(ModelListResponse {
        models: state.llm.list_models().await?,
    }))
}

async fn list_agents(State(state): State<AppState>) -> AppResult<Json<Vec<crate::models::Agent>>> {
    Ok(Json(state.store.list_agents().await?))
}

async fn create_agent(
    State(state): State<AppState>,
    Json(request): Json<CreateAgentRequest>,
) -> AppResult<Json<crate::models::Agent>> {
    Ok(Json(state.store.create_agent(request).await?))
}

async fn update_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<UpdateAgentRequest>,
) -> AppResult<Json<crate::models::Agent>> {
    Ok(Json(state.store.update_agent(agent_id, request).await?))
}

async fn delete_agent(State(state): State<AppState>, Path(agent_id): Path<Uuid>) -> AppResult<()> {
    state.store.delete_agent(agent_id).await
}

async fn read_chat(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<Vec<ChatMessage>>> {
    state.store.get_agent(agent_id).await?;
    Ok(Json(state.store.read_chat(agent_id).await?))
}

async fn send_message_complete(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Json<SendMessageResponse>> {
    if request.content.trim().is_empty() {
        return Err(AppError::BadRequest(
            "message content cannot be empty".to_string(),
        ));
    }

    let agent = state.store.get_agent(agent_id).await?;
    let history = state.store.read_chat(agent_id).await?;
    let user_message = ChatMessage {
        role: ChatRole::User,
        content: request.content.trim().to_string(),
        created_at: Utc::now(),
    };
    let mut messages_for_llm = history;
    messages_for_llm.push(user_message.clone());

    let assistant_content = state.llm.complete(&agent, &messages_for_llm).await?;
    let assistant_message = ChatMessage {
        role: ChatRole::Assistant,
        content: assistant_content,
        created_at: Utc::now(),
    };
    state
        .store
        .append_chat_messages(agent_id, &[user_message.clone(), assistant_message.clone()])
        .await?;

    Ok(Json(SendMessageResponse {
        user: user_message,
        assistant: assistant_message,
    }))
}

async fn send_message_stream(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    if request.content.trim().is_empty() {
        return Err(AppError::BadRequest(
            "message content cannot be empty".to_string(),
        ));
    }

    let agent = state.store.get_agent(agent_id).await?;
    let history = state.store.read_chat(agent_id).await?;
    let user_message = ChatMessage {
        role: ChatRole::User,
        content: request.content.trim().to_string(),
        created_at: Utc::now(),
    };
    let mut messages_for_llm = history;
    messages_for_llm.push(user_message.clone());
    let llm_stream = state.llm.stream(&agent, &messages_for_llm).await?;

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
            role: ChatRole::Assistant,
            content: assistant_content,
            created_at: Utc::now(),
        };

        match store
            .append_chat_messages(agent_id, &[user_message, assistant_message.clone()])
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
        models::Agent,
        openai::{ChatClient, ChatStream},
    };

    struct MockChatClient;

    #[async_trait]
    impl ChatClient for MockChatClient {
        async fn list_models(&self) -> AppResult<Vec<String>> {
            Ok(vec!["test-model".to_string(), "other-model".to_string()])
        }

        async fn complete(&self, _agent: &Agent, _messages: &[ChatMessage]) -> AppResult<String> {
            Ok("mock complete".to_string())
        }

        async fn stream(&self, _agent: &Agent, _messages: &[ChatMessage]) -> AppResult<ChatStream> {
            Ok(Box::pin(stream::iter(vec![
                Ok("mock ".to_string()),
                Ok("stream".to_string()),
            ])))
        }
    }

    async fn test_app() -> (Router, Uuid) {
        let temp_dir = tempfile::tempdir().unwrap().keep();
        let store = Arc::new(JsonStore::new(temp_dir).await.unwrap());
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Test Agent".to_string(),
                system_prompt: "Reply briefly.".to_string(),
                model: "test-model".to_string(),
            })
            .await
            .unwrap();
        let app = build_router(AppState {
            store,
            llm: Arc::new(MockChatClient),
        });
        (app, agent.id)
    }

    #[tokio::test]
    async fn sends_non_streaming_message_and_persists_chat() {
        let (app, agent_id) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/chats/{agent_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "content": "hello" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["assistant"]["content"], "mock complete");
    }

    #[tokio::test]
    async fn lists_available_models() {
        let (app, _) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["models"][0], "test-model");
    }

    #[tokio::test]
    async fn streams_message_chunks_as_sse() {
        let (app, agent_id) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/chats/{agent_id}/messages/stream"))
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
