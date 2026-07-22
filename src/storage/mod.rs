mod agents;
mod conversations;
mod db;
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
        ChatMessage, ChatMessageStatus, ChatRole, ContextCheckpoint, CreateAgentRequest,
        CreateConversationRequest, CreateModelProviderRequest, ProviderType,
        UpdateAgentProfileRequest, UpdateAgentRequest, UpdateAgentWorkspaceRequest,
        UpdateConversationRequest, UpdateModelProviderRequest,
    };

    fn provider_request(name: &str, model: &str) -> CreateModelProviderRequest {
        CreateModelProviderRequest {
            provider_type: ProviderType::OpenAiCompatible,
            name: name.to_string(),
            base_url: "https://example.test/v1".to_string(),
            api_key: "sk-test".to_string(),
            models: vec![model.to_string()],
            model_configs: Vec::new(),
            extra_body: serde_json::Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
        }
    }

    #[tokio::test]
    async fn initializes_settings_and_providers_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let settings = store.read_settings().await.unwrap();
        assert_eq!(settings.host, "127.0.0.1");
        assert!(temp_dir.path().join("settings.json").exists());
        assert!(temp_dir.path().join("weiagent.db").exists());
        assert!(store.list_model_providers().await.unwrap().is_empty());
        assert!(temp_dir.path().join("tool_defaults.json").exists());
    }

    #[tokio::test]
    async fn repairs_key_status_slots_to_match_saved_keys() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let mut defaults = store.read_tool_defaults().await.unwrap();
        let source = defaults
            .web_search
            .sources
            .iter_mut()
            .find(|source| source.id == "grok")
            .unwrap();
        source.api_keys = vec!["key-a".to_string(), "key-b".to_string()];
        source.key_statuses = vec![
            Some(crate::domain::WebSearchKeyStatusRecord {
                status: "normal".to_string(),
                detail: "a".to_string(),
                checked_at: Utc::now().to_rfc3339(),
            }),
            Some(crate::domain::WebSearchKeyStatusRecord {
                status: "normal".to_string(),
                detail: "b".to_string(),
                checked_at: Utc::now().to_rfc3339(),
            }),
            Some(crate::domain::WebSearchKeyStatusRecord {
                status: "error".to_string(),
                detail: "stale".to_string(),
                checked_at: Utc::now().to_rfc3339(),
            }),
        ];
        store.update_tool_defaults(defaults).await.unwrap();

        let repaired = store.read_tool_defaults().await.unwrap();
        let source = repaired
            .web_search
            .sources
            .iter()
            .find(|source| source.id == "grok")
            .unwrap();
        assert_eq!(source.api_keys.len(), 2);
        assert_eq!(source.key_statuses.len(), 2);
        assert_eq!(source.key_statuses[1].as_ref().unwrap().detail, "b");
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
    async fn web_search_attempts_update_matching_key_statuses() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let mut defaults = store.read_tool_defaults().await.unwrap();
        let source = defaults
            .web_search
            .sources
            .iter_mut()
            .find(|source| source.id == "grok")
            .unwrap();
        source.api_keys = vec!["grok-a".to_string(), "grok-b".to_string()];
        let tavily = defaults
            .web_search
            .sources
            .iter_mut()
            .find(|source| source.id == "tavily")
            .unwrap();
        tavily.api_keys = vec!["tvly-a".to_string()];
        store.update_tool_defaults(defaults).await.unwrap();

        let updates = vec![
            crate::domain::tools::WebSearchKeyStatusUpdate {
                source_id: "grok".to_string(),
                key_index: 0,
                api_key: "grok-b".to_string(),
                record: crate::domain::WebSearchKeyStatusRecord {
                    status: "quota_limited".to_string(),
                    detail: "quota exceeded".to_string(),
                    checked_at: "2026-07-22T00:00:00Z".to_string(),
                },
            },
            crate::domain::tools::WebSearchKeyStatusUpdate {
                source_id: "grok".to_string(),
                key_index: 0,
                api_key: "stale-key".to_string(),
                record: crate::domain::WebSearchKeyStatusRecord {
                    status: "unauthorized".to_string(),
                    detail: "stale".to_string(),
                    checked_at: "2026-07-22T00:00:01Z".to_string(),
                },
            },
            crate::domain::tools::WebSearchKeyStatusUpdate {
                source_id: "grok".to_string(),
                key_index: 0,
                api_key: "grok-a".to_string(),
                record: crate::domain::WebSearchKeyStatusRecord {
                    status: "officially_blocked".to_string(),
                    detail: "account is blocked".to_string(),
                    checked_at: "2026-07-22T00:00:02Z".to_string(),
                },
            },
            crate::domain::tools::WebSearchKeyStatusUpdate {
                source_id: "tavily".to_string(),
                key_index: 0,
                api_key: "tvly-a".to_string(),
                record: crate::domain::WebSearchKeyStatusRecord {
                    status: "normal".to_string(),
                    detail: "request succeeded".to_string(),
                    checked_at: "2026-07-22T00:00:03Z".to_string(),
                },
            },
        ];

        let updated = store
            .update_web_search_key_statuses(&updates)
            .await
            .unwrap();
        assert_eq!(updated, 3);

        let saved = store.read_tool_defaults().await.unwrap();
        let source = saved
            .web_search
            .sources
            .iter()
            .find(|source| source.id == "grok")
            .unwrap();
        assert_eq!(
            source.key_statuses[0].as_ref().unwrap().status,
            "officially_blocked"
        );
        assert_eq!(
            source.key_statuses[1].as_ref().unwrap().status,
            "quota_limited"
        );
        assert!(source.is_api_key_blocked("grok-a"));
        assert_eq!(
            source.api_keys,
            vec!["grok-a".to_string(), "grok-b".to_string()]
        );
        let tavily = saved
            .web_search
            .sources
            .iter()
            .find(|source| source.id == "tavily")
            .unwrap();
        assert_eq!(tavily.api_keys, vec!["tvly-a".to_string()]);
        assert_eq!(tavily.key_statuses[0].as_ref().unwrap().status, "normal");

        let mut client_update = saved.redact_secrets();
        let source = client_update
            .web_search
            .sources
            .iter_mut()
            .find(|source| source.id == "grok")
            .unwrap();
        source.api_keys.clear();
        source.key_statuses.clear();
        store.update_tool_defaults(client_update).await.unwrap();
        let cleared = store.read_tool_defaults().await.unwrap();
        let source = cleared
            .web_search
            .sources
            .iter()
            .find(|source| source.id == "grok")
            .unwrap();
        assert!(source.api_keys.is_empty());
        assert!(source.is_api_key_blocked("grok-a"));
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
                    model_configs: None,
                    extra_body: None,
                    prompt_cache: None,
                    cache_capabilities: None,
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
                description: "Plans the work.".to_string(),
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
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
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
    async fn conversation_append_preserves_context_checkpoint() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let agent = store
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|agent| agent.is_builtin)
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let old = ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: conversation.id,
            role: ChatRole::User,
            content: "old".to_string(),
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        };
        store
            .append_conversation_messages(conversation.id, std::slice::from_ref(&old))
            .await
            .unwrap();
        let checkpoint = ContextCheckpoint {
            summary: "old state".to_string(),
            covers_until_message_id: old.id,
            created_at: Utc::now(),
            generation: 1,
            source_tokens_estimate: 100,
        };
        store
            .write_context_checkpoint(conversation.id, checkpoint.clone())
            .await
            .unwrap();
        let new_message = ChatMessage {
            id: Uuid::new_v4(),
            content: "new".to_string(),
            ..old
        };

        store
            .append_conversation_messages(conversation.id, &[new_message])
            .await
            .unwrap();

        assert_eq!(
            store
                .read_context_checkpoint(conversation.id)
                .await
                .unwrap(),
            Some(checkpoint)
        );
    }

    #[tokio::test]
    async fn history_restart_only_invalidates_intersected_checkpoint() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let agent = store
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|agent| agent.is_builtin)
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let make_message = |role, content: &str| ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: conversation.id,
            role,
            content: content.to_string(),
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        };
        let first_user = make_message(ChatRole::User, "first");
        let first_assistant = make_message(ChatRole::Assistant, "answer");
        let second_user = make_message(ChatRole::User, "second");
        let second_assistant = make_message(ChatRole::Assistant, "answer two");
        store
            .append_conversation_messages(
                conversation.id,
                &[
                    first_user.clone(),
                    first_assistant.clone(),
                    second_user.clone(),
                    second_assistant,
                ],
            )
            .await
            .unwrap();
        let checkpoint = ContextCheckpoint {
            summary: "first turn".to_string(),
            covers_until_message_id: first_assistant.id,
            created_at: Utc::now(),
            generation: 1,
            source_tokens_estimate: 100,
        };
        store
            .write_context_checkpoint(conversation.id, checkpoint.clone())
            .await
            .unwrap();

        store
            .truncate_conversation_from_message(conversation.id, second_user.id)
            .await
            .unwrap();
        assert_eq!(
            store
                .read_context_checkpoint(conversation.id)
                .await
                .unwrap(),
            Some(checkpoint)
        );

        store
            .truncate_conversation_from_message(conversation.id, first_user.id)
            .await
            .unwrap();
        assert!(
            store
                .read_context_checkpoint(conversation.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn paginates_messages_on_user_turn_boundaries() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(provider_request("OpenAI", "gpt-4o-mini"))
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Planner".to_string(),
                description: "Plans the work.".to_string(),
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
        let mut messages = Vec::new();
        for turn in 0..6 {
            for role in [ChatRole::User, ChatRole::Assistant] {
                messages.push(ChatMessage {
                    id: Uuid::new_v4(),
                    agent_id: agent.id,
                    conversation_id: conversation.id,
                    role,
                    content: format!("turn {turn}"),
                    reasoning_content: None,
                    status: ChatMessageStatus::Done,
                    created_at: Utc::now(),
                    tool_calls: None,
                    tool_call_id: None,
                    attachments: Vec::new(),
                    usage: None,
                });
            }
        }
        store
            .append_conversation_messages(conversation.id, &messages)
            .await
            .unwrap();

        let latest = store
            .read_conversation_message_page(conversation.id, None, 5)
            .await
            .unwrap();
        assert_eq!(latest.messages.len(), 6);
        assert_eq!(latest.messages[0].role, ChatRole::User);
        assert!(latest.has_more);

        let older = store
            .read_conversation_message_page(conversation.id, latest.next_cursor, 5)
            .await
            .unwrap();
        assert_eq!(older.messages.len(), 6);
        assert_eq!(older.messages[0].role, ChatRole::User);
        assert!(!older.has_more);
        assert_eq!(
            older
                .messages
                .iter()
                .chain(latest.messages.iter())
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>()
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
                description: "Plans conversations.".to_string(),
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
            reasoning_content: None,
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
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
    async fn conversation_lists_do_not_reparse_message_history() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(provider_request("Fast List", "fast-model"))
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Fast List Agent".to_string(),
                description: "Exercises metadata-only conversation listing.".to_string(),
                model_provider_id: provider.id,
                model_id: "fast-model".to_string(),
                avatar_icon: Some("rocket".to_string()),
                avatar_color: Some("#ef4444".to_string()),
            })
            .await
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        tokio::fs::write(store.conversation_file(conversation.id), "{broken-json")
            .await
            .unwrap();

        assert_eq!(store.list_conversations(agent.id).await.unwrap().len(), 1);
        assert_eq!(
            store.list_conversation_ids(agent.id).await.unwrap(),
            vec![conversation.id]
        );
    }

    #[tokio::test]
    async fn pinned_conversations_sort_first_and_persist() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let agent = store
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|agent| agent.is_builtin)
            .unwrap();
        let pinned = store
            .create_conversation(
                agent.id,
                CreateConversationRequest {
                    title: Some("Pinned".to_string()),
                },
            )
            .await
            .unwrap();
        let recent = store
            .create_conversation(
                agent.id,
                CreateConversationRequest {
                    title: Some("Recent".to_string()),
                },
            )
            .await
            .unwrap();

        let updated = store
            .update_conversation(
                pinned.id,
                UpdateConversationRequest {
                    is_pinned: true,
                    title: None,
                },
            )
            .await
            .unwrap();
        assert!(updated.is_pinned);

        let listed = store.list_conversations(agent.id).await.unwrap();
        assert_eq!(listed[0].id, pinned.id);
        assert_eq!(listed[1].id, recent.id);

        drop(store);
        let reopened = JsonStore::new(temp_dir.path()).await.unwrap();
        let persisted = reopened.list_conversations(agent.id).await.unwrap();
        assert_eq!(persisted[0].id, pinned.id);
        assert!(persisted[0].is_pinned);
    }

    #[tokio::test]
    async fn initializes_builtin_agent_workspace_and_blocks_delete() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let agents = store.list_agents().await.unwrap();
        let builtin = agents.iter().find(|agent| agent.is_builtin).unwrap();

        assert_eq!(builtin.name, "Master");
        assert_eq!(
            builtin.description,
            "Coordinates tasks, delegates work, and drives complex jobs to completion."
        );
        assert!(builtin.is_builtin);
        assert!(
            temp_dir
                .path()
                .join("agents")
                .join(builtin.id.to_string())
                .exists()
        );
        assert!(store.delete_agent(builtin.id).await.is_err());

        let provider = store
            .create_model_provider(provider_request("Core", "core-model"))
            .await
            .unwrap();
        let updated = store
            .update_agent(
                builtin.id,
                UpdateAgentRequest {
                    name: Some("Renamed".to_string()),
                    description: Some("Changed description".to_string()),
                    model_provider_id: Some(provider.id),
                    model_id: Some("core-model".to_string()),
                    avatar_icon: None,
                    avatar_color: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "Master");
        assert_eq!(
            updated.description,
            "Coordinates tasks, delegates work, and drives complex jobs to completion."
        );
    }

    #[tokio::test]
    async fn migrates_missing_agent_descriptions() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_id = Uuid::new_v4();
        let now = Utc::now();
        let legacy_agents = serde_json::json!([{
            "id": legacy_id,
            "name": "Legacy",
            "model_provider_id": Uuid::nil(),
            "model_id": "legacy-model",
            "is_builtin": false,
            "avatar_icon": "bot",
            "avatar_color": "#22c55e",
            "created_at": now,
            "updated_at": now
        }]);
        tokio::fs::write(
            temp_dir.path().join("agents.json"),
            serde_json::to_string_pretty(&legacy_agents).unwrap(),
        )
        .await
        .unwrap();

        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let migrated = store.get_agent(legacy_id).await.unwrap();
        assert_eq!(migrated.description, "Custom AI agent");

        let persisted: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(temp_dir.path().join("agents.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(persisted[0]["description"], "Custom AI agent");
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
                description: "Operates local tools.".to_string(),
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
        let mut tools_config = crate::domain::AgentToolsConfig::default();
        let prompt = store
            .build_agent_system_prompt(&stored, &tools_config)
            .await
            .unwrap();
        assert!(prompt.starts_with("## Environment"));
        assert!(prompt.contains("## Agent"));
        assert!(prompt.contains("当前处理本轮对话的智能体"));
        assert!(prompt.contains(&format!("- Name: {}", stored.name)));
        assert!(prompt.contains(&format!("- ID: {}", stored.id)));
        assert!(prompt.contains("## Runtime"));
        assert!(prompt.contains(&format!("- Date: {}", crate::tools::prompt::today_date())));
        assert!(prompt.contains("必须以此为准"));
        assert!(prompt.contains("Trust: absolute"));
        assert!(prompt.contains("## Personality"));
        assert!(prompt.find("## Environment").unwrap() < prompt.find("## Agent").unwrap());
        assert!(prompt.find("## Agent").unwrap() < prompt.find("## Runtime").unwrap());
        assert!(prompt.find("## Runtime").unwrap() < prompt.find("## Personality").unwrap());
        assert!(prompt.contains("Calm and direct."));
        assert!(prompt.contains("User prefers concise plans."));
        assert!(prompt.contains("Ask only important questions."));
        let again = store
            .build_agent_system_prompt(&stored, &tools_config)
            .await
            .unwrap();
        assert_eq!(prompt, again);
        assert!(!prompt.contains("Web Search Policy V1"));

        tools_config.web_search.enabled = true;
        let search_prompt = store
            .build_agent_system_prompt(&stored, &tools_config)
            .await
            .unwrap();
        assert!(search_prompt.contains("## Web Search Policy V1"));
        let search_hash = crate::llm::cache::system_prompt_hash(&search_prompt);
        tools_config.web_search.sources[0].api_keys = vec!["dynamic-key".to_string()];
        tools_config.web_search.sources[0].priority = 999;
        tools_config.web_search.sources[0].key_statuses =
            vec![Some(crate::domain::WebSearchKeyStatusRecord {
                status: "normal".to_string(),
                detail: "dynamic health".to_string(),
                checked_at: chrono::Utc::now().to_rfc3339(),
            })];
        for source in &mut tools_config.web_search.sources {
            source.enabled = false;
        }
        let source_changed_prompt = store
            .build_agent_system_prompt(&stored, &tools_config)
            .await
            .unwrap();
        assert_eq!(
            search_hash,
            crate::llm::cache::system_prompt_hash(&source_changed_prompt)
        );
    }

    #[tokio::test]
    async fn updates_agent_profile_and_workspace_together() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(provider_request("Local", "local-model"))
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Operator".to_string(),
                description: "Original description".to_string(),
                model_provider_id: provider.id,
                model_id: "local-model".to_string(),
                avatar_icon: Some("brain".to_string()),
                avatar_color: Some("#8b5cf6".to_string()),
            })
            .await
            .unwrap();

        let updated = store
            .update_agent_profile(
                agent.id,
                UpdateAgentProfileRequest {
                    agent: UpdateAgentRequest {
                        name: Some("Planner".to_string()),
                        description: Some("Updated description".to_string()),
                        model_provider_id: None,
                        model_id: None,
                        avatar_icon: None,
                        avatar_color: None,
                    },
                    workspace: UpdateAgentWorkspaceRequest {
                        personality: "Calm".to_string(),
                        memory: "Project context".to_string(),
                        instructions: "Make a plan".to_string(),
                    },
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.agent.name, "Planner");
        assert_eq!(updated.workspace.instructions, "Make a plan");
        assert_eq!(
            store.read_agent_workspace(agent.id).await.unwrap(),
            updated.workspace
        );

        let rejected = store
            .update_agent_profile(
                agent.id,
                UpdateAgentProfileRequest {
                    agent: UpdateAgentRequest {
                        name: Some("Invalid".to_string()),
                        description: None,
                        model_provider_id: None,
                        model_id: Some("missing-model".to_string()),
                        avatar_icon: None,
                        avatar_color: None,
                    },
                    workspace: UpdateAgentWorkspaceRequest {
                        personality: "Should not persist".to_string(),
                        memory: String::new(),
                        instructions: String::new(),
                    },
                },
            )
            .await;
        assert!(rejected.is_err());
        assert_eq!(store.get_agent(agent.id).await.unwrap().name, "Planner");
        assert_eq!(
            store
                .read_agent_workspace(agent.id)
                .await
                .unwrap()
                .personality,
            "Calm"
        );
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
                description: "First avatar test agent.".to_string(),
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
                description: "Second avatar test agent.".to_string(),
                model_provider_id: provider.id,
                model_id: "avatar-model".to_string(),
                avatar_icon: Some("compass".to_string()),
                avatar_color: Some("#06b6d4".to_string()),
            })
            .await;

        assert!(duplicate.is_err());
    }
}
