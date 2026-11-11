use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

use super::*;
use crate::domain::agents::UpdateAgentWorkspaceRequest;
use crate::domain::{
    ApiKeyStrategy, ChatMessage, ChatMessageStatus, ChatRole, ChatShortcutAction, ChatShortcutKind,
    ChatSystemEvent, ContextCheckpoint, ContextCompactionEvent, ContextCompactionSource,
    CreateAgentRequest, CreateConversationRequest, CreateModelProviderRequest, ImageAttachment,
    ImageSource, ProviderType, UpdateAgentProfileRequest, UpdateAgentRequest,
    UpdateConversationRequest, UpdateModelProviderRequest,
};

use crate::test_support::provider_request;

fn tool_table_names(conn: &rusqlite::Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table'
               AND (name LIKE 'tool_%' OR name = 'task_plans')
             ORDER BY name",
        )
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[tokio::test]
async fn initializes_settings_and_database() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();

    let settings = store.read_settings().await.unwrap();
    assert_eq!(settings.host, "127.0.0.1");
    assert!(temp_dir.path().join("settings.json").exists());
    assert!(temp_dir.path().join("weiagent.db").exists());
    assert!(store.list_model_providers().await.unwrap().is_empty());
    let defaults = store.read_tool_defaults().await.unwrap();
    assert_eq!(
        defaults.max_llm_iterations,
        crate::domain::tools::DEFAULT_MAX_LLM_ITERATIONS
    );
    assert!(defaults.terminal.enabled);
    assert!(defaults.web_search.enabled);
    assert!(defaults.task_planning.enabled);
    assert_eq!(
        defaults.task_planning.progress_interval,
        crate::domain::tools::DEFAULT_TASK_PLAN_PROGRESS_INTERVAL
    );
    assert_eq!(
        defaults
            .web_search
            .sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| source.id.as_str())
            .collect::<Vec<_>>(),
        vec!["keenable", "jina", "firecrawl"]
    );
    let conn = db::open_data_dir(temp_dir.path()).unwrap();
    let default_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tool_settings WHERE scope_type = 'default'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(default_rows, 1);
    let migration_versions = conn
        .prepare("SELECT version FROM app_schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        migration_versions,
        (1..=migrations::LATEST_SCHEMA_VERSION).collect::<Vec<_>>()
    );
    assert_eq!(
        tool_table_names(&conn),
        vec![
            "task_plans",
            "tool_mcp_agent_baselines",
            "tool_mcp_agent_seen_servers",
            "tool_mcp_agent_seen_tools",
            "tool_mcp_agent_servers",
            "tool_mcp_agent_tools",
            "tool_mcp_servers",
            "tool_mcp_tools",
            "tool_settings",
            "tool_terminal_approval_grants",
            "tool_web_search_agent_sources",
            "tool_web_search_api_keys",
            "tool_web_search_sources",
        ]
    );
}

