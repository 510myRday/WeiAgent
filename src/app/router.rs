use axum::{
    Router,
    http::Method,
    routing::{get, post, put},
};
use tower_http::cors::{Any, CorsLayer};

use crate::app::{
    AppState,
    handlers::{agents, conversations, providers, settings},
    static_assets::serve_frontend,
};

pub fn build_router(state: AppState) -> Router {
    api_router()
        .fallback(|uri: axum::http::Uri| async move { serve_frontend(uri).await })
        .layer(cors_layer())
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
        .route(
            "/api/settings",
            get(settings::read_settings).put(settings::update_settings),
        )
        .route(
            "/api/model-providers",
            get(providers::list_model_providers).post(providers::create_model_provider),
        )
        .route(
            "/api/model-providers/{provider_id}",
            put(providers::update_model_provider).delete(providers::delete_model_provider),
        )
        .route(
            "/api/model-providers/{provider_id}/models",
            get(providers::list_provider_models),
        )
        .route(
            "/api/agents",
            get(agents::list_agents).post(agents::create_agent),
        )
        .route(
            "/api/agents/{agent_id}",
            put(agents::update_agent).delete(agents::delete_agent),
        )
        .route(
            "/api/agents/{agent_id}/workspace",
            get(agents::read_agent_workspace).put(agents::update_agent_workspace),
        )
        .route(
            "/api/agents/{agent_id}/conversations",
            get(conversations::list_conversations).post(conversations::create_conversation),
        )
        .route(
            "/api/conversations/{conversation_id}",
            axum::routing::delete(conversations::delete_conversation),
        )
        .route(
            "/api/conversations/{conversation_id}/messages",
            get(conversations::read_conversation_messages)
                .post(conversations::send_message_complete),
        )
        .route(
            "/api/conversations/{conversation_id}/messages/stream",
            post(conversations::send_message_stream),
        )
}
