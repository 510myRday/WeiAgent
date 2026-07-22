use axum::{Json, extract::State};

use crate::{
    app::AppState,
    domain::AppSettings,
    error::{AppError, AppResult},
};

pub async fn read_settings(State(state): State<AppState>) -> AppResult<Json<AppSettings>> {
    Ok(Json(state.store.read_settings().await?))
}

pub async fn update_settings(
    State(state): State<AppState>,
    Json(request): Json<AppSettings>,
) -> AppResult<Json<AppSettings>> {
    match (
        request.conversation_title_provider_id,
        request.conversation_title_model_id.as_deref(),
    ) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(AppError::BadRequest(
                "conversation title provider and model must be configured together".to_string(),
            ));
        }
        (Some(provider_id), Some(model_id)) => {
            let provider = state.store.get_model_provider(provider_id).await?;
            if !provider.models.iter().any(|model| model == model_id.trim()) {
                return Err(AppError::BadRequest(
                    "conversation title model is not configured for the selected provider"
                        .to_string(),
                ));
            }
        }
        (None, None) => {}
    }
    let updated = state.store.write_settings(request).await?;
    state.llm_logger.set_enabled(updated.llm_log_enabled);
    Ok(Json(updated))
}
