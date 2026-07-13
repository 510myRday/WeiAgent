use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{AgentToolsConfig, ApprovalDecision, ApprovalRequest, ApprovalResponse, ProcessInfo},
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
    let pending = state.approval_manager.resolve(
        &request.approval_id,
        request.agent_id,
        request.decision.clone(),
    );
    match pending {
        Some(entry) => match request.decision {
            ApprovalDecision::Approve => Ok(Json(ApprovalResponse {
                status: "approved".to_string(),
                message: Some(approval_summary("Command approved", &entry)),
            })),
            ApprovalDecision::Reject => Ok(Json(ApprovalResponse {
                status: "rejected".to_string(),
                message: Some(approval_summary("Command rejected by user", &entry)),
            })),
            ApprovalDecision::AlwaysApprove => Ok(Json(ApprovalResponse {
                status: "always_approved".to_string(),
                message: Some(approval_summary(
                    "Command approved and a 24-hour context-bound grant will be stored",
                    &entry,
                )),
            })),
        },
        None => Err(AppError::BadRequest(format!(
            "Approval '{}' not found or expired",
            request.approval_id
        ))),
    }
}

fn approval_summary(prefix: &str, entry: &crate::domain::PendingApproval) -> String {
    format!(
        "{prefix}: {} (agent={}, conversation={}, call={}, cwd={}, runtime={}, network={}, write_roots={}, background={})",
        entry.command,
        entry.agent_id,
        entry.conversation_id,
        entry.tool_call_id,
        entry.cwd.as_deref().unwrap_or("<default>"),
        entry.runtime,
        entry.network_enabled,
        entry.write_roots.join(";"),
        entry.background,
    )
}

#[derive(Debug, Deserialize)]
pub struct ProcessListQuery {
    pub agent_id: Uuid,
}

pub async fn list_tool_processes(
    State(state): State<AppState>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<Vec<ProcessInfo>>> {
    Ok(Json(state.process_manager.list(Some(query.agent_id)).await))
}

pub async fn get_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .process_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(info))
}

pub async fn stop_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .process_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    state
        .process_manager
        .stop(&process_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}
