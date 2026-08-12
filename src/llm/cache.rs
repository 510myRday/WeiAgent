use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::{ModelProvider, ProviderType};
use crate::llm::types::ToolDefinition;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    Disabled,
    #[default]
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptCacheConfig {
    #[serde(default)]
    pub mode: PromptCacheMode,
    #[serde(default)]
    pub use_stable_key: bool,
    #[serde(default)]
    pub retention: Option<String>,
    #[serde(default)]
    pub anthropic_ttl: Option<String>,
    #[serde(default)]
    pub gemini_cached_content: Option<String>,
}

impl Default for PromptCacheConfig {
    fn default() -> Self {
        Self {
            mode: PromptCacheMode::Automatic,
            use_stable_key: false,
            retention: None,
            anthropic_ttl: None,
            gemini_cached_content: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCacheCapabilities {
    #[serde(default = "default_true")]
    pub automatic_prefix_cache: bool,
    #[serde(default)]
    pub explicit_breakpoints: bool,
    #[serde(default)]
    pub stable_cache_key: bool,
    #[serde(default)]
    pub supports_cache_retention: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProviderCacheCapabilities {
    fn default() -> Self {
        Self {
            automatic_prefix_cache: true,
            explicit_breakpoints: false,
            stable_cache_key: false,
            supports_cache_retention: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheContext {
    pub cache_key: Option<String>,
    pub explicit_breakpoints: bool,
}

pub fn effective_capabilities(provider: &ModelProvider) -> ProviderCacheCapabilities {
    if let Some(capabilities) = &provider.cache_capabilities {
        return capabilities.clone();
    }
    match provider.provider_type {
        ProviderType::OpenAiCompatible => ProviderCacheCapabilities::default(),
        ProviderType::OpenAiResponses => ProviderCacheCapabilities {
            automatic_prefix_cache: true,
            // Explicit breakpoint fields are opt-in because many Responses
            // compatible gateways reject fields they do not implement.
            explicit_breakpoints: false,
            stable_cache_key: true,
            // Retention is opt-in for compatibility with Responses gateways
            // that support prompt_cache_key but reject retention fields.
            supports_cache_retention: false,
        },
        ProviderType::AnthropicMessages => ProviderCacheCapabilities {
            automatic_prefix_cache: true,
            explicit_breakpoints: true,
            stable_cache_key: false,
            supports_cache_retention: false,
        },
        ProviderType::GoogleGemini => ProviderCacheCapabilities {
            automatic_prefix_cache: true,
            explicit_breakpoints: true,
            stable_cache_key: false,
            supports_cache_retention: false,
        },
        ProviderType::OpenAiImages => ProviderCacheCapabilities::default(),
    }
}

/// GPT-5.6+ (and later GPT major versions) support explicit
/// `prompt_cache_breakpoint` / `prompt_cache_options` on Responses.
pub fn model_supports_prompt_cache_breakpoints(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    let Some(pos) = lower.find("gpt-") else {
        return false;
    };
    let rest = &lower[pos + 4..];
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if version.is_empty() {
        return false;
    }
    let mut parts = version.split('.');
    let major = parts.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    major > 5 || (major == 5 && minor >= 6)
}

pub fn tools_hash(tools: Option<&[ToolDefinition]>) -> String {
    let value = tools.map_or_else(|| json!([]), |tools| json!(tools));
    hash_json(&value)
}

pub fn system_prompt_hash(system_prompt: &str) -> String {
    hash_bytes(system_prompt.as_bytes())
}

pub fn build_context(
    provider: &ModelProvider,
    model_id: &str,
    agent_id: Uuid,
    protocol: &str,
    system_prompt: &str,
    tools: Option<&[ToolDefinition]>,
) -> CacheContext {
    let capabilities = effective_capabilities(provider);
    let config = &provider.prompt_cache;
    let cache_key = (config.use_stable_key
        && capabilities.stable_cache_key
        && !matches!(config.mode, PromptCacheMode::Disabled))
    .then(|| {
        let tools_hash = tools_hash(tools);
        let static_prefix_hash = system_prompt_hash(system_prompt);
        format!(
            "weiagent:{}:{}:{}:{}:{}:{}",
            provider.id,
            model_id,
            agent_id,
            protocol,
            &tools_hash[..16],
            &static_prefix_hash[..16]
        )
    });
    let model_breakpoints = model_supports_prompt_cache_breakpoints(model_id);
    CacheContext {
        cache_key,
        explicit_breakpoints: capabilities.explicit_breakpoints
            && model_breakpoints
            && !matches!(config.mode, PromptCacheMode::Disabled)
            && (matches!(config.mode, PromptCacheMode::Explicit)
                || matches!(config.mode, PromptCacheMode::Automatic)),
    }
}

/// Mark each user message's last cacheable content block so GPT-5.6+ can reuse
/// the stable instructions/tools/history prefix across agent tool loops.
pub fn apply_responses_prompt_cache_breakpoints(input: &mut [Value]) {
    for item in input.iter_mut() {
        if item.get("type").and_then(|value| value.as_str()) != Some("message") {
            continue;
        }
        if item.get("role").and_then(|value| value.as_str()) != Some("user") {
            continue;
        }
        let Some(content) = item
            .get_mut("content")
            .and_then(|value| value.as_array_mut())
        else {
            continue;
        };
        let last_idx = content.iter().enumerate().rev().find_map(|(index, block)| {
            match block.get("type").and_then(|value| value.as_str()) {
                Some("input_text") | Some("input_image") | Some("input_file") => Some(index),
                _ => None,
            }
        });
        let Some(index) = last_idx else {
            continue;
        };
        if let Some(block) = content.get_mut(index).and_then(|value| value.as_object_mut()) {
            block.insert(
                "prompt_cache_breakpoint".to_string(),
                json!({ "mode": "explicit" }),
            );
        }
    }
}

pub fn apply_openai_fields(
    body: &mut Map<String, Value>,
    provider: &ModelProvider,
    context: &CacheContext,
) {
    let config = &provider.prompt_cache;
    let capabilities = effective_capabilities(provider);
    if matches!(config.mode, PromptCacheMode::Disabled)
        || !capabilities.automatic_prefix_cache
        || (matches!(config.mode, PromptCacheMode::Explicit) && !capabilities.explicit_breakpoints)
    {
        return;
    }
    if let Some(key) = context.cache_key.as_ref() {
        body.insert("prompt_cache_key".to_string(), json!(key));
    }
    if capabilities.supports_cache_retention
        && let Some(retention) = config.retention.as_ref()
        && !retention.trim().is_empty()
    {
        body.insert("prompt_cache_retention".to_string(), json!(retention));
    }
    if context.explicit_breakpoints {
        body.insert(
            "prompt_cache_options".to_string(),
            json!({ "mode": "explicit" }),
        );
    }
}

fn hash_json(value: &Value) -> String {
    serde_json::to_vec(value).map_or_else(
        |_| hash_bytes(value.to_string().as_bytes()),
        |bytes| hash_bytes(&bytes),
    )
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn provider() -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type: ProviderType::OpenAiResponses,
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_keys: vec!["TOKEN".into()],
            key_strategy: Default::default(),
            models: vec!["model".into()],
            model_configs: vec![],
            extra_body: Value::Null,
            prompt_cache: PromptCacheConfig {
                use_stable_key: true,
                ..Default::default()
            },
            cache_capabilities: None,
            proxy_url: None,
            image_generation_url: None,
            image_edit_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn system_hash_and_key_change_with_static_prompt() {
        let p = provider();
        let a = build_context(&p, "model", Uuid::nil(), "responses", "one", None);
        let b = build_context(&p, "model", Uuid::nil(), "responses", "two", None);
        assert_ne!(system_prompt_hash("one"), system_prompt_hash("two"));
        assert_ne!(a.cache_key, b.cache_key);
    }

    #[test]
    fn openai_fields_follow_declared_capabilities() {
        let mut p = provider();
        p.prompt_cache.retention = Some("24h".into());
        let context = build_context(&p, "gpt-5.6", Uuid::nil(), "responses", "system", None);
        let mut body = Map::new();
        apply_openai_fields(&mut body, &p, &context);
        assert!(body.contains_key("prompt_cache_key"));
        assert!(!body.contains_key("prompt_cache_retention"));
        assert!(!body.contains_key("prompt_cache_options"));
        assert!(!context.explicit_breakpoints);

        p.cache_capabilities = Some(ProviderCacheCapabilities {
            automatic_prefix_cache: true,
            explicit_breakpoints: true,
            stable_cache_key: true,
            supports_cache_retention: true,
        });
        let context = build_context(&p, "gpt-5.6", Uuid::nil(), "responses", "system", None);
        let mut body = Map::new();
        apply_openai_fields(&mut body, &p, &context);
        assert_eq!(body["prompt_cache_options"]["mode"], "explicit");
        assert!(context.explicit_breakpoints);

        p.cache_capabilities = Some(ProviderCacheCapabilities::default());
        let context = build_context(&p, "model", Uuid::nil(), "openai_chat", "system", None);
        let mut body = Map::new();
        apply_openai_fields(&mut body, &p, &context);
        assert!(!body.contains_key("prompt_cache_key"));
        assert!(!body.contains_key("prompt_cache_retention"));
        assert!(!body.contains_key("prompt_cache_options"));
    }

    #[test]
    fn gpt56_family_detects_breakpoint_support() {
        assert!(model_supports_prompt_cache_breakpoints("gpt-5.6"));
        assert!(model_supports_prompt_cache_breakpoints("openai/gpt-5.6-pro"));
        assert!(model_supports_prompt_cache_breakpoints("GPT-5.7-mini"));
        assert!(model_supports_prompt_cache_breakpoints("gpt-6"));
        assert!(!model_supports_prompt_cache_breakpoints("gpt-5.5"));
        assert!(!model_supports_prompt_cache_breakpoints("gpt-4o"));
        assert!(!model_supports_prompt_cache_breakpoints("test-model"));
    }

    #[test]
    fn marks_user_message_content_blocks_with_breakpoints() {
        let mut input = vec![
            json!({
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "hello"},
                    {"type": "input_image", "image_url": "https://example.test/a.png"}
                ]
            }),
            json!({
                "type": "function_call_output",
                "call_id": "call-1",
                "output": "ok"
            }),
        ];
        apply_responses_prompt_cache_breakpoints(&mut input);
        assert_eq!(
            input[0]["content"][1]["prompt_cache_breakpoint"]["mode"],
            "explicit"
        );
        assert!(input[0]["content"][0].get("prompt_cache_breakpoint").is_none());
        assert!(input[1].get("prompt_cache_breakpoint").is_none());
    }

    #[test]
    fn default_retention_is_omitted_and_nullable() {
        assert_eq!(PromptCacheConfig::default().retention, None);
        let parsed: PromptCacheConfig = serde_json::from_str(r#"{"retention":null}"#).unwrap();
        assert_eq!(parsed.retention, None);
        let omitted: PromptCacheConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(omitted.retention, None);
    }
}