#[test]
fn startup_migration_removes_legacy_stream_payloads() {
    let temp_dir = tempfile::tempdir().unwrap();
    let conn = db::open_data_dir(temp_dir.path()).unwrap();
    let stream_id = Uuid::new_v4().to_string();
    let regular_id = Uuid::new_v4().to_string();
    let now = Utc::now();

    for id in [&stream_id, &regular_id] {
        conn.execute(
            "INSERT INTO llm_requests (
                id, created_at, created_at_ms, agent_name, provider_name, model_id, has_payload
             ) VALUES (?1, ?2, ?3, 'Agent', 'Provider', 'model', 1)",
            params![id, now.to_rfc3339(), now.timestamp_millis()],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO llm_payloads (request_id, raw_request, raw_response)
         VALUES (?1, '', ?2)",
        params![
            stream_id,
            "data: {\"choices\":[{\"delta\":{\"content\":\"old\"}}]}\ndata: [DONE]"
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO llm_payloads (request_id, raw_request, raw_response)
         VALUES (?1, '', ?2)",
        params![
            regular_id,
            "{\"choices\":[{\"message\":{\"content\":\"keep\"}}]}"
        ],
    )
    .unwrap();
    conn.execute("DELETE FROM app_schema_migrations WHERE version = 26", [])
        .unwrap();
    drop(conn);

    let conn = db::open_data_dir(temp_dir.path()).unwrap();
    let stream_payload_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM llm_payloads WHERE request_id = ?1",
            [&stream_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stream_payload_count, 0);
    assert_eq!(
        conn.query_row(
            "SELECT has_payload FROM llm_requests WHERE id = ?1",
            [&stream_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT raw_response FROM llm_payloads WHERE request_id = ?1",
            [&regular_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "{\"choices\":[{\"message\":{\"content\":\"keep\"}}]}"
    );
}

#[tokio::test]
async fn upgrades_legacy_task_plans_before_creating_active_indexes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let conversation_id = Uuid::new_v4();
    let planning_id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    let conn = db::open_data_dir(temp_dir.path()).unwrap();
    conn.execute_batch(
        "CREATE TABLE task_plans (
            conversation_id TEXT PRIMARY KEY NOT NULL,
            planning_id TEXT NOT NULL UNIQUE,
            explanation TEXT,
            plan_json TEXT NOT NULL
                CHECK (json_valid(plan_json) AND json_type(plan_json) = 'array'),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        ) WITHOUT ROWID;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_plans (
            conversation_id, planning_id, explanation, plan_json, created_at, updated_at
         ) VALUES (?1, ?2, 'Legacy plan', ?3, ?4, ?4)",
        params![
            conversation_id.to_string(),
            planning_id.to_string(),
            r#"[{"step":"Legacy task","status":"pending"}]"#,
            now,
        ],
    )
    .unwrap();
    drop(conn);

    let _store = Store::new(temp_dir.path()).await.unwrap();
    let conn = db::open_data_dir(temp_dir.path()).unwrap();
    let columns = conn
        .prepare("PRAGMA table_info(task_plans)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(columns.iter().any(|column| column == "active"));
    assert_eq!(
        conn.query_row(
            "SELECT active FROM task_plans WHERE planning_id = ?1",
            [planning_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    assert!(
        conn.query_row(
            "SELECT 1 FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_task_plans_one_active_conversation'",
            [],
            |_| Ok(()),
        )
        .is_ok()
    );
}

#[test]
fn upgrades_v3_messages_with_response_items_column() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE app_schema_migrations (
            version INTEGER PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
         );
         INSERT INTO app_schema_migrations (version, name, applied_at)
         VALUES (3, 'conversations_sqlite_v3', '2026-01-01T00:00:00Z');
         CREATE TABLE conversation_messages (
            seq INTEGER PRIMARY KEY,
            reasoning_content TEXT
         );",
    )
    .unwrap();

    conversation_db::migrate_conversations(&mut conn).unwrap();

    let columns = conn
        .prepare("PRAGMA table_info(conversation_messages)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        columns
            .iter()
            .any(|name| name == "openai_response_items_json")
    );
    let applied: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM app_schema_migrations WHERE version = 4
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(applied);
}

#[tokio::test]
async fn repairs_key_status_slots_to_match_saved_keys() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let mut defaults = store.read_tool_defaults().await.unwrap();
    let source = defaults
        .web_search
        .sources
        .iter_mut()
        .find(|source| source.id == "firecrawl")
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
        .find(|source| source.id == "firecrawl")
        .unwrap();
    assert_eq!(source.api_keys.len(), 2);
    assert_eq!(source.key_statuses.len(), 2);
    assert_eq!(source.key_statuses[1].as_ref().unwrap().detail, "b");
}

#[tokio::test]
async fn source_without_key_is_forced_off_but_jina_stays_available() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let mut defaults = store.read_tool_defaults().await.unwrap();
    defaults
        .web_search
        .sources
        .iter_mut()
        .find(|source| source.id == "tavily")
        .unwrap()
        .enabled = true;

    let saved = store.update_tool_defaults(defaults).await.unwrap();
    let tavily = saved
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "tavily")
        .unwrap();
    let jina = saved
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "jina")
        .unwrap();
    assert!(!tavily.enabled);
    assert!(jina.enabled);
    assert!(jina.api_keys.is_empty());
}

