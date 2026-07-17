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
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(64))
        .with_state(state)
}

pub fn build_router_dev(state: AppState) -> Router {
    api_router()
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(64))
        .with_state(state)
}

fn no_store() -> SetResponseHeaderLayer<HeaderValue> {
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
            "/api/agents/{agent_id}/runs",
            get(conversations::list_active_agent_runs),
        )
        .route(
            "/api/conversations/{conversation_id}",
            put(conversations::update_conversation).delete(conversations::delete_conversation),
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
            "/api/conversations/{conversation_id}/runs",
            post(conversations::create_message_run),
        )
        .route(
            "/api/conversations/{conversation_id}/run",
            get(conversations::active_conversation_run),
        )
        .route(
            "/api/runs/{run_id}/events",
            get(conversations::stream_run_events),
        )
        .route(
            "/api/runs/{run_id}",
            get(conversations::read_run).delete(conversations::cancel_run),
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
            "/api/tools/codex-runtime",
            get(tools::read_codex_runtime).post(tools::install_codex_runtime),
        )
        .route(
            "/api/tools/codex-runtime/refresh",
            post(tools::refresh_codex_runtime),
        )
        .route(
            "/api/tools/runtime/{component}",
            post(tools::install_runtime_component),
        )
        .route(
            "/api/tools/runtime/{component}/refresh",
            post(tools::refresh_runtime_component),
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
        .layer(no_store())
}
