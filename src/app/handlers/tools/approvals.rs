use axum::{Json, extract::State};

use crate::{
    app::AppState,
    domain::{ApprovalDecision, ApprovalRequest, ApprovalResponse},
    error::{AppError, AppResult},
    tools::approval::ApprovalResolution,
};

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