#[tokio::test]
async fn restores_missing_builtin_web_search_sources_and_preserves_existing_keys() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let mut defaults = store.read_tool_defaults().await.unwrap();
    let firecrawl = defaults
        .web_search
        .sources
        .iter_mut()
        .find(|source| source.id == "firecrawl")
        .unwrap();
    firecrawl.api_keys = vec!["fc-secret".to_string()];
    store.update_tool_defaults(defaults).await.unwrap();

    let mut client_update = store.read_tool_defaults().await.unwrap().redact_secrets();
    client_update
        .web_search
        .sources
        .retain(|source| source.id != "firecrawl");
    let updated = store.update_tool_defaults(client_update).await.unwrap();
    let firecrawl = updated
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "firecrawl")
        .unwrap();
    assert_eq!(firecrawl.api_keys, vec!["fc-secret".to_string()]);

    let persisted = store.read_tool_defaults().await.unwrap();
    assert!(
        persisted
            .web_search
            .sources
            .iter()
            .any(|source| source.id == "firecrawl")
    );
}

#[tokio::test]
async fn agent_tool_overrides_inherit_global_defaults() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
    defaults.task_planning.progress_interval = 5;
    store.update_tool_defaults(defaults).await.unwrap();

    let inherited = store.read_agent_tools_settings(agent.id).await.unwrap();
    assert_eq!(inherited.effective.max_llm_iterations, 12);
    assert_eq!(inherited.effective.terminal.default_timeout_ms, 45_000);
    assert_eq!(inherited.effective.task_planning.progress_interval, 5);
    assert_eq!(inherited.overrides.max_llm_iterations, None);

    let mut overrides = inherited.overrides;
    overrides.terminal.enabled = Some(false);
    overrides.task_planning.progress_interval = Some(7);
    let updated = store
        .update_agent_tools_overrides(agent.id, overrides)
        .await
        .unwrap();
    assert!(!updated.effective.terminal.enabled);
    assert_eq!(updated.effective.max_llm_iterations, 12);
    assert_eq!(updated.effective.terminal.default_timeout_ms, 45_000);
    assert_eq!(updated.effective.task_planning.progress_interval, 7);

    let mut explicit_empty_roots = updated.overrides;
    explicit_empty_roots.terminal.allowed_cwd_roots = Some(Vec::new());
    let saved = store
        .update_agent_tools_overrides(agent.id, explicit_empty_roots)
        .await
        .unwrap();
    assert_eq!(saved.overrides.terminal.allowed_cwd_roots, Some(Vec::new()));

    let mut inherited_roots = saved.overrides;
    inherited_roots.terminal.allowed_cwd_roots = None;
    let saved = store
        .update_agent_tools_overrides(agent.id, inherited_roots)
        .await
        .unwrap();
    assert_eq!(saved.overrides.terminal.allowed_cwd_roots, None);
}

