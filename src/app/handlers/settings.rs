use axum::{Json, extract::State};

use crate::{app::AppState, domain::AppSettings, error::AppResult};

pub async fn read_settings(State(state): State<AppState>) -> AppResult<Json<AppSettings>> {
    Ok(Json(state.store.read_settings().await?))
}

pub async fn update_settings(
    State(state): State<AppState>,
    Json(request): Json<AppSettings>,
) -> AppResult<Json<AppSettings>> {
    Ok(Json(state.store.write_settings(request).await?))
}
