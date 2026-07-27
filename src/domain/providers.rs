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

/// How a provider with several API keys picks the key for each request.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApiKeyStrategy {
    /// Stick to the current key; move to the next one only after it errors.
    #[default]
    #[serde(rename = "failover")]
    Failover,
    /// Every request advances to the next key in list order.
    #[serde(rename = "round_robin")]
    RoundRobin,
    /// Every request picks a random key.
    #[serde(rename = "random")]
    Random,
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
    /// Ordered API key pool; requests pick a key according to `key_strategy`.
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub key_strategy: ApiKeyStrategy,
    pub models: Vec<String>,
    #[serde(default)]
    pub model_configs: Vec<ModelConfig>,
    #[serde(default)]
    pub extra_body: serde_json::Value,
    #[serde(default)]
    pub prompt_cache: PromptCacheConfig,
    #[serde(default)]
    pub cache_capabilities: Option<ProviderCacheCapabilities>,
    /// Upstream proxy for this provider's LLM traffic. Supports HTTP and SOCKS proxy URLs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::domain::ip_proxies::serialize_optional_proxy_url"
    )]
    pub proxy_url: Option<String>,
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
    /// One masked entry per stored key, in pool order.
    pub masked_api_keys: Vec<String>,
    pub key_strategy: ApiKeyStrategy,
    pub models: Vec<String>,
    pub model_configs: Vec<ModelConfig>,
    pub extra_body: serde_json::Value,
    pub prompt_cache: PromptCacheConfig,
    pub cache_capabilities: Option<ProviderCacheCapabilities>,
    #[serde(serialize_with = "crate::domain::ip_proxies::serialize_optional_proxy_url")]
    pub proxy_url: Option<String>,
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
            has_api_key: provider.api_keys.iter().any(|key| !key.trim().is_empty()),
            masked_api_keys: provider
                .api_keys
                .iter()
                .map(|key| mask_api_key(key))
                .filter(|masked| !masked.is_empty())
                .collect(),
            key_strategy: provider.key_strategy,
            models: provider.models.clone(),
            model_configs: provider.model_configs.clone(),
            extra_body: provider.extra_body.clone(),
            prompt_cache: provider.prompt_cache.clone(),
            cache_capabilities: provider.cache_capabilities.clone(),
            proxy_url: provider.proxy_url.clone(),
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

fn mask_api_key(api_key: &str) -> String {
    super::tools::mask_api_key(api_key)
}

/// Merge a submitted key list into the stored pool. Masked entries reference
/// existing keys — resolved positionally when the mask at that slot lines up,
/// otherwise by unique mask content, so a retried update resolves identically
/// even after the pool shifted. Plaintext entries are new keys. Order follows
/// the submitted list; blanks, dangling masks, and duplicates drop out.
pub fn merge_api_keys(existing: &[String], submitted: &[String]) -> Vec<String> {
    let mut used = vec![false; existing.len()];
    let mut merged: Vec<String> = Vec::new();
    for (slot, entry) in submitted.iter().enumerate() {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let resolved = if super::tools::is_masked_api_key(entry) {
            let positional = existing
                .get(slot)
                .filter(|_| !used[slot])
                .filter(|key| super::tools::mask_api_key(key) == entry)
                .map(|key| (slot, key.clone()));
            let by_content = || {
                let mut matches = existing.iter().enumerate().filter(|(index, key)| {
                    !used[*index] && super::tools::mask_api_key(key) == entry
                });
                let first = matches.next().map(|(index, key)| (index, key.clone()));
                first.filter(|_| matches.next().is_none())
            };
            match positional.or_else(by_content) {
                Some((index, key)) => {
                    used[index] = true;
                    key
                }
                // The referenced key no longer exists (e.g. an update retried
                // after it already applied): drop the stale reference.
                None => continue,
            }
        } else {
            entry.to_string()
        };
        if !merged.contains(&resolved) {
            merged.push(resolved);
        }
    }
    merged
}

