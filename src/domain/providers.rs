use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    pub model_id: String,
    #[serde(default)]
    pub multimodal: bool,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub input_price: Option<f64>,
    #[serde(default)]
    pub output_price: Option<f64>,
    #[serde(default)]
    pub cache_write_price: Option<f64>,
    #[serde(default, alias = "cache_price")]
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
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

fn mask_api_key(api_key: &str) -> String {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let suffix = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("****{suffix}")
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
}

#[cfg(test)]
mod tests {
    use super::ModelConfig;

    #[test]
    fn legacy_model_config_migrates_cache_price_and_drops_removed_fields() {
        let config: ModelConfig = serde_json::from_value(serde_json::json!({
            "model_id": "legacy-model",
            "multimodal": true,
            "thinking": true,
            "tool_calling": true,
            "web_search": true,
            "input_price": 1.0,
            "output_price": 2.0,
            "cache_price": 0.25
        }))
        .unwrap();

        assert_eq!(config.cache_write_price, None);
        assert_eq!(config.cache_read_price, Some(0.25));

        let serialized = serde_json::to_value(config).unwrap();
        assert!(serialized.get("tool_calling").is_none());
        assert!(serialized.get("web_search").is_none());
        assert!(serialized.get("cache_price").is_none());
        assert_eq!(serialized["cache_read_price"], 0.25);
    }
}
