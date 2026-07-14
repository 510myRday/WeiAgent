use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue},
    routing::{get, post, put},
};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::app::{
    AppState,
    handlers::{agents, conversations, llm_logs, providers, settings, tools},
    static_assets::serve_frontend,
};

pub fn build_router(state: AppState) -> Router {
    api_router()
        .fallback(|uri: axum::http::Uri| async move { serve_frontend(uri).await })
        .layer(security_headers())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(64))
        .with_state(state)
}

pub fn build_router_dev(state: AppState) -> Router {
    api_router()
        .layer(security_headers())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(64))
        .with_state(state)
}

fn security_headers() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-store"),
    )
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
            "/api/model-providers/models",
            post(providers::fetch_provider_models),
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
        .route(
            "/api/agents/{agent_id}/tools-config",
            get(tools::read_agent_tools_config).put(tools::update_agent_tools_config),
        )
        .route(
            "/api/tools/defaults",
            get(tools::read_tool_defaults).put(tools::update_tool_defaults),
        )
        .route(
            "/api/tools/sandbox-runtime",
            get(tools::read_sandbox_runtime).post(tools::install_sandbox_runtime),
        )
        .route(
            "/api/tools/sandbox-runtime/refresh",
            post(tools::refresh_sandbox_runtime),
        )
        .route("/api/tools/approve", post(tools::approve_tool_call))
        .route("/api/tools/processes", get(tools::list_tool_processes))
        .route(
            "/api/tools/processes/{process_id}",
            get(tools::get_tool_process).delete(tools::stop_tool_process),
        )
        .route("/api/llm-logs", get(llm_logs::list_llm_logs))
        .route(
            "/api/llm-logs/{log_id}",
            get(llm_logs::get_llm_log).delete(llm_logs::delete_llm_log),
        )
}
