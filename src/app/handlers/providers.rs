use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        Agent, ChatMessage, ChatMessageStatus, ChatRole, CreateModelProviderRequest,
        FetchProviderModelsRequest, ModelProvider, ModelProviderResponse,
        UpdateModelProviderRequest,
    },
    error::AppResult,
    llm::client::RequestOptions,
};

#[derive(Debug, Deserialize)]
pub struct ModelTestRequest {
    pub prompt: String,
}

pub async fn list_model_providers(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ModelProviderResponse>>> {
    Ok(Json(state.store.list_model_providers().await?))
}

pub async fn read_model_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> AppResult<Json<ModelProviderResponse>> {
    let provider = state.store.get_model_provider(provider_id).await?;
    Ok(Json(ModelProviderResponse::from(&provider)))
}

pub async fn create_model_provider(
    State(state): State<AppState>,
    Json(request): Json<CreateModelProviderRequest>,
) -> AppResult<Json<ModelProviderResponse>> {
    Ok(Json(state.store.create_model_provider(request).await?))
}

pub async fn fetch_provider_models(
    State(state): State<AppState>,
    Json(request): Json<FetchProviderModelsRequest>,
) -> AppResult<Json<Vec<String>>> {
    let provider = ModelProvider {
        id: Uuid::nil(),
        provider_type: request.provider_type,
        name: String::new(),
        base_url: request.base_url,
        image_generation_url: request.image_generation_url,
        image_edit_url: request.image_edit_url,
        api_keys: vec![request.api_key],
        key_strategy: Default::default(),
        models: Vec::new(),
        model_configs: Vec::new(),
        extra_body: request.extra_body,
        prompt_cache: Default::default(),
        cache_capabilities: None,
        proxy_url: request.proxy_url,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    if provider.provider_type == crate::domain::ProviderType::OpenAiImages {
        let client = state.llm_registry.image_client_for(&provider)?;
        return Ok(Json(client.list_models(&provider).await?));
    }
    let client = state.llm_registry.client_for(&provider)?;
    Ok(Json(client.list_models(&provider).await?))
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
    if provider.provider_type == crate::domain::ProviderType::OpenAiImages {
        let client = state.llm_registry.image_client_for(&provider)?;
        return Ok(Json(client.list_models(&provider).await?));
    }
    let client = state.llm_registry.client_for(&provider)?;
    Ok(Json(client.list_models(&provider).await?))
}

pub async fn test_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let started = std::time::Instant::now();
    let provider = state.store.get_model_provider(provider_id).await?;
    let client = state.llm_registry.client_for(&provider)?;
    let models = client.list_models(&provider).await?;
    Ok(Json(serde_json::json!({
        "provider_id": provider_id,
        "elapsed_ms": started.elapsed().as_millis(),
        "models": models,
    })))
}

pub async fn test_model(
    State(state): State<AppState>,
    Path((provider_id, model_id)): Path<(Uuid, String)>,
    Json(request): Json<ModelTestRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let provider = state.store.get_model_provider(provider_id).await?;
    let agent = Agent {
        id: Uuid::nil(),
        name: "CLI model test".to_string(),
        description: String::new(),
        model_provider_id: provider_id,
        model_id: model_id.clone(),
        is_builtin: false,
        avatar_icon: String::new(),
        avatar_color: String::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: Uuid::nil(),
        role: ChatRole::User,
        content: request.prompt,
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
    };
    let settings = state.store.read_settings().await?;
    let started = std::time::Instant::now();
    let output = state
        .llm_registry
        .client_for(&provider)?
        .complete(
            &agent,
            &provider,
            &[message],
            "Reply briefly and do not call tools.",
            RequestOptions {
                timeout: std::time::Duration::from_secs(settings.request_timeout_seconds.max(1)),
                retry_attempts: settings.request_retry_attempts.max(1),
                thinking_level: None,
                on_retry: None,
                is_compaction: false,
            },
            None,
        )
        .await?;
    Ok(Json(serde_json::json!({
        "provider_id": provider_id,
        "model_id": model_id,
        "elapsed_ms": started.elapsed().as_millis(),
        "content": output.content,
        "usage": output.usage,
    })))
}