#[tokio::test]
async fn agent_web_search_source_overrides_round_trip_without_secrets() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let agent = store
        .list_agents()
        .await
        .unwrap()
        .into_iter()
        .find(|agent| agent.is_builtin)
        .unwrap();
    let mut defaults = store.read_tool_defaults().await.unwrap();
    let jina = defaults
        .web_search
        .sources
        .iter_mut()
        .find(|source| source.id == "jina")
        .unwrap();
    jina.api_keys = vec!["global-secret".to_string()];
    let jina_roles = vec![crate::domain::WebSearchSourceRole::OfficialDocs];
    store.update_tool_defaults(defaults).await.unwrap();

    let mut overrides = store
        .read_agent_tools_settings(agent.id)
        .await
        .unwrap()
        .overrides;
    overrides.web_search.source_overrides = vec![crate::domain::tools::WebSearchSourceOverrides {
        source_id: "jina".to_string(),
        enabled: Some(false),
        deep_enabled: Some(false),
        extract_enabled: Some(true),
        priority: Some(1),
        roles: Some(jina_roles.clone()),
        proxy_url: None,
        proxy_url_override: true,
    }];

    let saved = store
        .update_agent_tools_overrides(agent.id, overrides)
        .await
        .unwrap();
    let source = saved
        .effective
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "jina")
        .unwrap();
    assert!(!source.enabled);
    assert!(!source.deep_enabled);
    assert!(source.extract_enabled);
    assert_eq!(source.priority, 1);
    assert_eq!(source.roles, jina_roles);
    assert_eq!(source.proxy_url, None);
    assert_eq!(source.api_keys, vec!["global-secret".to_string()]);

    let mut reset = saved.overrides;
    reset.web_search.source_overrides.clear();
    let inherited = store
        .update_agent_tools_overrides(agent.id, reset)
        .await
        .unwrap();
    let source = inherited
        .effective
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "jina")
        .unwrap();
    assert!(source.enabled);
    assert!(source.deep_enabled);
    assert_eq!(source.priority, 40);
    assert_ne!(
        source.roles,
        vec![crate::domain::WebSearchSourceRole::OfficialDocs]
    );
}

#[tokio::test]
async fn web_search_attempts_update_matching_key_statuses() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let mut defaults = store.read_tool_defaults().await.unwrap();
    let source = defaults
        .web_search
        .sources
        .iter_mut()
        .find(|source| source.id == "firecrawl")
        .unwrap();
    source.api_keys = vec!["firecrawl-a".to_string(), "firecrawl-b".to_string()];
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
            operation: crate::domain::tools::WebSearchKeyOperation::Search,
            source_id: "firecrawl".to_string(),
            key_index: 0,
            api_key: "firecrawl-b".to_string(),
            record: crate::domain::WebSearchKeyStatusRecord {
                status: "quota_limited".to_string(),
                detail: "quota exceeded".to_string(),
                checked_at: "2026-07-22T00:00:00Z".to_string(),
            },
        },
        crate::domain::tools::WebSearchKeyStatusUpdate {
            operation: crate::domain::tools::WebSearchKeyOperation::Search,
            source_id: "firecrawl".to_string(),
            key_index: 0,
            api_key: "stale-key".to_string(),
            record: crate::domain::WebSearchKeyStatusRecord {
                status: "unauthorized".to_string(),
                detail: "stale".to_string(),
                checked_at: "2026-07-22T00:00:01Z".to_string(),
            },
        },
        crate::domain::tools::WebSearchKeyStatusUpdate {
            operation: crate::domain::tools::WebSearchKeyOperation::Search,
            source_id: "firecrawl".to_string(),
            key_index: 0,
            api_key: "firecrawl-a".to_string(),
            record: crate::domain::WebSearchKeyStatusRecord {
                status: "officially_blocked".to_string(),
                detail: "account is blocked".to_string(),
                checked_at: "2026-07-22T00:00:02Z".to_string(),
            },
        },
        crate::domain::tools::WebSearchKeyStatusUpdate {
            operation: crate::domain::tools::WebSearchKeyOperation::Extract,
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
        .find(|source| source.id == "firecrawl")
        .unwrap();
    assert_eq!(
        source.key_statuses[0].as_ref().unwrap().status,
        "officially_blocked"
    );
    assert_eq!(
        source.key_statuses[1].as_ref().unwrap().status,
        "quota_limited"
    );
    assert!(source.is_api_key_blocked("firecrawl-a"));
    assert_eq!(
        source.api_keys,
        vec!["firecrawl-a".to_string(), "firecrawl-b".to_string()]
    );
    let tavily = saved
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "tavily")
        .unwrap();
    assert_eq!(tavily.api_keys, vec!["tvly-a".to_string()]);
    assert!(tavily.key_statuses[0].is_none());
    assert_eq!(
        tavily.extract_key_statuses[0].as_ref().unwrap().status,
        "normal"
    );

    let mut client_update = saved.redact_secrets();
    let source = client_update
        .web_search
        .sources
        .iter_mut()
        .find(|source| source.id == "firecrawl")
        .unwrap();
    source.api_keys.clear();
    source.key_statuses.clear();
    store.update_tool_defaults(client_update).await.unwrap();
    let cleared = store.read_tool_defaults().await.unwrap();
    let source = cleared
        .web_search
        .sources
        .iter()
        .find(|source| source.id == "firecrawl")
        .unwrap();
    assert!(source.api_keys.is_empty());
    assert!(source.enabled);
    assert!(source.is_api_key_blocked("firecrawl-a"));
}

