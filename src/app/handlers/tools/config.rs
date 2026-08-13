use axum::{
    Json,
    extract::{Path, State},
};
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings},
    error::AppResult,
};

pub async fn read_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentToolsSettings>> {
    let settings = state.store.read_agent_tools_settings(agent_id).await?;
    Ok(Json(settings.redact_secrets()))
}

pub async fn update_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(overrides): Json<AgentToolsOverrides>,
) -> AppResult<Json<AgentToolsSettings>> {
    let settings = state
        .store
        .update_agent_tools_overrides(agent_id, overrides)
        .await?;
    Ok(Json(settings.redact_secrets()))
}

pub async fn read_tool_defaults(
    State(state): State<AppState>,
) -> AppResult<Json<AgentToolsConfig>> {
    let config = state.store.read_tool_defaults().await?;
    Ok(Json(config.redact_secrets()))
}

pub async fn update_tool_defaults(
    State(state): State<AppState>,
    Json(config): Json<AgentToolsConfig>,
) -> AppResult<Json<AgentToolsConfig>> {
    let config = state.store.update_tool_defaults(config).await?;
    if config.evidence_storage.enabled
        && let Err(error) = crate::tools::web_search::evidence::cleanup_evidence_store(
            &state.store.data_dir,
            &config.evidence_storage,
        )
        .await
    {
        tracing::warn!(error = %error, "failed to clean evidence store after settings update");
    }
    Ok(Json(config.redact_secrets()))
}
