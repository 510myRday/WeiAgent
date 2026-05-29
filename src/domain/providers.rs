use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderType {
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProvider {
    pub id: Uuid,
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
    #[serde(default)]
    pub extra_body: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelProviderResponse {
    pub id: Uuid,
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    pub has_api_key: bool,
    pub masked_api_key: String,
    pub models: Vec<String>,
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
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
    #[serde(default)]
    pub extra_body: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct FetchProviderModelsRequest {
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
    pub extra_body: Option<serde_json::Value>,
}
