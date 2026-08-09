use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    domain::{
        CreateModelProviderRequest, ImageModelConfig, ModelConfig, ModelProvider,
        ModelProviderResponse, ProviderType, UpdateModelProviderRequest, merge_api_keys,
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
        let model_configs =
            normalize_model_configs_for_provider(&models, request.model_configs, &provider_type)?;
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
        provider.model_configs = normalize_model_configs_for_provider(
            &provider.models,
            requested_model_configs.unwrap_or_else(|| provider.model_configs.clone()),
            &provider.provider_type,
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
        let mut agents = self.read_agents_unlocked().await?;
        let original_agents = agents.clone();
        let mut agents_changed = false;
        for agent in agents
            .iter_mut()
            .filter(|agent| agent.model_provider_id == id)
        {
            agent.model_provider_id = Uuid::nil();
            agent.model_id.clear();
            agent.updated_at = Utc::now();
            agents_changed = true;
        }

        if agents_changed {
            self.write_agents_unlocked(&agents).await?;
        }

        let deleted = self
            .database
            .call(move |connection| db::delete_provider_conn(connection, id))
            .await;
        let deleted = match deleted {
            Ok(deleted) => deleted,
            Err(error) => {
                if agents_changed {
                    let _ = self.write_agents_unlocked(&original_agents).await;
                }
                return Err(error);
            }
        };
        if !deleted {
            if agents_changed {
                let _ = self.write_agents_unlocked(&original_agents).await;
            }
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

#[cfg(test)]
fn normalize_model_configs(
    models: &[String],
    configs: Vec<ModelConfig>,
) -> AppResult<Vec<ModelConfig>> {
    normalize_model_configs_for_provider(models, configs, &ProviderType::OpenAiCompatible)
}

fn normalize_model_configs_for_provider(
    models: &[String],
    configs: Vec<ModelConfig>,
    provider_type: &ProviderType,
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
            let mut config = configs_by_id
                .remove(model_id)
                .unwrap_or_else(|| default_model_config(model_id, provider_type));
            validate_context_size(config.context_size)?;
            validate_model_price("input_price", config.input_price)?;
            validate_model_price("output_price", config.output_price)?;
            validate_model_price("cache_write_price", config.cache_write_price)?;
            validate_model_price("cache_read_price", config.cache_read_price)?;
            normalize_image_options(&mut config.image_options)?;
            Ok(config)
        })
        .collect()
}

fn default_model_config(model_id: &str, provider_type: &ProviderType) -> ModelConfig {
    ModelConfig {
        model_id: model_id.to_string(),
        multimodal: false,
        thinking: false,
        context_size: 128_000,
        input_price: None,
        output_price: None,
        cache_write_price: None,
        cache_read_price: None,
        image_options: (*provider_type == ProviderType::OpenAiImages).then(|| ImageModelConfig {
            size: Some("auto".to_string()),
            quality: Some("auto".to_string()),
            n: Some(1),
            output_format: None,
            output_compression: None,
            background: Some("auto".to_string()),
            moderation: Some("auto".to_string()),
            style: None,
            response_format: None,
            input_fidelity: None,
        }),
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
    let parsed = reqwest::Url::parse(&value)
        .map_err(|_| AppError::BadRequest("image endpoint URL must be valid".to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(AppError::BadRequest(
            "image endpoint URL must use http or https".to_string(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::BadRequest(
            "image endpoint URL cannot contain credentials".to_string(),
        ));
    }
    Ok(Some(value))
}

fn normalize_image_options(options: &mut Option<ImageModelConfig>) -> AppResult<()> {
    let Some(config) = options.as_mut() else {
        return Ok(());
    };
    config.size = normalized_optional_string(config.size.take());
    config.quality = normalized_enum(
        "image quality",
        config.quality.take(),
        &["auto", "low", "medium", "high", "standard", "hd"],
    )?;
    config.output_format = normalized_enum(
        "image output_format",
        config.output_format.take(),
        &["png", "jpeg", "webp"],
    )?;
    config.background = normalized_enum(
        "image background",
        config.background.take(),
        &["auto", "opaque", "transparent"],
    )?;
    config.moderation = normalized_enum(
        "image moderation",
        config.moderation.take(),
        &["auto", "low"],
    )?;
    config.style = normalized_enum("image style", config.style.take(), &["vivid", "natural"])?;
    config.response_format = normalized_enum(
        "image response_format",
        config.response_format.take(),
        &["b64_json", "url"],
    )?;
    config.input_fidelity = normalized_enum(
        "image input_fidelity",
        config.input_fidelity.take(),
        &["low", "high"],
    )?;
    if config.n.is_some_and(|value| !(1..=10).contains(&value)) {
        return Err(AppError::BadRequest(
            "image n must be between 1 and 10".to_string(),
        ));
    }
    if config.output_compression.is_some_and(|value| value > 100) {
        return Err(AppError::BadRequest(
            "image output_compression must be between 0 and 100".to_string(),
        ));
    }
    if config == &ImageModelConfig::default() {
        *options = None;
    }
    Ok(())
}

fn normalized_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn normalized_enum(
    field: &str,
    value: Option<String>,
    allowed: &[&str],
) -> AppResult<Option<String>> {
    let value = normalized_optional_string(value);
    if value
        .as_deref()
        .is_some_and(|value| !allowed.contains(&value))
    {
        return Err(AppError::BadRequest(format!(
            "{field} has an unsupported value"
        )));
    }
    Ok(value)
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
    use super::{normalize_model_configs, normalize_optional_url, validate_extra_body};
    use crate::domain::{ImageModelConfig, ModelConfig};

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

    #[test]
    fn image_options_reject_invalid_ranges_and_values() {
        let result = normalize_model_configs(
            &["image-model".to_string()],
            vec![ModelConfig {
                model_id: "image-model".to_string(),
                multimodal: false,
                thinking: false,
                context_size: 128_000,
                input_price: None,
                output_price: None,
                cache_write_price: None,
                cache_read_price: None,
                image_options: Some(ImageModelConfig {
                    n: Some(11),
                    ..Default::default()
                }),
            }],
        );
        assert!(result.is_err());

        let result = normalize_model_configs(
            &["image-model".to_string()],
            vec![ModelConfig {
                model_id: "image-model".to_string(),
                multimodal: false,
                thinking: false,
                context_size: 128_000,
                input_price: None,
                output_price: None,
                cache_write_price: None,
                cache_read_price: None,
                image_options: Some(ImageModelConfig {
                    style: Some("unsupported".to_string()),
                    ..Default::default()
                }),
            }],
        );
        assert!(result.is_err());
    }

    #[test]
    fn image_endpoint_urls_require_http_or_https_without_credentials() {
        assert!(normalize_optional_url(Some("file:///tmp/image".to_string())).is_err());
        assert!(
            normalize_optional_url(Some("https://user:pass@example.test/images".to_string()))
                .is_err()
        );
        assert!(
            normalize_optional_url(Some("https://api.example.test/images".to_string())).is_ok()
        );
    }
}
