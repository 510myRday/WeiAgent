use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue},
    middleware,
    routing::{get, patch, post, put},
};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::app::{
    AppState,
    handlers::{
        agents, assets, conversations, ip_proxies, llm_logs, providers, scheduling, settings,
        skills, tools,
    },
    security::enforce_api_security,
    static_assets::serve_frontend,
};

pub fn build_router(state: AppState) -> Router {
    let security = state.api_security.clone();
    let frontend_security = security.clone();
    api_router(security)
        .fallback(move |uri: axum::http::Uri| {
            let security = frontend_security.clone();
            async move { serve_frontend(uri, security.token()).await }
        })
        .layer(DefaultBodyLimit::max(48 * 1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(64))
        .with_state(state)
}

pub fn build_router_dev(state: AppState) -> Router {
    api_router(state.api_security.clone())
        .layer(DefaultBodyLimit::max(48 * 1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(64))
        .with_state(state)
}

fn no_store() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-store"),
    )
}

fn api_router(security: std::sync::Arc<crate::app::ApiSecurity>) -> Router<AppState> {
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
            get(providers::read_model_provider)
                .put(providers::update_model_provider)
                .delete(providers::delete_model_provider),
        )
        .route(
            "/api/model-providers/{provider_id}/models",
            get(providers::list_provider_models),
        )
        .route(
            "/api/model-providers/{provider_id}/test",
            post(providers::test_provider),
        )
        .route(
            "/api/model-providers/{provider_id}/models/{model_id}/test",
            post(providers::test_model),
        )
        .route(
            "/api/ip-proxies",
            get(ip_proxies::list_ip_proxies).post(ip_proxies::create_ip_proxy),
        )
        .route(
            "/api/ip-proxies/import",
            post(ip_proxies::import_ip_proxies),
        )
        .route(
            "/api/ip-proxies/{proxy_id}",
            get(ip_proxies::read_ip_proxy)
                .put(ip_proxies::update_ip_proxy)
                .delete(ip_proxies::delete_ip_proxy),
        )
        .route(
            "/api/ip-proxies/{proxy_id}/test",
            post(ip_proxies::test_ip_proxy),
        )
        .route(
            "/api/agents",
            get(agents::list_agents).post(agents::create_agent),
        )
        .route(
            "/api/agents/{agent_id}",
            get(agents::read_agent)
                .put(agents::update_agent)
                .delete(agents::delete_agent),
        )
        .route(
            "/api/agents/{agent_id}/workspace",
            get(agents::read_agent_workspace),
        )
        .route(
            "/api/agents/{agent_id}/profile",
            put(agents::update_agent_profile),
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
            "/api/conversations/{conversation_id}/title",
            post(conversations::generate_conversation_title),
        )
        .route(
            "/api/conversations/{conversation_id}/messages/page",
            get(conversations::read_conversation_message_page),
        )
        .route(
            "/api/conversations/{conversation_id}/usage",
            get(conversations::read_conversation_usage),
        )
        .route(
            "/api/conversations/{conversation_id}/task-plan",
            get(conversations::read_task_plan),
        )
        .route(
            "/api/conversations/{conversation_id}/task-plans",
            get(conversations::read_task_plans),
        )
        .route("/api/assets/local", get(assets::read_local_asset))
        .route("/api/assets/image", get(assets::read_local_asset))
        .route(
            "/api/conversations/{conversation_id}/runs",
            post(conversations::create_message_run),
        )
        .route(
            "/api/conversations/{conversation_id}/heartbeat",
            get(scheduling::read_heartbeat)
                .put(scheduling::set_heartbeat)
                .delete(scheduling::delete_heartbeat),
        )
        .route("/api/heartbeats", get(scheduling::list_heartbeats))
        .route(
            "/api/conversations/{conversation_id}/heartbeat/pause",
            post(scheduling::pause_heartbeat),
        )
        .route(
            "/api/conversations/{conversation_id}/heartbeat/resume",
            post(scheduling::resume_heartbeat),
        )
        .route(
            "/api/conversations/{conversation_id}/heartbeat/run",
            post(scheduling::run_heartbeat),
        )
        .route(
            "/api/conversations/{conversation_id}/heartbeat/executions",
            get(scheduling::list_heartbeat_executions),
        )
        .route(
            "/api/cron",
            get(scheduling::list_cron_tasks).post(scheduling::create_cron_task),
        )
        .route(
            "/api/cron/{task_id}",
            get(scheduling::read_cron_task)
                .patch(scheduling::update_cron_task)
                .delete(scheduling::delete_cron_task),
        )
        .route(
            "/api/cron/{task_id}/pause",
            post(scheduling::pause_cron_task),
        )
        .route(
            "/api/cron/{task_id}/resume",
            post(scheduling::resume_cron_task),
        )
        .route("/api/cron/{task_id}/run", post(scheduling::run_cron_task))
        .route(
            "/api/cron/{task_id}/executions",
            get(scheduling::list_cron_executions),
        )
        .route(
            "/api/conversations/{conversation_id}/run",
            get(conversations::active_conversation_run),
        )
        .route(
            "/api/runs/{run_id}/events",
            get(conversations::stream_run_events),
        )
        .route("/api/runs/{run_id}/steer", post(conversations::steer_run))
        .route(
            "/api/runs/{run_id}",
            get(conversations::read_run).delete(conversations::cancel_run),
        )
        .route(
            "/api/agents/{agent_id}/tools-config",
            get(tools::read_agent_tools_config).put(tools::update_agent_tools_config),
        )
        .route("/api/skills", get(skills::list_skills))
        .route("/api/skills/reload", post(skills::reload_skills))
        .route("/api/skills/changes", get(skills::skill_changes))
        .route("/api/skills/groups", post(skills::create_skill_group))
        .route(
            "/api/skills/groups/order",
            put(skills::reorder_skill_groups),
        )
        .route("/api/skills/import", post(skills::import_skill))
        .route("/api/skills/import-tasks", get(skills::list_import_tasks))
        .route("/api/skills/cleanup-orphans", post(skills::cleanup_orphans))
        .route(
            "/api/skills/{skill_name}/content",
            get(skills::read_skill_content),
        )
        .route(
            "/api/skills/{skill_name}/update",
            post(skills::update_skill),
        )
        .route(
            "/api/skills/{skill_name}/enabled",
            put(skills::update_skill_enabled),
        )
        .route(
            "/api/skills/{skill_name}/group",
            put(skills::assign_skill_group),
        )
        .route(
            "/api/skills/{skill_name}",
            get(skills::read_skill)
                .put(skills::update_skill_icon)
                .delete(skills::delete_skill),
        )
        .route(
            "/api/skills/groups/{group_id}/enabled",
            put(skills::update_skill_group_enabled),
        )
        .route(
            "/api/skills/groups/{group_id}",
            patch(skills::rename_skill_group).delete(skills::delete_skill_group),
        )
        .route(
            "/api/skills/groups/{group_id}/skills/order",
            put(skills::reorder_skill_group_items),
        )
        .route(
            "/api/agents/{agent_id}/skills-config",
            get(skills::read_agent_skills).put(skills::update_agent_skills),
        )
        .route(
            "/api/tools/defaults",
            get(tools::read_tool_defaults).put(tools::update_tool_defaults),
        )
        .route(
            "/api/tools/mcp",
            get(tools::list_mcp_servers).post(tools::create_mcp_server),
        )
        .route(
            "/api/tools/mcp/{server_id}",
            get(tools::read_mcp_server)
                .put(tools::update_mcp_server)
                .delete(tools::delete_mcp_server),
        )
        .route(
            "/api/tools/mcp/{server_id}/enabled",
            put(tools::update_mcp_server_enabled),
        )
        .route(
            "/api/tools/mcp/{server_id}/secrets",
            get(tools::read_mcp_server_secrets),
        )
        .route(
            "/api/tools/mcp/{server_id}/refresh",
            post(tools::refresh_mcp_server),
        )
        .route(
            "/api/tools/mcp/{server_id}/test",
            post(tools::test_mcp_server),
        )
        .route(
            "/api/tools/mcp/{server_id}/auth",
            get(tools::get_mcp_auth_state),
        )
        .route(
            "/api/tools/mcp/{server_id}/auth/reauthorize",
            post(tools::reauthorize_mcp_server),
        )
        .route(
            "/api/tools/mcp/{server_id}/auth/clear",
            post(tools::clear_mcp_auth),
        )
        .route(
            "/api/tools/mcp/{server_id}/tools/{tool_name}",
            put(tools::update_mcp_tool_enabled),
        )
        .route(
            "/api/tools/mcp/{server_id}/tools",
            put(tools::update_mcp_tools_enabled),
        )
        .route(
            "/api/agents/{agent_id}/mcp-config",
            get(tools::read_agent_mcp_config).put(tools::update_agent_mcp_config),
        )
        .route("/api/tools/pick-directory", post(tools::pick_directory))
        .route(
            "/api/tools/web-search-keys/check",
            post(tools::check_web_search_keys),
        )
        .route(
            "/api/tools/web-search-keys/import",
            post(tools::import_web_search_keys),
        )
        .route(
            "/api/tools/web-search-keys/delete",
            post(tools::delete_web_search_key),
        )
        .route(
            "/api/tools/environment-runtime",
            get(tools::read_environment_runtime),
        )
        .route(
            "/api/tools/environment-runtime/{component}",
            post(tools::install_environment_component),
        )
        .route(
            "/api/tools/environment-runtime/{component}/refresh",
            post(tools::refresh_environment_component),
        )
        .route("/api/tools/approve", post(tools::approve_tool_call))
        .route("/api/tools/processes", get(tools::list_tool_processes))
        .route(
            "/api/tools/processes/{process_id}",
            get(tools::get_tool_process).delete(tools::stop_tool_process),
        )
        .route("/api/terminal/sessions", get(tools::list_terminal_sessions))
        .route(
            "/api/terminal/sessions/{process_id}",
            get(tools::get_terminal_session).delete(tools::terminate_terminal_session),
        )
        .route(
            "/api/terminal/sessions/{process_id}/input",
            post(tools::write_terminal_session),
        )
        .route(
            "/api/terminal/sessions/{process_id}/resize",
            post(tools::resize_terminal_session),
        )
        .route(
            "/api/terminal/sessions/{process_id}/interrupt",
            post(tools::interrupt_terminal_session),
        )
        .route("/api/llm-logs", get(llm_logs::list_llm_logs))
        .route("/api/llm-logs/{log_id}", get(llm_logs::get_llm_log))
        .layer(no_store())
        .layer(middleware::from_fn_with_state(
            security,
            enforce_api_security,
        ))
}
