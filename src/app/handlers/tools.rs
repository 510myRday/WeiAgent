use axum::{
    Json,
    extract::{Path, State},
};
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{AgentToolsConfig, ApprovalDecision, ApprovalRequest, ApprovalResponse},
    error::{AppError, AppResult},
};

pub async fn read_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentToolsConfig>> {
    Ok(Json(state.store.read_agent_tools_config(agent_id).await?))
}

pub async fn update_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(config): Json<AgentToolsConfig>,
) -> AppResult<Json<AgentToolsConfig>> {
    Ok(Json(
        state
            .store
            .update_agent_tools_config(agent_id, config)
            .await?,
    ))
}

pub async fn approve_tool_call(
    State(state): State<AppState>,
    Json(request): Json<ApprovalRequest>,
) -> AppResult<Json<ApprovalResponse>> {
    // 清理过期审批
    state.approval_manager.cleanup_expired();

    let pending = state
        .approval_manager
        .resolve(&request.approval_id, request.decision.clone());
    match pending {
        Some(entry) => match request.decision {
            ApprovalDecision::Approve => Ok(Json(ApprovalResponse {
                status: "approved".to_string(),
                message: Some(format!("Command approved: {}", entry.command)),
            })),
            ApprovalDecision::Reject => Ok(Json(ApprovalResponse {
                status: "rejected".to_string(),
                message: Some("Command rejected by user".to_string()),
            })),
            ApprovalDecision::AlwaysApprove => Ok(Json(ApprovalResponse {
                status: "approved".to_string(),
                message: Some(format!("Command approved: {}", entry.command)),
            })),
        },
        None => Err(AppError::BadRequest(format!(
            "Approval '{}' not found or expired",
            request.approval_id
        ))),
    }
}
