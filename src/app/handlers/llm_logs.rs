use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use crate::{
    app::AppState,
    error::AppResult,
    storage::llm_logs::{LlmLogDetail, LlmLogEntry},
};

#[derive(Debug, Deserialize)]
pub struct LogListQuery {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_log_limit")]
    limit: usize,
}

fn default_log_limit() -> usize {
    200
}

pub async fn list_llm_logs(
    State(state): State<AppState>,
    Query(query): Query<LogListQuery>,
) -> AppResult<Json<Vec<LlmLogEntry>>> {
    Ok(Json(
        state
            .llm_logger
            .list_logs(query.offset, query.limit)
            .await?,
    ))
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
