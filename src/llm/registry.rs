use std::sync::Arc;

use crate::{
    domain::{ModelProvider, ProviderType},
    error::AppResult,
    llm::{client::DynChatClient, providers::openai_compatible::OpenAiCompatibleChatClient},
};

pub trait LlmRegistry: Send + Sync {
    fn client_for(&self, provider: &ModelProvider) -> AppResult<DynChatClient>;
}

pub type DynLlmRegistry = Arc<dyn LlmRegistry>;

pub struct DefaultLlmRegistry {
    openai_compatible: DynChatClient,
}

impl Default for DefaultLlmRegistry {
    fn default() -> Self {
        Self {
            openai_compatible: Arc::new(OpenAiCompatibleChatClient::new()),
        }
    }
}

impl LlmRegistry for DefaultLlmRegistry {
    fn client_for(&self, provider: &ModelProvider) -> AppResult<DynChatClient> {
        match provider.provider_type {
            ProviderType::OpenAiCompatible => Ok(self.openai_compatible.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::domain::ModelProvider;

    #[test]
    fn resolves_openai_compatible_client() {
        let registry = DefaultLlmRegistry::default();
        let provider = ModelProvider {
            id: Uuid::new_v4(),
            provider_type: ProviderType::OpenAiCompatible,
            name: "OpenAI Compatible".to_string(),
            base_url: "https://example.test/v1".to_string(),
            api_key: "test-key".to_string(),
            models: vec!["test-model".to_string()],
            extra_body: serde_json::Value::Null,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert!(registry.client_for(&provider).is_ok());
    }
}
