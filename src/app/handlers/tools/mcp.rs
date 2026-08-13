use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        McpAgentOverrides, McpAgentSettings, McpAuthState, McpServer, McpServerDraft,
        McpServerStatus,
    },
    error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct McpToolEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct McpToolsEnabledRequest {
    pub tool_names: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct McpServerEnabledRequest {
    pub enabled: bool,
}

pub async fn list_mcp_servers(State(state): State<AppState>) -> AppResult<Json<Vec<McpServer>>> {
    let servers = state
        .mcp_manager
        .list_servers()
        .await?
        .into_iter()
        .map(|server| server.redacted())
        .collect();
    Ok(Json(servers))
}

pub async fn read_mcp_server_secrets(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<crate::domain::McpTransport>> {
    let transport = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|server| server.id == server_id)
        .map(|server| server.transport)
        .ok_or(AppError::NotFound)?;
    Ok(Json(transport))
}

pub async fn create_mcp_server(
    State(state): State<AppState>,
    Json(draft): Json<McpServerDraft>,
) -> AppResult<Json<McpServer>> {
    let mut server = state.store.create_mcp_server(draft).await?;
    let server_id = server.id;
    if server.enabled {
        let _ = state
            .store
            .update_mcp_server_status(server_id, McpServerStatus::Connecting, None, None)
            .await;
        server.status = McpServerStatus::Connecting;
        server.last_error = None;
    }
    let mcp_manager = state.mcp_manager.clone();
    tokio::spawn(async move {
        refresh_mcp_server_in_background(mcp_manager, server_id).await;
    });
    Ok(Json(server.redacted()))
}

pub async fn update_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(mut draft): Json<McpServerDraft>,
) -> AppResult<Json<McpServer>> {
    draft.id = Some(server_id);
    let mut server = state.store.update_mcp_server(draft).await?;
    if server.enabled {
        let _ = state
            .store
            .update_mcp_server_status(server_id, McpServerStatus::Connecting, None, None)
            .await;
        server.status = McpServerStatus::Connecting;
        server.last_error = None;
    }
    let mcp_manager = state.mcp_manager.clone();
    tokio::spawn(async move {
        refresh_mcp_server_in_background(mcp_manager, server_id).await;
    });
    Ok(Json(server.redacted()))
}

async fn refresh_mcp_server_in_background(
    mcp_manager: std::sync::Arc<crate::tools::mcp::McpManager>,
    server_id: Uuid,
) {
    mcp_manager.stop_server(server_id).await;
    let enabled = match mcp_manager.list_servers().await {
        Ok(servers) => servers
            .into_iter()
            .find(|server| server.id == server_id)
            .is_some_and(|server| server.enabled),
        Err(error) => {
            tracing::debug!(%error, %server_id, "MCP background refresh could not read server state");
            return;
        }
    };
    if !enabled {
        return;
    }
    if let Err(error) = mcp_manager.refresh_server(server_id).await {
        tracing::debug!(%error, %server_id, "MCP background refresh failed");
    }
}

pub async fn update_mcp_server_enabled(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(request): Json<McpServerEnabledRequest>,
) -> AppResult<Json<McpServer>> {
    if !request.enabled {
        state.mcp_manager.stop_server(server_id).await;
    }
    let server = state
        .store
        .update_mcp_server_enabled(server_id, request.enabled)
        .await?;
    Ok(Json(server.redacted()))
}

pub async fn delete_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<()> {
    state.mcp_manager.delete_server(server_id).await
}

pub async fn refresh_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpServer>> {
    Ok(Json(
        state
            .mcp_manager
            .refresh_server(server_id)
            .await?
            .redacted(),
    ))
}

pub async fn test_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpServer>> {
    Ok(Json(
        state
            .mcp_manager
            .refresh_server(server_id)
            .await?
            .redacted(),
    ))
}

pub async fn get_mcp_auth_state(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpAuthState>> {
    Ok(Json(state.mcp_manager.auth_state(server_id).await?))
}

pub async fn reauthorize_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpAuthState>> {
    Ok(Json(state.mcp_manager.reauthorize(server_id).await?))
}

pub async fn clear_mcp_auth(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpAuthState>> {
    Ok(Json(state.mcp_manager.clear_auth(server_id).await?))
}

pub async fn update_mcp_tool_enabled(
    State(state): State<AppState>,
    Path((server_id, tool_name)): Path<(Uuid, String)>,
    Json(request): Json<McpToolEnabledRequest>,
) -> AppResult<Json<McpServer>> {
    state
        .store
        .update_mcp_tool_enabled(server_id, &tool_name, request.enabled)
        .await?;
    let server = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|item| item.id == server_id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(server.redacted()))
}

pub async fn update_mcp_tools_enabled(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(request): Json<McpToolsEnabledRequest>,
) -> AppResult<Json<McpServer>> {
    let names = request
        .tool_names
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err(AppError::BadRequest(
            "at least one MCP tool is required".to_string(),
        ));
    }
    state
        .store
        .update_mcp_tools_enabled(server_id, &names, request.enabled)
        .await?;
    let server = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|item| item.id == server_id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(server.redacted()))
}

pub async fn read_agent_mcp_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<McpAgentSettings>> {
    state.store.get_agent(agent_id).await?;
    let settings = state.store.read_mcp_agent_settings(agent_id).await?;
    Ok(Json(McpAgentSettings {
        defaults: settings
            .defaults
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
        overrides: settings.overrides,
        effective: settings
            .effective
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
    }))
}

pub async fn update_agent_mcp_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(overrides): Json<McpAgentOverrides>,
) -> AppResult<Json<McpAgentSettings>> {
    state.store.get_agent(agent_id).await?;
    let settings = state
        .store
        .update_mcp_agent_overrides(agent_id, overrides)
        .await?;
    Ok(Json(McpAgentSettings {
        defaults: settings
            .defaults
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
        overrides: settings.overrides,
        effective: settings
            .effective
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
    }))
}
