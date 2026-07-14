mod agents;
mod conversations;
mod files;
mod json_store;
pub mod llm_logs;
mod providers;
mod validation;
mod workspaces;

pub use json_store::JsonStore;
pub use llm_logs::LlmLogger;

pub(super) const DEFAULT_AGENT_ICON: &str = "bot";
pub(super) const DEFAULT_AGENT_COLOR: &str = "#22c55e";
pub(super) const BUILTIN_CEO_ICON: &str = "crown";
pub(super) const BUILTIN_CEO_COLOR: &str = "#2563eb";
pub(super) const WORKSPACE_PERSONALITY: &str = "personality.md";
pub(super) const WORKSPACE_MEMORY: &str = "memory.md";
pub(super) const WORKSPACE_INSTRUCTIONS: &str = "instructions.md";
pub(super) const WORKSPACE_TOOLS: &str = "tools.json";

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{
        ChatMessage, ChatMessageStatus, ChatRole, CreateAgentRequest, CreateConversationRequest,
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
        assert!(temp_dir.path().join("tool_defaults.json").exists());
    }

    #[tokio::test]
    async fn agent_tool_overrides_inherit_global_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let agent = store
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|agent| agent.is_builtin)
            .unwrap();
        let mut defaults = store.read_tool_defaults().await.unwrap();
        defaults.max_llm_iterations = 12;
        defaults.terminal.default_timeout_ms = 45_000;
        store.update_tool_defaults(defaults).await.unwrap();

        let inherited = store.read_agent_tools_settings(agent.id).await.unwrap();
        assert_eq!(inherited.effective.max_llm_iterations, 12);
        assert_eq!(inherited.effective.terminal.default_timeout_ms, 45_000);
        assert_eq!(inherited.overrides.max_llm_iterations, None);

        let mut overrides = inherited.overrides;
        overrides.terminal.enabled = Some(false);
        let updated = store
            .update_agent_tools_overrides(agent.id, overrides)
            .await
            .unwrap();
        assert!(!updated.effective.terminal.enabled);
        assert_eq!(updated.effective.max_llm_iterations, 12);
        assert_eq!(updated.effective.terminal.default_timeout_ms, 45_000);
    }

    #[tokio::test]
    async fn legacy_agent_tools_config_migrates_to_differences() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let agent = store
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|agent| agent.is_builtin)
            .unwrap();
        let defaults = store.read_tool_defaults().await.unwrap();
        let mut legacy = defaults.clone();
        let legacy_enabled = !defaults.terminal.enabled;
        legacy.terminal.enabled = legacy_enabled;
        let legacy_path = store.agent_workspace_dir(agent.id).join(WORKSPACE_TOOLS);
        tokio::fs::write(&legacy_path, serde_json::to_string_pretty(&legacy).unwrap())
            .await
            .unwrap();

        let settings = store.read_agent_tools_settings(agent.id).await.unwrap();
        assert_eq!(settings.overrides.terminal.enabled, Some(legacy_enabled));
        assert_eq!(settings.overrides.max_llm_iterations, None);
        let migrated: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(legacy_path).await.unwrap()).unwrap();
        assert_eq!(migrated["version"], 1);
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
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
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
    async fn conversation_list_derives_title_and_message_count_from_messages() {
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
            .create_conversation(
                agent.id,
                CreateConversationRequest {
                    title: Some("新会话".to_string()),
                },
            )
            .await
            .unwrap();
        let message = ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: conversation.id,
            role: ChatRole::User,
            content: "  这是第一条用户消息\n用于生成标题  ".to_string(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
        };

        store
            .append_conversation_messages(conversation.id, std::slice::from_ref(&message))
            .await
            .unwrap();

        let listed = store.list_conversations(agent.id).await.unwrap();
        let listed_conversation = listed
            .iter()
            .find(|item| item.id == conversation.id)
            .unwrap();
        assert_eq!(listed_conversation.message_count, 1);
        assert_eq!(listed_conversation.title, "这是第一条用户消息 用于生成标题");
    }

    #[tokio::test]
    async fn initializes_builtin_agent_workspace_and_blocks_delete() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let agents = store.list_agents().await.unwrap();
        let builtin = agents.iter().find(|agent| agent.is_builtin).unwrap();

        assert_eq!(builtin.name, "Master");
        assert!(builtin.is_builtin);
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
        let tools_config = store.read_agent_tools_config(stored.id).await.unwrap();
        let prompt = store
            .build_agent_system_prompt(&stored, &tools_config)
            .await
            .unwrap();
        assert!(prompt.starts_with("## Environment"));
        assert!(prompt.contains("## Personality"));
        assert!(prompt.find("## Environment").unwrap() < prompt.find("## Personality").unwrap());
        assert!(prompt.contains("Calm and direct."));
        assert!(prompt.contains("User prefers concise plans."));
        assert!(prompt.contains("Ask only important questions."));
        let again = store
            .build_agent_system_prompt(&stored, &tools_config)
            .await
            .unwrap();
        assert_eq!(prompt, again);
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