#[tokio::test]
async fn stores_provider_type_and_merges_key_pool_updates() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let provider = store
        .create_model_provider(CreateModelProviderRequest {
            name: "OpenAI".to_string(),
            api_keys: vec!["sk-test-secret".to_string()],
            models: vec!["gpt-4o-mini".to_string()],
            ..provider_request("OpenAI", "gpt-4o-mini")
        })
        .await
        .unwrap();

    assert_eq!(provider.provider_type, ProviderType::OpenAiCompatible);
    assert!(provider.has_api_key);
    assert_eq!(provider.masked_api_keys, vec!["****cret".to_string()]);
    assert_eq!(provider.key_strategy, ApiKeyStrategy::Failover);

    // Omitting api_keys leaves the stored pool untouched.
    let updated = store
        .update_model_provider(
            provider.id,
            UpdateModelProviderRequest {
                provider_type: Some(ProviderType::OpenAiCompatible),
                name: Some("OpenAI Compatible".to_string()),
                base_url: None,
                api_keys: None,
                api_key_labels: None,
                key_strategy: None,
                models: None,
                model_configs: None,
                extra_body: None,
                prompt_cache: None,
                cache_capabilities: None,
                proxy_url: None,
                image_generation_url: None,
                image_edit_url: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "OpenAI Compatible");
    assert_eq!(updated.provider_type, ProviderType::OpenAiCompatible);
    assert!(updated.has_api_key);

    // Masked entries keep their stored key; plaintext entries append.
    let expanded = store
        .update_model_provider(
            provider.id,
            UpdateModelProviderRequest {
                provider_type: None,
                name: None,
                base_url: None,
                api_keys: Some(vec!["****cret".to_string(), "sk-added-key".to_string()]),
                api_key_labels: Some(vec!["primary".to_string(), "backup".to_string()]),
                key_strategy: Some(ApiKeyStrategy::RoundRobin),
                models: None,
                model_configs: None,
                extra_body: None,
                prompt_cache: None,
                cache_capabilities: None,
                proxy_url: None,
                image_generation_url: None,
                image_edit_url: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        expanded.masked_api_keys,
        vec!["****cret".to_string(), "****-key".to_string()]
    );
    assert_eq!(expanded.key_strategy, ApiKeyStrategy::RoundRobin);
    assert_eq!(expanded.api_key_labels, vec!["primary", "backup"]);

    // The merged pool survives a reload from the database.
    let reloaded = store.get_model_provider(provider.id).await.unwrap();
    assert_eq!(
        reloaded.api_keys,
        vec!["sk-test-secret".to_string(), "sk-added-key".to_string()]
    );
    assert_eq!(reloaded.key_strategy, ApiKeyStrategy::RoundRobin);
    assert_eq!(reloaded.api_key_labels, vec!["primary", "backup"]);
}

#[tokio::test]
async fn stores_provider_without_an_api_key_and_can_clear_a_key_pool() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
    let provider = store
        .create_model_provider(CreateModelProviderRequest {
            name: "Anonymous provider".to_string(),
            api_keys: Vec::new(),
            models: vec!["public-model".to_string()],
            ..provider_request("Anonymous provider", "public-model")
        })
        .await
        .unwrap();

    assert!(!provider.has_api_key);
    assert!(provider.masked_api_keys.is_empty());
    assert!(
        store
            .get_model_provider(provider.id)
            .await
            .unwrap()
            .api_keys
            .is_empty()
    );

    let with_key = store
        .update_model_provider(
            provider.id,
            UpdateModelProviderRequest {
                provider_type: None,
                name: None,
                base_url: None,
                api_keys: Some(vec!["sk-temporary".to_string()]),
                api_key_labels: None,
                key_strategy: None,
                models: None,
                model_configs: None,
                extra_body: None,
                prompt_cache: None,
                cache_capabilities: None,
                proxy_url: None,
                image_generation_url: None,
                image_edit_url: None,
            },
        )
        .await
        .unwrap();
    assert!(with_key.has_api_key);

    let cleared = store
        .update_model_provider(
            provider.id,
            UpdateModelProviderRequest {
                provider_type: None,
                name: None,
                base_url: None,
                api_keys: Some(Vec::new()),
                api_key_labels: Some(Vec::new()),
                key_strategy: None,
                models: None,
                model_configs: None,
                extra_body: None,
                prompt_cache: None,
                cache_capabilities: None,
                proxy_url: None,
                image_generation_url: None,
                image_edit_url: None,
            },
        )
        .await
        .unwrap();
    assert!(!cleared.has_api_key);
    assert!(
        store
            .get_model_provider(provider.id)
            .await
            .unwrap()
            .api_keys
            .is_empty()
    );
}

