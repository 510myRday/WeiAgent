use axum::{
    Json,
    extract::{Path, State},
};

use crate::{
    app::AppState,
    domain::{EnvironmentRuntimeStatus, RuntimeComponentKind},
    error::AppResult,
};

pub async fn read_environment_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<EnvironmentRuntimeStatus>> {
    Ok(Json(state.environment_runtime.status().await))
}

pub async fn install_environment_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<EnvironmentRuntimeStatus>> {
    Ok(Json(
        state.environment_runtime.start_install(component).await?,
    ))
}

pub async fn refresh_environment_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<EnvironmentRuntimeStatus>> {
    Ok(Json(
        state
            .environment_runtime
            .refresh_component(component)
            .await?,
    ))
}
