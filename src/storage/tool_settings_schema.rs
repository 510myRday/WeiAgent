use rusqlite::{Connection, params};

use super::sql_util::{column_exists, now_rfc3339};
use crate::domain::tools::DEFAULT_TASK_PLAN_PROGRESS_INTERVAL;
use crate::error::AppResult;

const TOOL_SETTINGS_SCHEMA_VERSION: i64 = 12;

const TOOL_SETTINGS_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS tool_settings (
    scope_type TEXT NOT NULL CHECK (scope_type IN ('default', 'agent')),
    scope_id TEXT NOT NULL,
    config_version INTEGER NOT NULL DEFAULT 1 CHECK (config_version = 1),
    max_llm_iterations INTEGER
        CHECK (max_llm_iterations IS NULL OR max_llm_iterations BETWEEN 1 AND 999),
    terminal_enabled INTEGER
        CHECK (terminal_enabled IS NULL OR terminal_enabled IN (0, 1)),
    terminal_permission_mode TEXT
        CHECK (terminal_permission_mode IS NULL OR terminal_permission_mode IN ('request_approval', 'full_access')),
    terminal_default_timeout_ms INTEGER
        CHECK (terminal_default_timeout_ms IS NULL OR terminal_default_timeout_ms > 0),
    terminal_allowed_cwd_roots_json TEXT
        CHECK (terminal_allowed_cwd_roots_json IS NULL OR (json_valid(terminal_allowed_cwd_roots_json) AND json_type(terminal_allowed_cwd_roots_json) = 'array')),
    web_search_enabled INTEGER
        CHECK (web_search_enabled IS NULL OR web_search_enabled IN (0, 1)),
    web_search_max_results INTEGER
        CHECK (web_search_max_results IS NULL OR web_search_max_results > 0),
    web_search_timeout_ms INTEGER
        CHECK (web_search_timeout_ms IS NULL OR web_search_timeout_ms > 0),
    web_search_key_selection TEXT
        CHECK (web_search_key_selection IS NULL OR web_search_key_selection IN ('sequential', 'round_robin', 'random')),
    web_extract_remote_enabled INTEGER
        CHECK (web_extract_remote_enabled IS NULL OR web_extract_remote_enabled IN (0, 1)),
    web_extract_max_remote_attempts INTEGER
        CHECK (web_extract_max_remote_attempts IS NULL OR web_extract_max_remote_attempts BETWEEN 1 AND 5),
    web_extract_allow_grok_fallback INTEGER
        CHECK (web_extract_allow_grok_fallback IS NULL OR web_extract_allow_grok_fallback IN (0, 1)),
    evidence_storage_enabled INTEGER
        CHECK (evidence_storage_enabled IS NULL OR evidence_storage_enabled IN (0, 1)),
    evidence_retention_days INTEGER
        CHECK (evidence_retention_days IS NULL OR evidence_retention_days > 0),
    evidence_max_total_mb INTEGER
        CHECK (evidence_max_total_mb IS NULL OR evidence_max_total_mb > 0),
    task_planning_enabled INTEGER
        CHECK (task_planning_enabled IS NULL OR task_planning_enabled IN (0, 1)),
    task_planning_progress_interval INTEGER
        CHECK (task_planning_progress_interval IS NULL OR task_planning_progress_interval BETWEEN 1 AND 999),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope_type, scope_id),
    UNIQUE (scope_id),
    CHECK (
        (
            scope_type = 'default'
            AND scope_id = 'default'
            AND max_llm_iterations IS NOT NULL
            AND terminal_enabled IS NOT NULL
            AND terminal_permission_mode IS NOT NULL
            AND terminal_default_timeout_ms IS NOT NULL
            AND terminal_allowed_cwd_roots_json IS NOT NULL
            AND web_search_enabled IS NOT NULL
            AND web_search_max_results IS NOT NULL
            AND web_search_timeout_ms IS NOT NULL
            AND web_search_key_selection IS NOT NULL
            AND web_extract_remote_enabled IS NOT NULL
            AND web_extract_max_remote_attempts IS NOT NULL
            AND web_extract_allow_grok_fallback IS NOT NULL
            AND evidence_storage_enabled IS NOT NULL
            AND evidence_retention_days IS NOT NULL
            AND evidence_max_total_mb IS NOT NULL
            AND task_planning_enabled IS NOT NULL
            AND task_planning_progress_interval IS NOT NULL
        )
        OR
        (
            scope_type = 'agent'
            AND length(scope_id) = 36
            AND web_search_timeout_ms IS NULL
            AND web_search_key_selection IS NULL
            AND web_extract_remote_enabled IS NULL
            AND web_extract_max_remote_attempts IS NULL
            AND web_extract_allow_grok_fallback IS NULL
            AND evidence_storage_enabled IS NULL
            AND evidence_retention_days IS NULL
            AND evidence_max_total_mb IS NULL
        )
    )
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tool_terminal_approval_grants (
    agent_id TEXT NOT NULL CHECK (length(agent_id) = 36),
    position INTEGER NOT NULL CHECK (position >= 0),
    command TEXT NOT NULL CHECK (length(trim(command)) > 0),
    cwd TEXT,
    permission_mode TEXT NOT NULL CHECK (permission_mode = 'request_approval'),
    created_at TEXT,
    expires_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, position),
    FOREIGN KEY (agent_id) REFERENCES tool_settings(scope_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_tool_terminal_grants_expiry
    ON tool_terminal_approval_grants(expires_at);

CREATE TABLE IF NOT EXISTS task_plans (
    planning_id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL,
    conversation_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
    explanation TEXT,
    plan_json TEXT NOT NULL
        CHECK (json_valid(plan_json) AND json_type(plan_json) = 'array'),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tool_web_search_sources (
    source_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(source_id)) > 0),
    scope_id TEXT NOT NULL DEFAULT 'default' CHECK (scope_id = 'default'),
    position INTEGER NOT NULL UNIQUE CHECK (position >= 0),
    provider TEXT NOT NULL CHECK (provider IN ('tavily', 'exa', 'jina', 'grok', 'firecrawl')),
    display_name TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    deep_enabled INTEGER NOT NULL CHECK (deep_enabled IN (0, 1)),
    extract_enabled INTEGER NOT NULL DEFAULT 1 CHECK (extract_enabled IN (0, 1)),
    priority INTEGER NOT NULL CHECK (priority >= 0),
    independence_group TEXT NOT NULL,
    roles_json TEXT NOT NULL
        CHECK (json_valid(roles_json) AND json_type(roles_json) = 'array'),
    blocked_api_key_hashes_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(blocked_api_key_hashes_json) AND json_type(blocked_api_key_hashes_json) = 'array'),
    proxy_url TEXT,
    FOREIGN KEY (scope_id) REFERENCES tool_settings(scope_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tool_web_search_agent_sources (
    agent_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    enabled INTEGER
        CHECK (enabled IS NULL OR enabled IN (0, 1)),
    deep_enabled INTEGER
        CHECK (deep_enabled IS NULL OR deep_enabled IN (0, 1)),
    extract_enabled INTEGER
        CHECK (extract_enabled IS NULL OR extract_enabled IN (0, 1)),
    priority INTEGER
        CHECK (priority IS NULL OR priority >= 0),
    roles_json TEXT
        CHECK (roles_json IS NULL OR (json_valid(roles_json) AND json_type(roles_json) = 'array')),
    proxy_url_override INTEGER NOT NULL DEFAULT 0 CHECK (proxy_url_override IN (0, 1)),
    proxy_url TEXT,
    PRIMARY KEY (agent_id, source_id),
    FOREIGN KEY (agent_id) REFERENCES tool_settings(scope_id) ON DELETE CASCADE,
    FOREIGN KEY (source_id) REFERENCES tool_web_search_sources(source_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_tool_web_search_agent_sources_agent
    ON tool_web_search_agent_sources(agent_id);

CREATE TABLE IF NOT EXISTS tool_web_search_api_keys (
    source_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    api_key TEXT NOT NULL CHECK (length(trim(api_key)) > 0),
    key_status TEXT,
    status_detail TEXT,
    checked_at TEXT,
    extract_key_status TEXT,
    extract_status_detail TEXT,
    extract_checked_at TEXT,
    PRIMARY KEY (source_id, position),
    UNIQUE (source_id, api_key),
    FOREIGN KEY (source_id) REFERENCES tool_web_search_sources(source_id) ON DELETE CASCADE
) WITHOUT ROWID;
"#;

pub(super) fn ensure_tool_settings_schema(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_schema_migrations (
            version INTEGER PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )?;
    let applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [TOOL_SETTINGS_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    conn.execute_batch(TOOL_SETTINGS_SCHEMA_SQL)?;
    if !column_exists(conn, "tool_settings", "task_planning_enabled")? {
        conn.execute(
            "ALTER TABLE tool_settings ADD COLUMN task_planning_enabled INTEGER",
            [],
        )?;
        conn.execute(
            "UPDATE tool_settings
             SET task_planning_enabled = CASE
                 WHEN scope_type = 'default' THEN 1
                 ELSE NULL
             END",
            [],
        )?;
    }
    if !column_exists(conn, "tool_settings", "task_planning_progress_interval")? {
        conn.execute(
            "ALTER TABLE tool_settings ADD COLUMN task_planning_progress_interval INTEGER
             CHECK (task_planning_progress_interval IS NULL OR task_planning_progress_interval BETWEEN 1 AND 999)",
            [],
        )?;
        conn.execute(
            "UPDATE tool_settings
             SET task_planning_progress_interval = CASE
                 WHEN scope_type = 'default' THEN ?1
                 ELSE NULL
             END",
            [i64::from(DEFAULT_TASK_PLAN_PROGRESS_INTERVAL)],
        )?;
    }
    conn.execute(
        "UPDATE tool_settings SET task_planning_enabled = 1,
                task_planning_progress_interval = COALESCE(task_planning_progress_interval, ?1)
         WHERE scope_type = 'default' AND scope_id = 'default' AND task_planning_enabled IS NULL",
        [i64::from(DEFAULT_TASK_PLAN_PROGRESS_INTERVAL)],
    )?;
    conn.execute(
        "UPDATE tool_settings SET task_planning_progress_interval = ?1
         WHERE scope_type = 'default' AND scope_id = 'default'
           AND task_planning_progress_interval IS NULL",
        [i64::from(DEFAULT_TASK_PLAN_PROGRESS_INTERVAL)],
    )?;
    migrate_task_plans_schema(conn)?;
    ensure_task_plan_indexes(conn)?;
    let legacy_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = 2
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !legacy_applied {
        conn.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (2, ?1, ?2)",
            params!["tool_settings_compact_v2", now_rfc3339()],
        )?;
    }
    let task_planning_v7_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = 7
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !task_planning_v7_applied {
        conn.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (7, ?1, ?2)",
            params!["tool_settings_task_planning_v7", now_rfc3339()],
        )?;
    }
    let task_planning_v8_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = 8
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !task_planning_v8_applied {
        conn.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (8, ?1, ?2)",
            params!["tool_settings_task_planning_v8", now_rfc3339()],
        )?;
    }
    let task_planning_v9_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = 9
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !task_planning_v9_applied {
        conn.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (9, ?1, ?2)",
            params!["tool_settings_task_planning_v9", now_rfc3339()],
        )?;
    }
    let firecrawl_anonymous_v10_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = 10
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !firecrawl_anonymous_v10_applied {
        conn.execute(
            "UPDATE tool_web_search_sources
             SET enabled = 1
             WHERE source_id = 'firecrawl'
               AND enabled = 0
               AND NOT EXISTS (
                   SELECT 1 FROM tool_web_search_api_keys
                   WHERE source_id = 'firecrawl'
               )",
            [],
        )?;
        conn.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (10, ?1, ?2)",
            params!["tool_settings_firecrawl_anonymous_v10", now_rfc3339()],
        )?;
    }
    let task_planning_default_v11_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = 11
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !task_planning_default_v11_applied {
        conn.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (11, ?1, ?2)",
            params!["tool_settings_task_planning_default_v11", now_rfc3339()],
        )?;
    }
    if !applied {
        conn.execute(
            "UPDATE tool_settings
             SET task_planning_progress_interval = ?1
             WHERE scope_type = 'default'
               AND scope_id = 'default'
               AND task_planning_progress_interval = 3",
            [i64::from(DEFAULT_TASK_PLAN_PROGRESS_INTERVAL)],
        )?;
    }
    if applied {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO app_schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, ?3)",
        params![
            TOOL_SETTINGS_SCHEMA_VERSION,
            "tool_settings_web_search_agent_sources_v12",
            now_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn migrate_task_plans_schema(conn: &Connection) -> AppResult<()> {
    if !column_exists(conn, "task_plans", "conversation_message_id")?
        || !column_exists(conn, "task_plans", "active")?
    {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TABLE task_plans_v8 (
                planning_id TEXT PRIMARY KEY NOT NULL,
                conversation_id TEXT NOT NULL,
                conversation_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
                explanation TEXT,
                plan_json TEXT NOT NULL
                    CHECK (json_valid(plan_json) AND json_type(plan_json) = 'array'),
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
            ) WITHOUT ROWID;
            ",
        )?;
        tx.execute(
            "INSERT INTO task_plans_v8 (
                planning_id, conversation_id, conversation_message_id,
                explanation, plan_json, created_at, updated_at, active
             )
             SELECT planning_id, conversation_id, NULL, explanation, plan_json,
                    created_at, updated_at,
                    CASE
                        WHEN json_array_length(plan_json) > 0
                         AND NOT EXISTS (
                             SELECT 1 FROM json_each(task_plans.plan_json)
                             WHERE json_extract(json_each.value, '$.status') != 'completed'
                         )
                        THEN 0
                        ELSE 1
                    END
             FROM task_plans",
            [],
        )?;
        tx.execute("DROP TABLE task_plans", [])?;
        tx.execute("ALTER TABLE task_plans_v8 RENAME TO task_plans", [])?;
        tx.commit()?;
    }
    Ok(())
}

pub(super) fn ensure_task_plan_indexes(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_task_plans_one_active_conversation
             ON task_plans(conversation_id) WHERE active = 1;
         CREATE INDEX IF NOT EXISTS idx_task_plans_conversation_updated
             ON task_plans(conversation_id, updated_at DESC);",
    )?;
    Ok(())
}
