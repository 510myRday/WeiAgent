use axum::{
    Json,
    extract::{Path, State},
};
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{CreateModelProviderRequest, ModelProviderResponse, UpdateModelProviderRequest},
    error::AppResult,
};

pub async fn list_model_providers(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ModelProviderResponse>>> {
    Ok(Json(state.store.list_model_providers().await?))
}

pub async fn create_model_provider(
    State(state): State<AppState>,
    Json(request): Json<CreateModelProviderRequest>,
) -> AppResult<Json<ModelProviderResponse>> {
    Ok(Json(state.store.create_model_provider(request).await?))
}

pub async fn update_model_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<UpdateModelProviderRequest>,
) -> AppResult<Json<ModelProviderResponse>> {
    Ok(Json(
        state
            .store
            .update_model_provider(provider_id, request)
            .await?,
    ))
}

pub async fn delete_model_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> AppResult<()> {
    state.store.delete_model_provider(provider_id).await
}

pub async fn list_provider_models(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> AppResult<Json<Vec<String>>> {
    let provider = state.store.get_model_provider(provider_id).await?;
    let client = state.llm_registry.client_for(&provider)?;
    Ok(Json(client.list_models(&provider).await?))
}
