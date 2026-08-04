use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    domain::{
        CreateModelProviderRequest, ModelConfig, ModelProvider, ModelProviderResponse,
        UpdateModelProviderRequest, merge_api_keys,
    },
    error::{AppError, AppResult},
    storage::{
        db,
        store::Store,
        validation::{normalize_models, validate_non_empty},
    },
};

impl Store {
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
        let api_keys = merge_api_keys(&[], &request.api_keys);
        let models = normalize_models(request.models)?;
        let model_configs = normalize_model_configs(&models, request.model_configs)?;
        validate_extra_body(&request.extra_body)?;
        let proxy_url = normalize_provider_proxy_url(request.proxy_url)?;
        let image_generation_url = normalize_optional_url(request.image_generation_url)?;
        let image_edit_url = normalize_optional_url(request.image_edit_url)?;

        let _guard = self.lock.lock().await;
        let now = Utc::now();
        let provider = ModelProvider {
            id: Uuid::new_v4(),
            provider_type,
            name,
            base_url,
            image_generation_url,
            image_edit_url,
            api_keys,
            key_strategy: request.key_strategy,
            models,
            model_configs,
            extra_body: request.extra_body,
            prompt_cache: request.prompt_cache,
            cache_capabilities: request.cache_capabilities,
            proxy_url,
            created_at: now,
            updated_at: now,
        };
        let stored = provider.clone();
        self.database
            .call(move |connection| db::upsert_provider_conn(connection, &stored))
            .await?;
        Ok(ModelProviderResponse::from(&provider))
    }

    pub async fn update_model_provider(
        &self,
        id: Uuid,
        request: UpdateModelProviderRequest,
    ) -> AppResult<ModelProviderResponse> {
        let _guard = self.lock.lock().await;
        let mut providers = self
            .database
            .call(|connection| db::read_providers_conn(connection))
            .await?;
        let mut agents = self.read_agents_unlocked().await?;
        let original_agents = agents.clone();
        let mut agents_changed = false;
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
        if let Some(image_generation_url) = request.image_generation_url {
            provider.image_generation_url = normalize_optional_url(image_generation_url)?;
        }
        if let Some(image_edit_url) = request.image_edit_url {
            provider.image_edit_url = normalize_optional_url(image_edit_url)?;
        }
        if let Some(submitted) = request.api_keys {
            provider.api_keys = merge_api_keys(&provider.api_keys, &submitted);
        }
        if let Some(key_strategy) = request.key_strategy {
            provider.key_strategy = key_strategy;
        }
        let requested_model_configs = request.model_configs;
        if let Some(models) = request.models {
            let models = normalize_models(models)?;
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
        }
        provider.model_configs = normalize_model_configs(
            &provider.models,
            requested_model_configs.unwrap_or_else(|| provider.model_configs.clone()),
        )?;
        if let Some(extra_body) = request.extra_body {
            validate_extra_body(&extra_body)?;
            provider.extra_body = extra_body;
        }
        if let Some(prompt_cache) = request.prompt_cache {
            provider.prompt_cache = prompt_cache;
        }
        if let Some(cache_capabilities) = request.cache_capabilities {
            provider.cache_capabilities = cache_capabilities;
        }
        if let Some(proxy_url) = request.proxy_url {
            provider.proxy_url = normalize_provider_proxy_url(proxy_url)?;
        }

        provider.updated_at = Utc::now();
        let updated = provider.clone();
        if agents_changed {
            self.write_agents_unlocked(&agents).await?;
        }
        let stored = updated.clone();
        if let Err(error) = self
            .database
            .call(move |connection| db::upsert_provider_conn(connection, &stored))
            .await
        {
            if agents_changed {
                let _ = self.write_agents_unlocked(&original_agents).await;
            }
            return Err(error);
        }
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

        if !self
            .database
            .call(move |connection| db::delete_provider_conn(connection, id))
            .await?
        {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    pub async fn get_model_provider(&self, id: Uuid) -> AppResult<ModelProvider> {
        self.read_providers()
            .await?
            .into_iter()
            .find(|provider| provider.id == id)
            .ok_or(AppError::NotFound)
    }
}

fn normalize_model_configs(
    models: &[String],
    configs: Vec<ModelConfig>,
) -> AppResult<Vec<ModelConfig>> {
    let mut configs_by_id = configs
        .into_iter()
        .map(|mut config| {
            config.model_id = config.model_id.trim().to_string();
            (config.model_id.clone(), config)
        })
        .filter(|(model_id, _)| !model_id.is_empty())
        .collect::<HashMap<_, _>>();

    models
        .iter()
        .map(|model_id| {
            let config = configs_by_id
                .remove(model_id)
                .unwrap_or_else(|| default_model_config(model_id));
            validate_context_size(config.context_size)?;
            validate_model_price("input_price", config.input_price)?;
            validate_model_price("output_price", config.output_price)?;
            validate_model_price("cache_write_price", config.cache_write_price)?;
            validate_model_price("cache_read_price", config.cache_read_price)?;
            Ok(config)
        })
        .collect()
}

fn default_model_config(model_id: &str) -> ModelConfig {
    ModelConfig {
        model_id: model_id.to_string(),
        multimodal: false,
        thinking: false,
        context_size: 128_000,
        input_price: None,
        output_price: None,
        cache_write_price: None,
        cache_read_price: None,
        image_options: None,
    }
}

fn normalize_optional_url(value: Option<String>) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    reqwest::Url::parse(&value)
        .map_err(|_| AppError::BadRequest("image endpoint URL must be valid".to_string()))?;
    Ok(Some(value))
}