#[tokio::test]
async fn deleting_provider_clears_agent_model_selection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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

    store.delete_model_provider(provider.id).await.unwrap();

    let updated_agent = store.get_agent(agent.id).await.unwrap();
    assert_eq!(updated_agent.model_provider_id, Uuid::nil());
    assert!(updated_agent.model_id.is_empty());
    assert!(store.list_model_providers().await.unwrap().is_empty());
}

#[tokio::test]
async fn validates_agent_provider_and_stores_conversations() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
        openai_response_items: vec![serde_json::json!({
            "type": "reasoning",
            "encrypted_content": "opaque-state"
        })],
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: vec![ChatShortcutAction {
            kind: ChatShortcutKind::Skill,
            id: "research".to_string(),
            name: "research".to_string(),
        }],
        system_event: Some(ChatSystemEvent::ContextCompacted(ContextCompactionEvent {
            source: ContextCompactionSource::Manual,
            threshold_percent: 80,
            before_tokens: 1000,
            after_tokens: 200,
            created_at: Utc::now(),
        })),
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
async fn repairs_missing_tool_outputs_idempotently() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
    let assistant = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: conversation.id,
        role: ChatRole::Assistant,
        content: String::new(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: Some(serde_json::json!([
            {
                "id": "call-missing",
                "type": "function",
                "function": {"name": "execute_terminal", "arguments": "{}"}
            },
            {
                "id": "call-complete",
                "type": "function",
                "function": {"name": "execute_terminal", "arguments": "{}"}
            }
        ])),
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
    };
    let completed = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: conversation.id,
        role: ChatRole::Tool,
        content: serde_json::json!({"status": "completed"}).to_string(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: Some("call-complete".to_string()),
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
    };
    store
        .append_conversation_messages(conversation.id, &[assistant, completed])
        .await
        .unwrap();

    store
        .repair_incomplete_tool_calls(conversation.id)
        .await
        .unwrap();
    store
        .repair_incomplete_tool_calls(conversation.id)
        .await
        .unwrap();

    let messages = store
        .read_conversation_messages(conversation.id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 3);
    let interrupted = messages.last().unwrap();
    assert_eq!(interrupted.role, ChatRole::Tool);
    assert_eq!(interrupted.tool_call_id.as_deref(), Some("call-missing"));
    let result: serde_json::Value = serde_json::from_str(&interrupted.content).unwrap();
    assert_eq!(result["status"], "interrupted");
    assert!(result["reason"].as_str().unwrap().contains("interrupted"));
}

