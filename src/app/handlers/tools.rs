use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings, ApprovalDecision,
        ApprovalRequest, ApprovalResponse, CodexRuntimeStatus, ProcessInfo, RuntimeComponentKind,
    },
    error::{AppError, AppResult},
    tools::approval::ApprovalResolution,
};

pub async fn read_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentToolsSettings>> {
    Ok(Json(state.store.read_agent_tools_settings(agent_id).await?))
}

pub async fn update_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(overrides): Json<AgentToolsOverrides>,
) -> AppResult<Json<AgentToolsSettings>> {
    Ok(Json(
        state
            .store
            .update_agent_tools_overrides(agent_id, overrides)
            .await?,
    ))
}

pub async fn read_tool_defaults(
    State(state): State<AppState>,
) -> AppResult<Json<AgentToolsConfig>> {
    Ok(Json(state.store.read_tool_defaults().await?))
}

pub async fn update_tool_defaults(
    State(state): State<AppState>,
    Json(config): Json<AgentToolsConfig>,
) -> AppResult<Json<AgentToolsConfig>> {
    Ok(Json(state.store.update_tool_defaults(config).await?))
}

pub async fn read_codex_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(state.codex_runtime.status().await))
}

pub async fn refresh_codex_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(state.codex_runtime.refresh().await?))
}

pub async fn install_codex_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(
        state
            .codex_runtime
            .install(RuntimeComponentKind::Codex)
            .await?,
    ))
}

pub async fn install_runtime_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(state.codex_runtime.install(component).await?))
}

pub async fn refresh_runtime_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(
        state.codex_runtime.refresh_component(component).await?,
    ))
}

pub async fn approve_tool_call(
    State(state): State<AppState>,
    Json(request): Json<ApprovalRequest>,
) -> AppResult<Json<ApprovalResponse>> {
    let resolution = state.approval_manager.resolve(
        &request.approval_id,
        request.agent_id,
        request.decision.clone(),
    );
    match resolution {
        ApprovalResolution::Resolved(entry) => match request.decision {
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
        ApprovalResolution::AlreadyResolved(decision) => Ok(Json(ApprovalResponse {
            status: approval_status(&decision).to_string(),
            message: Some("Approval was already processed".to_string()),
        })),
        ApprovalResolution::AgentMismatch => Err(AppError::BadRequest(
            "Approval belongs to another agent".to_string(),
        )),
        ApprovalResolution::NotFound => Err(AppError::BadRequest(format!(
            "Approval '{}' not found or expired",
            request.approval_id
        ))),
    }
}

fn approval_status(decision: &ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approve => "approved",
        ApprovalDecision::Reject => "rejected",
        ApprovalDecision::AlwaysApprove => "always_approved",
    }
}

fn approval_summary(prefix: &str, entry: &crate::domain::PendingApproval) -> String {
    format!(
        "{prefix}: {} (agent={}, conversation={}, call={}, cwd={}, runtime={}, permission={}, justification={}, background={})",
        entry.command,
        entry.agent_id,
        entry.conversation_id,
        entry.tool_call_id,
        entry.cwd.as_deref().unwrap_or("<default>"),
        entry.runtime,
        entry.permission,
        entry.justification.as_deref().unwrap_or("<none>"),
        entry.background,
    )
}

#[derive(Debug, Deserialize)]
pub struct ProcessListQuery {
    pub agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ProcessOwnerQuery {
    pub agent_id: Uuid,
}

pub async fn list_tool_processes(
    State(state): State<AppState>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<Vec<ProcessInfo>>> {
    Ok(Json(state.terminal_manager.list(query.agent_id).await))
}

pub async fn get_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
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
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    state
        .terminal_manager
        .terminate(&process_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}
