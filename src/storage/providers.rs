use chrono::Utc;
use uuid::Uuid;

use crate::{
    domain::{
        CreateModelProviderRequest, ModelProvider, ModelProviderResponse,
        UpdateModelProviderRequest,
    },
    error::{AppError, AppResult},
    storage::{
        json_store::JsonStore,
        validation::{normalize_models, validate_non_empty},
    },
};

impl JsonStore {
    pub async fn list_model_providers(&self) -> AppResult<Vec<ModelProviderResponse>> {
        let providers = self.read_providers().await?;
        Ok(providers
            .iter()
            .map(ModelProviderResponse::from)
            .collect::<Vec<_>>())
    }

    pub async fn create_model_provider(
        &self,
        request: CreateModelProviderRequest,
    ) -> AppResult<ModelProviderResponse> {
        let provider_type = request.provider_type;
        let name = validate_non_empty("provider name", &request.name)?;
        let base_url = validate_non_empty("base_url", &request.base_url)?;
        let api_key = validate_non_empty("api_key", &request.api_key)?;
        let models = normalize_models(request.models)?;

        let _guard = self.lock.lock().await;
        let mut providers = self.read_providers_unlocked().await?;
        let now = Utc::now();
        let provider = ModelProvider {
            id: Uuid::new_v4(),
            provider_type,
            name,
            base_url,
            api_key,
            models,
            extra_body: request.extra_body,
            created_at: now,
            updated_at: now,
        };
        providers.push(provider.clone());
        self.write_providers_unlocked(&providers).await?;
        Ok(ModelProviderResponse::from(&provider))
    }

    pub async fn update_model_provider(
        &self,
        id: Uuid,
        request: UpdateModelProviderRequest,
    ) -> AppResult<ModelProviderResponse> {
        let _guard = self.lock.lock().await;
        let mut providers = self.read_providers_unlocked().await?;
        let agents = self.read_agents_unlocked().await?;
        let provider = providers
            .iter_mut()
            .find(|provider| provider.id == id)
            .ok_or(AppError::NotFound)?;

        if let Some(provider_type) = request.provider_type {
            provider.provider_type = provider_type;
        }
        if let Some(name) = request.name {
            provider.name = validate_non_empty("provider name", &name)?;
        }
        if let Some(base_url) = request.base_url {
            provider.base_url = validate_non_empty("base_url", &base_url)?;
        }
        if let Some(api_key) = request.api_key
            && !api_key.trim().is_empty()
        {
            provider.api_key = api_key.trim().to_string();
        }
        if let Some(models) = request.models {
            let models = normalize_models(models)?;
            let mut agents = agents;
            let mut agents_changed = false;
            for agent in agents
                .iter_mut()
                .filter(|agent| agent.model_provider_id == provider.id)
            {
                if !models.contains(&agent.model_id) {
                    agent.model_id = String::new();
                    agent.updated_at = Utc::now();
                    agents_changed = true;
                }
            }
            provider.models = models;
            if agents_changed {
                self.write_agents_unlocked(&agents).await?;
            }
        }
        if let Some(extra_body) = request.extra_body {
            provider.extra_body = extra_body;
        }

        provider.updated_at = Utc::now();
        let updated = provider.clone();
        self.write_providers_unlocked(&providers).await?;
        Ok(ModelProviderResponse::from(&updated))
    }

    pub async fn delete_model_provider(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if let Some(agent) = agents.iter().find(|agent| agent.model_provider_id == id) {
            return Err(AppError::BadRequest(format!(
                "provider is used by agent {}",
                agent.name
            )));
        }

        let mut providers = self.read_providers_unlocked().await?;
        let before = providers.len();
        providers.retain(|provider| provider.id != id);
        if providers.len() == before {
            return Err(AppError::NotFound);
        }
        self.write_providers_unlocked(&providers).await
    }

    pub async fn get_model_provider(&self, id: Uuid) -> AppResult<ModelProvider> {
        self.read_providers()
            .await?
            .into_iter()
            .find(|provider| provider.id == id)
            .ok_or(AppError::NotFound)
    }
}
