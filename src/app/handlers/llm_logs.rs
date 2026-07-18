use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use crate::{
    app::AppState,
    error::AppResult,
    storage::llm_logs::{LlmLogDetail, LlmLogListFilter, LlmLogListPage},
};

#[derive(Debug, Deserialize)]
pub struct LogListQuery {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_log_limit")]
    limit: usize,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    agent_name: Option<String>,
    #[serde(default)]
    provider_name: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

fn default_log_limit() -> usize {
    10
}

pub async fn list_llm_logs(
    State(state): State<AppState>,
    Query(query): Query<LogListQuery>,
) -> AppResult<Json<LlmLogListPage>> {
    let filter = LlmLogListFilter {
        agent_id: query.agent_id.filter(|value| !value.trim().is_empty()),
        agent_name: query.agent_name.filter(|value| !value.trim().is_empty()),
        provider_name: query.provider_name.filter(|value| !value.trim().is_empty()),
        model_id: query.model_id.filter(|value| !value.trim().is_empty()),
        q: query.q.filter(|value| !value.trim().is_empty()),
        since: query.since.filter(|value| !value.trim().is_empty()),
        until: query.until.filter(|value| !value.trim().is_empty()),
    };
    Ok(Json(
        state
            .llm_logger
            .list_logs_page(query.offset, query.limit, filter)
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
