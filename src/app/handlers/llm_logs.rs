use axum::{
    Json,
    extract::{Path, State},
};

use crate::{
    app::AppState,
    error::AppResult,
    storage::llm_logs::{LlmLogDetail, LlmLogEntry},
};

pub async fn list_llm_logs(State(state): State<AppState>) -> AppResult<Json<Vec<LlmLogEntry>>> {
    Ok(Json(state.llm_logger.list_logs().await?))
}

pub async fn get_llm_log(
    State(state): State<AppState>,
    Path(log_id): Path<String>,
) -> AppResult<Json<LlmLogDetail>> {
    Ok(Json(state.llm_logger.get_log(&log_id).await?))
}

pub async fn delete_llm_log(
    State(state): State<AppState>,
    Path(log_id): Path<String>,
) -> AppResult<()> {
    state.llm_logger.delete_log(&log_id).await
}
