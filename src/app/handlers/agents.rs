use axum::{
    Json,
    extract::{Path, State},
};
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        AgentProfileResponse, AgentResponse, AgentWorkspace, CreateAgentRequest,
        UpdateAgentProfileRequest, UpdateAgentRequest, UpdateAgentWorkspaceRequest,
    },
    error::{AppError, AppResult},
};

pub async fn list_agents(State(state): State<AppState>) -> AppResult<Json<Vec<AgentResponse>>> {
    Ok(Json(state.store.list_agents().await?))
}

pub async fn create_agent(
    State(state): State<AppState>,
    Json(request): Json<CreateAgentRequest>,
) -> AppResult<Json<AgentResponse>> {
    Ok(Json(state.store.create_agent(request).await?))
}

pub async fn update_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<UpdateAgentRequest>,
) -> AppResult<Json<AgentResponse>> {
    Ok(Json(state.store.update_agent(agent_id, request).await?))
}

pub async fn update_agent_profile(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<UpdateAgentProfileRequest>,
) -> AppResult<Json<AgentProfileResponse>> {
    Ok(Json(
        state.store.update_agent_profile(agent_id, request).await?,
    ))
}

pub async fn delete_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<()> {
    if state.store.get_agent(agent_id).await?.is_builtin {
        return Err(AppError::BadRequest(
            "built-in agents cannot be deleted".to_string(),
        ));
    }
    let conversation_ids = state.store.list_conversation_ids(agent_id).await?;
    for conversation_id in conversation_ids {
        state
            .run_manager
            .cancel_conversation_and_wait(conversation_id)
            .await?;
    }
    state.terminal_manager.stop_for_agent(agent_id).await;
    state.store.delete_agent(agent_id).await
}

pub async fn read_agent_workspace(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentWorkspace>> {
    Ok(Json(state.store.read_agent_workspace(agent_id).await?))
}

pub async fn update_agent_workspace(
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