#[tokio::test]
async fn conversation_append_preserves_context_checkpoint() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
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
    let store = Store::new(temp_dir.path()).await.unwrap();
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
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
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
async fn history_restart_removes_deleted_generated_images() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
    let generated_dir = store
        .attachments_dir
        .join("generated")
        .join(conversation.id.to_string());
    tokio::fs::create_dir_all(&generated_dir).await.unwrap();
    let generated_path = generated_dir.join("generated.png");
    tokio::fs::write(&generated_path, b"generated")
        .await
        .unwrap();
    let retained_path = store.attachments_dir.join("retained.png");
    tokio::fs::write(&retained_path, b"retained").await.unwrap();

    let user = |content: &str| ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: conversation.id,
        role: ChatRole::User,
        content: content.to_string(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
    };
    let first_user = user("first");
    let second_user = user("second");
    let removed_assistant = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: conversation.id,
        role: ChatRole::Assistant,
        content: String::new(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: vec![
            ImageAttachment {
                id: "generated".to_string(),
                name: "generated.png".to_string(),
                media_type: "image/png".to_string(),
                is_mask: false,
                source: ImageSource::Local {
                    path: generated_path.to_string_lossy().into_owned(),
                },
            },
            ImageAttachment {
                id: "retained".to_string(),
                name: "retained.png".to_string(),
                media_type: "image/png".to_string(),
                is_mask: false,
                source: ImageSource::Local {
                    path: retained_path.to_string_lossy().into_owned(),
                },
            },
        ],
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
    };
    store
        .append_conversation_messages(
            conversation.id,
            &[first_user, second_user.clone(), removed_assistant],
        )
        .await
        .unwrap();

    store
        .truncate_conversation_from_message(conversation.id, second_user.id)
        .await
        .unwrap();

    assert!(!generated_path.exists());
    assert!(retained_path.exists());
}

#[tokio::test]
async fn deleting_a_conversation_removes_generated_images() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
    let generated_dir = store
        .attachments_dir
        .join("generated")
        .join(conversation.id.to_string());
    tokio::fs::create_dir_all(&generated_dir).await.unwrap();
    let generated_path = generated_dir.join("generated.png");
    tokio::fs::write(&generated_path, b"generated")
        .await
        .unwrap();
    let message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: conversation.id,
        role: ChatRole::Assistant,
        content: String::new(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: vec![ImageAttachment {
            id: "generated".to_string(),
            name: "generated.png".to_string(),
            media_type: "image/png".to_string(),
            is_mask: false,
            source: ImageSource::Local {
                path: generated_path.to_string_lossy().into_owned(),
            },
        }],
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
    };
    store
        .append_conversation_messages(conversation.id, &[message])
        .await
        .unwrap();

    store.delete_conversation(conversation.id).await.unwrap();

    assert!(!generated_path.exists());
    assert!(!generated_dir.exists());
}

#[tokio::test]
async fn paginates_messages_on_user_turn_boundaries() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
                openai_response_items: Vec::new(),
                status: ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: None,
                tool_call_id: None,
                attachments: Vec::new(),
                usage: None,
                shortcut_actions: Vec::new(),
                system_event: None,
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
    let store = Store::new(temp_dir.path()).await.unwrap();
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
                title: Some("New Conversation".to_string()),
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
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
        shortcut_actions: Vec::new(),
        system_event: None,
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
    let store = Store::new(temp_dir.path()).await.unwrap();
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

    assert_eq!(store.list_conversations(agent.id).await.unwrap().len(), 1);
    assert_eq!(
        store.list_conversation_ids(agent.id).await.unwrap(),
        vec![conversation.id]
    );
}