fn validate_context_size(context_size: u32) -> AppResult<()> {
    if context_size == 0 {
        return Err(AppError::BadRequest(
            "context_size must be a positive integer".to_string(),
        ));
    }
    Ok(())
}

fn validate_model_price(field: &str, price: Option<f64>) -> AppResult<()> {
    if price.is_none_or(|value| value.is_finite() && value >= 0.0) {
        return Ok(());
    }
    Err(AppError::BadRequest(format!(
        "{field} must be a non-negative finite number"
    )))
}

fn normalize_provider_proxy_url(value: Option<String>) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if !(value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("socks4://")
        || value.starts_with("socks4a://")
        || value.starts_with("socks5://")
        || value.starts_with("socks5h://")
        || value.starts_with("socks://"))
    {
        return Err(AppError::BadRequest(
            "provider proxy_url must start with http://, https://, socks4://, socks4a://, socks5:// or socks5h://"
                .to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

fn validate_extra_body(extra_body: &serde_json::Value) -> AppResult<()> {
    if extra_body.is_null() || extra_body.is_object() {
        return Ok(());
    }
    Err(AppError::BadRequest(
        "extra_body must be a JSON object".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{normalize_model_configs, validate_extra_body};
    use crate::domain::ModelConfig;

    #[test]
    fn extra_body_accepts_objects_and_null() {
        assert!(validate_extra_body(&serde_json::Value::Null).is_ok());
        assert!(validate_extra_body(&serde_json::json!({"temperature": 0.2})).is_ok());
    }

    #[test]
    fn extra_body_rejects_non_objects() {
        assert!(validate_extra_body(&serde_json::json!(["invalid"])).is_err());
    }

    #[test]
    fn model_configs_follow_the_model_list_and_fill_defaults() {
        let configs = normalize_model_configs(
            &["model-a".to_string(), "model-b".to_string()],
            vec![ModelConfig {
                model_id: "model-b".to_string(),
                multimodal: true,
                thinking: false,
                context_size: 200_000,
                input_price: Some(0.25),
                output_price: Some(1.5),
                cache_write_price: Some(0.08),
                cache_read_price: Some(0.05),
                image_options: None,
            }],
        )
        .unwrap();

        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].model_id, "model-a");
        assert!(!configs[0].multimodal);
        assert_eq!(configs[0].context_size, 128_000);
        assert_eq!(configs[1].model_id, "model-b");
        assert!(configs[1].multimodal);
        assert_eq!(configs[1].context_size, 200_000);
        assert_eq!(configs[1].output_price, Some(1.5));
        assert_eq!(configs[1].cache_write_price, Some(0.08));
        assert_eq!(configs[1].cache_read_price, Some(0.05));
    }

    #[test]
    fn model_configs_reject_invalid_prices() {
        let result = normalize_model_configs(
            &["model-a".to_string()],
            vec![ModelConfig {
                model_id: "model-a".to_string(),
                multimodal: false,
                thinking: false,
                context_size: 128_000,
                input_price: Some(-0.01),
                output_price: None,
                cache_write_price: None,
                cache_read_price: None,
                image_options: None,
            }],
        );

        assert!(result.is_err());
    }

    #[test]
    fn model_configs_reject_zero_context_size() {
        let result = normalize_model_configs(
            &["model-a".to_string()],
            vec![ModelConfig {
                model_id: "model-a".to_string(),
                multimodal: false,
                thinking: false,
                context_size: 0,
                input_price: None,
                output_price: None,
                cache_write_price: None,
                cache_read_price: None,
                image_options: None,
            }],
        );

        assert!(result.is_err());
    }
}
