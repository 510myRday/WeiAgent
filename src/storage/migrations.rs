//! Database migration orchestration.
//!
//! Storage modules own their table definitions and migration details. This
//! module owns only startup ordering so the database worker stays focused on
//! connection lifecycle and query dispatch.

use rusqlite::Connection;

use crate::error::AppResult;

const QUERY_INDEXES_SCHEMA_VERSION: i64 = 12;
const TASK_PLANNING_DEFAULT_SCHEMA_VERSION: i64 = 11;
const LLM_STREAM_LOG_CLEANUP_SCHEMA_VERSION: i64 = 26;
#[cfg(test)]
pub(super) const LATEST_SCHEMA_VERSION: i64 = LLM_STREAM_LOG_CLEANUP_SCHEMA_VERSION;

pub(super) fn initialize(conn: &mut Connection) -> AppResult<()> {
    super::db::migrate_base_schema(conn)?;
    cleanup_legacy_stream_payloads(conn)?;
    super::conversation_db::migrate_conversations(conn)?;
    super::agents::migrate_agents(conn)?;
    ensure_schema_marker(
        conn,
        TASK_PLANNING_DEFAULT_SCHEMA_VERSION,
        "tool_settings_task_planning_default_v11",
    )?;
    ensure_query_indexes(conn)?;
    ensure_schema_marker(conn, 13, "skills_groups_v1")?;
    ensure_schema_marker(conn, 14, "skills_groups_v1_compat")?;
    ensure_schema_marker(conn, 15, "skills_groups_v1_ordering")?;
    ensure_schema_marker(conn, 16, "skills_groups_v1_assignments")?;
    ensure_schema_marker(conn, 17, "skills_groups_v1_metadata")?;
    ensure_schema_marker(conn, 18, "skills_groups_v1_legacy")?;
    ensure_schema_marker(conn, 19, "skills_groups_v1_runtime")?;
    ensure_schema_marker(conn, 20, "skills_groups_v1_complete")?;
    super::scheduling::migrate(conn)?;
    Ok(())
}

fn cleanup_legacy_stream_payloads(conn: &mut Connection) -> AppResult<()> {
    let applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [LLM_STREAM_LOG_CLEANUP_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }

    let tx = conn.transaction()?;
    let removed_stream_payloads = tx.execute(
        "DELETE FROM llm_payloads
         WHERE raw_response IS NOT NULL
           AND (
               raw_response GLOB 'data:*'
               OR raw_response GLOB '*\ndata:*'
               OR raw_response GLOB 'event:*'
               OR raw_response GLOB '*\nevent:*'
               OR instr(raw_response, '[DONE]') > 0
               OR instr(raw_response, 'LLM stream log middle truncated') > 0
           )",
        [],
    )?;
    tx.execute(
        "DELETE FROM llm_payloads
         WHERE raw_request = '' AND raw_response IS NULL",
        [],
    )?;
    tx.execute(
        "UPDATE llm_requests
         SET has_payload = CASE WHEN EXISTS(
             SELECT 1 FROM llm_payloads p
             WHERE p.request_id = llm_requests.id
               AND (trim(p.raw_request) <> '' OR trim(coalesce(p.raw_response, '')) <> '')
         ) THEN 1 ELSE 0 END",
        [],
    )?;
    tx.execute(
        "INSERT INTO app_schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![
            LLM_STREAM_LOG_CLEANUP_SCHEMA_VERSION,
            "llm_stream_payload_cleanup_v26",
            super::sql_util::now_rfc3339()
        ],
    )?;
    tx.commit()?;

    if removed_stream_payloads > 0 {
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    }
    Ok(())
}

fn ensure_schema_marker(conn: &Connection, version: i64, name: &str) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO app_schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![version, name, super::sql_util::now_rfc3339()],
    )?;
    Ok(())
}

fn ensure_query_indexes(conn: &mut Connection) -> AppResult<()> {
    let applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [QUERY_INDEXES_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }

    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_llm_requests_conversation_created
             ON llm_requests(conversation_id, created_at_ms DESC);
         CREATE INDEX IF NOT EXISTS idx_conversation_messages_usage_seq
             ON conversation_messages(conversation_id, seq DESC)
             WHERE usage_json IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_conversation_messages_status_seq
             ON conversation_messages(conversation_id, status, seq);
         CREATE INDEX IF NOT EXISTS idx_model_providers_name
             ON model_providers(name COLLATE NOCASE);",
    )?;
    tx.execute(
        "INSERT INTO app_schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![
            QUERY_INDEXES_SCHEMA_VERSION,
            "query_indexes_v12",
            super::sql_util::now_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(())
}
