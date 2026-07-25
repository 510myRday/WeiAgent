use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::llm::cache::{PromptCacheConfig, ProviderCacheCapabilities};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderType {
    #[default]
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
    #[serde(rename = "google_gemini")]
    GoogleGemini,
}

fn default_context_size() -> u32 {
    128_000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    pub model_id: String,
    #[serde(default)]
    pub multimodal: bool,
    #[serde(default)]
    pub thinking: bool,
    /// Model context window size in tokens. Defaults to 128K.
    #[serde(default = "default_context_size")]
    pub context_size: u32,
    #[serde(default)]
    pub input_price: Option<f64>,
    #[serde(default)]
    pub output_price: Option<f64>,
    #[serde(default)]
    pub cache_write_price: Option<f64>,
    #[serde(default)]
    pub cache_read_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelProvider {
    pub id: Uuid,
    #[serde(default)]
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
    #[serde(default)]
    pub model_configs: Vec<ModelConfig>,
    #[serde(default)]
    pub extra_body: serde_json::Value,
    #[serde(default)]
    pub prompt_cache: PromptCacheConfig,
    #[serde(default)]
    pub cache_capabilities: Option<ProviderCacheCapabilities>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelProviderResponse {
    pub id: Uuid,
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    pub has_api_key: bool,
    pub masked_api_key: String,
    pub models: Vec<String>,
    pub model_configs: Vec<ModelConfig>,
    pub extra_body: serde_json::Value,
    pub prompt_cache: PromptCacheConfig,
    pub cache_capabilities: Option<ProviderCacheCapabilities>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ModelProvider> for ModelProviderResponse {
    fn from(provider: &ModelProvider) -> Self {
        Self {
            id: provider.id,
            provider_type: provider.provider_type.clone(),
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            has_api_key: !provider.api_key.trim().is_empty(),
            masked_api_key: mask_api_key(&provider.api_key),
            models: provider.models.clone(),
            model_configs: provider.model_configs.clone(),
            extra_body: provider.extra_body.clone(),
            prompt_cache: provider.prompt_cache.clone(),
            cache_capabilities: provider.cache_capabilities.clone(),
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

fn mask_api_key(api_key: &str) -> String {
    super::tools::mask_api_key(api_key)
}

#[derive(Debug, Deserialize)]
pub struct CreateModelProviderRequest {
    #[serde(default)]
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
    #[serde(default)]
    pub model_configs: Vec<ModelConfig>,
    #[serde(default)]
    pub extra_body: serde_json::Value,
    #[serde(default)]
    pub prompt_cache: PromptCacheConfig,
    #[serde(default)]
    pub cache_capabilities: Option<ProviderCacheCapabilities>,
}

#[derive(Debug, Deserialize)]
pub struct FetchProviderModelsRequest {
    #[serde(default)]
    pub provider_type: ProviderType,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub extra_body: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModelProviderRequest {
    pub provider_type: Option<ProviderType>,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
    pub model_configs: Option<Vec<ModelConfig>>,
    pub extra_body: Option<serde_json::Value>,
    pub prompt_cache: Option<PromptCacheConfig>,
    pub cache_capabilities: Option<Option<ProviderCacheCapabilities>>,
}

#[cfg(test)]
mod tests {
    use super::ModelConfig;

    #[test]
    fn model_config_defaults_context_size_and_prices() {
        let config: ModelConfig = serde_json::from_value(serde_json::json!({
            "model_id": "basic-model",
            "multimodal": true,
            "thinking": true,
            "input_price": 1.0,
            "output_price": 2.0,
            "cache_read_price": 0.25
        }))
        .unwrap();

        assert_eq!(config.cache_write_price, None);
        assert_eq!(config.cache_read_price, Some(0.25));
        assert_eq!(config.context_size, 128_000);

        let serialized = serde_json::to_value(config).unwrap();
        assert_eq!(serialized["cache_read_price"], 0.25);
        assert_eq!(serialized["context_size"], 128_000);
    }

    #[test]
    fn model_config_keeps_explicit_context_size() {
        let config: ModelConfig = serde_json::from_value(serde_json::json!({
            "model_id": "wide-model",
            "context_size": 200000
        }))
        .unwrap();

        assert_eq!(config.context_size, 200_000);
    }
}