#[derive(Debug, Deserialize)]
pub struct CreateModelProviderRequest {
    #[serde(default)]
    pub provider_type: ProviderType,
    pub name: String,
    pub base_url: String,
    /// Plaintext keys; masked entries are only meaningful on update.
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub key_strategy: ApiKeyStrategy,
    pub models: Vec<String>,
    #[serde(default)]
    pub model_configs: Vec<ModelConfig>,
    #[serde(default)]
    pub extra_body: serde_json::Value,
    #[serde(default)]
    pub prompt_cache: PromptCacheConfig,
    #[serde(default)]
    pub cache_capabilities: Option<ProviderCacheCapabilities>,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FetchProviderModelsRequest {
    #[serde(default)]
    pub provider_type: ProviderType,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub extra_body: serde_json::Value,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModelProviderRequest {
    pub provider_type: Option<ProviderType>,
    pub name: Option<String>,
    pub base_url: Option<String>,
    /// Missing = keys unchanged. Entries: masked = keep that stored key,
    /// plaintext = new key; the merged pool must not end up empty.
    pub api_keys: Option<Vec<String>>,
    pub key_strategy: Option<ApiKeyStrategy>,
    pub models: Option<Vec<String>>,
    pub model_configs: Option<Vec<ModelConfig>>,
    pub extra_body: Option<serde_json::Value>,
    pub prompt_cache: Option<PromptCacheConfig>,
    pub cache_capabilities: Option<Option<ProviderCacheCapabilities>>,
    /// Missing = unchanged, null = clear, string = set.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub proxy_url: Option<Option<String>>,
}

fn deserialize_double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::{ModelConfig, merge_api_keys};

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn merge_api_keys_keeps_masked_entries_and_appends_new_keys() {
        let existing = keys(&["sk-alpha-1111", "sk-beta-2222"]);
        let merged = merge_api_keys(&existing, &keys(&["****1111", "****2222", " sk-new-3333 "]));
        assert_eq!(
            merged,
            keys(&["sk-alpha-1111", "sk-beta-2222", "sk-new-3333"])
        );
    }

    #[test]
    fn merge_api_keys_supports_deleting_and_reordering() {
        let existing = keys(&["sk-alpha-1111", "sk-beta-2222", "sk-gamma-3333"]);
        // The first key was removed client-side, so the remaining masks shift slots.
        let merged = merge_api_keys(&existing, &keys(&["****2222", "****3333"]));
        assert_eq!(merged, keys(&["sk-beta-2222", "sk-gamma-3333"]));
    }

    #[test]
    fn merge_api_keys_is_idempotent_when_an_update_is_retried() {
        // First apply: delete the middle key, add one.
        let original = keys(&["sk-alpha-1111", "sk-beta-2222", "sk-gamma-3333"]);
        let submitted = keys(&["****1111", "****3333", "sk-new-4444"]);
        let applied = merge_api_keys(&original, &submitted);
        // Retry of the same payload against the already-updated pool.
        let retried = merge_api_keys(&applied, &submitted);
        assert_eq!(
            applied,
            keys(&["sk-alpha-1111", "sk-gamma-3333", "sk-new-4444"])
        );
        assert_eq!(retried, applied);
    }

    #[test]
    fn merge_api_keys_prefers_positional_match_on_mask_collisions() {
        let existing = keys(&["sk-alpha-9999", "sk-beta-9999"]);
        let merged = merge_api_keys(&existing, &keys(&["****9999", "****9999"]));
        assert_eq!(merged, existing);
    }

    #[test]
    fn merge_api_keys_drops_blanks_duplicates_and_dangling_masks() {
        let existing = keys(&["sk-alpha-1111"]);
        let merged = merge_api_keys(
            &existing,
            &keys(&["  ", "****1111", "sk-alpha-1111", "****gone", "sk-new-2222"]),
        );
        assert_eq!(merged, keys(&["sk-alpha-1111", "sk-new-2222"]));
    }

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
