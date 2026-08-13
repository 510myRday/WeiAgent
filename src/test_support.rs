use crate::domain::{CreateModelProviderRequest, ProviderType};

pub fn provider_request(name: &str, model: &str) -> CreateModelProviderRequest {
    CreateModelProviderRequest {
        provider_type: ProviderType::OpenAiCompatible,
        name: name.to_string(),
        base_url: "https://example.test/v1".to_string(),
        api_keys: vec!["sk-test".to_string()],
        key_strategy: Default::default(),
        models: vec![model.to_string()],
        model_configs: Vec::new(),
        extra_body: serde_json::Value::Null,
        prompt_cache: Default::default(),
        cache_capabilities: None,
        proxy_url: None,
        image_generation_url: None,
        image_edit_url: None,
    }
}
