mod agents;
mod conversations;
mod files;
mod json_store;
mod providers;
mod validation;
mod workspaces;

pub use json_store::JsonStore;

pub(super) const BUILTIN_CEO_KEY: &str = "wei-ceo";
pub(super) const DEFAULT_AGENT_ICON: &str = "bot";
pub(super) const DEFAULT_AGENT_COLOR: &str = "#22c55e";
pub(super) const BUILTIN_CEO_ICON: &str = "crown";
pub(super) const BUILTIN_CEO_COLOR: &str = "#2563eb";
pub(super) const WORKSPACE_PERSONALITY: &str = "personality.md";
pub(super) const WORKSPACE_MEMORY: &str = "memory.md";
pub(super) const WORKSPACE_INSTRUCTIONS: &str = "instructions.md";
pub(super) const WORKSPACE_TOOLS: &str = "tools.json";
pub(super) const WORKSPACE_SETTINGS: &str = "settings.json";

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{
        ChatMessage, ChatRole, CreateAgentRequest, CreateConversationRequest,
        CreateModelProviderRequest, ProviderType, UpdateAgentWorkspaceRequest,
        UpdateModelProviderRequest,
    };

    fn provider_request(name: &str, model: &str) -> CreateModelProviderRequest {
        CreateModelProviderRequest {
            provider_type: ProviderType::OpenAiCompatible,
            name: name.to_string(),
            base_url: "https://example.test/v1".to_string(),
            api_key: "sk-test".to_string(),
            models: vec![model.to_string()],
            extra_body: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn initializes_settings_and_providers_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let settings = store.read_settings().await.unwrap();
        assert_eq!(settings.host, "127.0.0.1");
        assert!(temp_dir.path().join("model_providers.json").exists());
    }

    #[tokio::test]
    async fn stores_provider_type_and_preserves_blank_key_update() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                name: "OpenAI".to_string(),
                api_key: "sk-test-secret".to_string(),
                models: vec!["gpt-4o-mini".to_string()],
                ..provider_request("OpenAI", "gpt-4o-mini")
            })
            .await
            .unwrap();

        assert_eq!(provider.provider_type, ProviderType::OpenAiCompatible);
        assert!(provider.has_api_key);
        assert_eq!(provider.masked_api_key, "****cret");

        let updated = store
            .update_model_provider(
                provider.id,
                UpdateModelProviderRequest {
                    provider_type: Some(ProviderType::OpenAiCompatible),
                    name: Some("OpenAI Compatible".to_string()),
                    base_url: None,
                    api_key: Some(String::new()),
                    models: None,
                    extra_body: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "OpenAI Compatible");
        assert_eq!(updated.provider_type, ProviderType::OpenAiCompatible);
        assert!(updated.has_api_key);
    }

    #[tokio::test]
    async fn validates_agent_provider_and_stores_conversations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(provider_request("OpenAI", "gpt-4o-mini"))
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Planner".to_string(),
                model_provider_id: provider.id,
                model_id: "gpt-4o-mini".to_string(),
                avatar_icon: Some("sparkles".to_string()),
                avatar_color: Some("#3b82f6".to_string()),
            })
            .await
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let message = ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: conversation.id,
            role: ChatRole::User,
            content: "hello".to_string(),
            created_at: Utc::now(),
        };
        store
            .append_conversation_messages(conversation.id, std::slice::from_ref(&message))
            .await
            .unwrap();

        assert_eq!(
            store
                .read_conversation_messages(conversation.id)
                .await
                .unwrap(),
            vec![message]
        );
    }

    #[tokio::test]
    async fn initializes_builtin_agent_workspace_and_blocks_delete() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let agents = store.list_agents().await.unwrap();
        let builtin = agents
            .iter()
            .find(|agent| agent.builtin_key.as_deref() == Some(BUILTIN_CEO_KEY))
            .unwrap();

        assert_eq!(builtin.name, "Wei 首席专家");
        assert!(builtin.is_builtin);
        assert!(!builtin.can_delete);
        assert!(
            temp_dir
                .path()
                .join("agents")
                .join(builtin.id.to_string())
                .exists()
        );
        assert!(store.delete_agent(builtin.id).await.is_err());
    }

    #[tokio::test]
    async fn creates_workspace_and_composes_markdown_prompt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(provider_request("Local", "local-model"))
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Operator".to_string(),
                model_provider_id: provider.id,
                model_id: "local-model".to_string(),
                avatar_icon: Some("brain".to_string()),
                avatar_color: Some("#8b5cf6".to_string()),
            })
            .await
            .unwrap();

        store
            .update_agent_workspace(
                agent.id,
                UpdateAgentWorkspaceRequest {
                    personality: "Calm and direct.".to_string(),
                    memory: "User prefers concise plans.".to_string(),
                    instructions: "Ask only important questions.".to_string(),
                },
            )
            .await
            .unwrap();

        let stored = store.get_agent(agent.id).await.unwrap();
        let prompt = store.build_agent_system_prompt(&stored).await.unwrap();
        assert!(prompt.contains("Calm and direct."));
        assert!(prompt.contains("User prefers concise plans."));
        assert!(prompt.contains("Ask only important questions."));
    }

    #[tokio::test]
    async fn rejects_duplicate_agent_avatar_combination() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(provider_request("Avatar Test", "avatar-model"))
            .await
            .unwrap();

        store
            .create_agent(CreateAgentRequest {
                name: "One".to_string(),
                model_provider_id: provider.id,
                model_id: "avatar-model".to_string(),
                avatar_icon: Some("compass".to_string()),
                avatar_color: Some("#06b6d4".to_string()),
            })
            .await
            .unwrap();
        let duplicate = store
            .create_agent(CreateAgentRequest {
                name: "Two".to_string(),
                model_provider_id: provider.id,
                model_id: "avatar-model".to_string(),
                avatar_icon: Some("compass".to_string()),
                avatar_color: Some("#06b6d4".to_string()),
            })
            .await;

        assert!(duplicate.is_err());
    }
}
