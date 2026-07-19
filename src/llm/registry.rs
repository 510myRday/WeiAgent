use std::sync::Arc;

use crate::{
    domain::{ModelProvider, ProviderType},
    error::AppResult,
    llm::{
        client::DynChatClient,
        protocol::{
            anthropic::AnthropicMessagesClient, gemini::GeminiClient,
            openai_chat::OpenAiChatClient, openai_responses::OpenAiResponsesClient,
        },
    },
    storage::LlmLogger,
};

pub trait LlmRegistry: Send + Sync {
    fn client_for(&self, provider: &ModelProvider) -> AppResult<DynChatClient>;
}

pub type DynLlmRegistry = Arc<dyn LlmRegistry>;

pub struct DefaultLlmRegistry {
    openai_compatible: DynChatClient,
    openai_responses: DynChatClient,
    anthropic_messages: DynChatClient,
    google_gemini: DynChatClient,
}

impl DefaultLlmRegistry {
    pub fn new(llm_logger: Arc<LlmLogger>) -> Self {
        Self {
            openai_compatible: Arc::new(OpenAiChatClient::new().with_logger(llm_logger.clone())),
            openai_responses: Arc::new(
                OpenAiResponsesClient::new().with_logger(llm_logger.clone()),
            ),
            anthropic_messages: Arc::new(
                AnthropicMessagesClient::new().with_logger(llm_logger.clone()),
            ),
            google_gemini: Arc::new(GeminiClient::new().with_logger(llm_logger)),
        }
    }
}

impl LlmRegistry for DefaultLlmRegistry {
    fn client_for(&self, provider: &ModelProvider) -> AppResult<DynChatClient> {
        match provider.provider_type {
            ProviderType::OpenAiCompatible => Ok(self.openai_compatible.clone()),
            ProviderType::OpenAiResponses => Ok(self.openai_responses.clone()),
            ProviderType::AnthropicMessages => Ok(self.anthropic_messages.clone()),
            ProviderType::GoogleGemini => Ok(self.google_gemini.clone()),
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
    fn resolves_every_provider_client() {
        let temp = tempfile::tempdir().unwrap();
        let logger = Arc::new(LlmLogger::new(temp.path(), false).unwrap());
        let registry = DefaultLlmRegistry::new(logger);
        for provider_type in [
            ProviderType::OpenAiCompatible,
            ProviderType::OpenAiResponses,
            ProviderType::AnthropicMessages,
            ProviderType::GoogleGemini,
        ] {
            let provider = ModelProvider {
                id: Uuid::new_v4(),
                provider_type,
                name: "Provider".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "test-key".to_string(),
                models: vec!["test-model".to_string()],
                model_configs: Vec::new(),
                extra_body: serde_json::Value::Null,
                prompt_cache: Default::default(),
                cache_capabilities: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            assert!(registry.client_for(&provider).is_ok());
        }
    }
}
