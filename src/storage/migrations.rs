//! Database migration orchestration.
//!
//! Storage modules own their table definitions and migration details. This
//! module owns only startup ordering so the database worker stays focused on
//! connection lifecycle and query dispatch.

use rusqlite::Connection;

use crate::error::AppResult;

const QUERY_INDEXES_SCHEMA_VERSION: i64 = 12;
#[cfg(test)]
pub(super) const LATEST_SCHEMA_VERSION: i64 = QUERY_INDEXES_SCHEMA_VERSION;

pub(super) fn initialize(conn: &mut Connection) -> AppResult<()> {
    super::db::migrate_base_schema(conn)?;
    super::conversation_db::migrate_conversations(conn)?;
    ensure_query_indexes(conn)?;
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
