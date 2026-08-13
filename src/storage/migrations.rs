//! Database migration orchestration.
//!
//! Storage modules own their table definitions and migration details. This
//! module owns only startup ordering so the database worker stays focused on
//! connection lifecycle and query dispatch.

use rusqlite::Connection;

use crate::error::AppResult;

const QUERY_INDEXES_SCHEMA_VERSION: i64 = 12;
const TASK_PLANNING_DEFAULT_SCHEMA_VERSION: i64 = 11;
#[cfg(test)]
pub(super) const LATEST_SCHEMA_VERSION: i64 = 20;

pub(super) fn initialize(conn: &mut Connection) -> AppResult<()> {
    super::db::migrate_base_schema(conn)?;
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