#[tokio::test]
async fn pinned_conversations_sort_first_and_persist() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
    let reopened = Store::new(temp_dir.path()).await.unwrap();
    let persisted = reopened.list_conversations(agent.id).await.unwrap();
    assert_eq!(persisted[0].id, pinned.id);
    assert!(persisted[0].is_pinned);
}

#[tokio::test]
async fn initializes_builtin_agent_workspace_and_blocks_delete() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();

    let agents = store.list_agents().await.unwrap();
    let builtin = agents.iter().find(|agent| agent.is_builtin).unwrap();

    assert_eq!(builtin.name, "Wei Sir");
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
    assert_eq!(updated.name, "Wei Sir");
    assert_eq!(
        updated.description,
        "Coordinates tasks, delegates work, and drives complex jobs to completion."
    );
}

#[tokio::test]
async fn creates_workspace_and_composes_markdown_prompt() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = Store::new(temp_dir.path()).await.unwrap();
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
        .update_agent_profile(
            agent.id,
            UpdateAgentProfileRequest {
                agent: UpdateAgentRequest {
                    name: None,
                    description: None,
                    model_provider_id: None,
                    model_id: None,
                    avatar_icon: None,
                    avatar_color: None,
                },
                workspace: UpdateAgentWorkspaceRequest {
                    personality: "Calm and direct.".to_string(),
                    memory: "User prefers concise plans.".to_string(),
                    instructions: "Ask only important questions.".to_string(),
                },
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
    assert!(prompt.contains(
        "The agent handling this conversation receives user messages, calls tools, and replies:"
    ));
    assert!(prompt.contains(&format!("- Name: {}", stored.name)));
    assert!(prompt.contains(&format!("- ID: {}", stored.id)));
    assert!(prompt.contains("## Runtime"));
    assert!(prompt.contains("## Task Planning Policy"));
    assert!(prompt.contains("## File Discovery and Reading Policy"));
    assert!(prompt.contains("node_modules"));
    assert!(prompt.contains("target"));
    assert!(prompt.contains(".venv"));
    assert!(prompt.contains(
        "Every non-planning tool call must include the current conversation's planning_id"
    ));
    assert!(prompt.contains(&format!("- Date: {}", crate::tools::prompt::today_date())));
    assert!(prompt.contains("The current local system date is today's date and is authoritative."));
    assert!(!prompt.contains("Trust: absolute"));
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
    assert!(prompt.contains(
        "Use web search for current or source-backed information, verify important claims with original pages, and cite the supporting links."
    ));

    tools_config.web_search.enabled = false;
    let no_search_prompt = store
        .build_agent_system_prompt(&stored, &tools_config)
        .await
        .unwrap();
    assert!(!no_search_prompt.contains(
        "Use web search for current or source-backed information, verify important claims with original pages, and cite the supporting links."
    ));
    assert!(no_search_prompt.contains("Task Planning Policy"));

    tools_config.task_planning.enabled = false;
    let no_planning_prompt = store
        .build_agent_system_prompt(&stored, &tools_config)
        .await
        .unwrap();
    assert!(!no_planning_prompt.contains("Task Planning Policy"));

    tools_config.terminal.enabled = false;
    let no_terminal_prompt = store
        .build_agent_system_prompt(&stored, &tools_config)
        .await
        .unwrap();
    assert!(!no_terminal_prompt.contains("## File Discovery and Reading Policy"));

    tools_config.task_planning.enabled = true;
    tools_config.terminal.enabled = true;
    tools_config.web_search.enabled = true;
    let search_prompt = store
        .build_agent_system_prompt(&stored, &tools_config)
        .await
        .unwrap();
    assert!(search_prompt.contains(
        "Use web search for current or source-backed information, verify important claims with original pages, and cite the supporting links."
    ));
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
    let store = Store::new(temp_dir.path()).await.unwrap();
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
    let store = Store::new(temp_dir.path()).await.unwrap();
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
