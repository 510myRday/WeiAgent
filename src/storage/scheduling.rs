use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::store::Store;
use crate::{
    domain::{
        ConversationHeartbeat, ConversationHeartbeatExecution, ScheduledTask,
        ScheduledTaskExecution,
    },
    error::{AppError, AppResult},
};

const HEARTBEAT_MIN_INTERVAL_SECONDS: u64 = 60;
const HEARTBEAT_MAX_INTERVAL_SECONDS: u64 = 30 * 24 * 60 * 60;

pub(super) const SCHEDULING_SCHEMA_VERSION: i64 = 24;

pub(super) fn migrate(conn: &mut Connection) -> AppResult<()> {
    let applied = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_schema_migrations WHERE version = ?1)",
        [SCHEDULING_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS scheduled_tasks (
            id TEXT PRIMARY KEY NOT NULL,
            agent_id TEXT NOT NULL,
            source_conversation_id TEXT,
            schedule_expr TEXT NOT NULL,
            timezone_offset TEXT NOT NULL,
            prompt TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
            next_run_at TEXT NOT NULL,
            last_run_at TEXT,
            last_status TEXT,
            last_error TEXT,
            active_execution_id TEXT,
            latest_execution_conversation_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(agent_id) REFERENCES agents(id) ON DELETE CASCADE,
            FOREIGN KEY(source_conversation_id) REFERENCES conversations(id) ON DELETE SET NULL
        );
        CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due
            ON scheduled_tasks(enabled, next_run_at);
        CREATE TABLE IF NOT EXISTS scheduled_task_executions (
            id TEXT PRIMARY KEY NOT NULL,
            task_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            scheduled_at TEXT NOT NULL,
            started_at TEXT,
            finished_at TEXT,
            status TEXT NOT NULL,
            error TEXT,
            trigger_type TEXT NOT NULL DEFAULT 'scheduled',
            agent_name TEXT,
            provider_name TEXT,
            model_id TEXT,
            FOREIGN KEY(task_id) REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_scheduled_task_executions_task
            ON scheduled_task_executions(task_id, scheduled_at DESC);
        CREATE TABLE IF NOT EXISTS conversation_heartbeat_executions (
            id TEXT PRIMARY KEY NOT NULL,
            conversation_id TEXT NOT NULL,
            scheduled_at TEXT NOT NULL,
            started_at TEXT,
            finished_at TEXT,
            status TEXT NOT NULL,
            error TEXT,
            trigger_type TEXT NOT NULL DEFAULT 'scheduled',
            agent_name TEXT,
            provider_name TEXT,
            model_id TEXT,
            output_preview TEXT,
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_conversation_heartbeat_executions_conversation
            ON conversation_heartbeat_executions(conversation_id, scheduled_at DESC);
        CREATE TABLE IF NOT EXISTS conversation_heartbeats (
            conversation_id TEXT PRIMARY KEY NOT NULL,
            agent_id TEXT NOT NULL,
            interval_seconds INTEGER NOT NULL,
            prompt TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
            next_fire_at TEXT NOT NULL,
            pending INTEGER NOT NULL DEFAULT 0 CHECK (pending IN (0, 1)),
            last_run_at TEXT,
            last_status TEXT,
            last_error TEXT,
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY(agent_id) REFERENCES agents(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_conversation_heartbeats_due
            ON conversation_heartbeats(enabled, next_fire_at, pending);",
    )?;
    let has_trigger_type: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scheduled_task_executions') WHERE name = 'trigger_type')",
        [],
        |row| row.get(0),
    )?;
    if !has_trigger_type {
        tx.execute(
            "ALTER TABLE scheduled_task_executions ADD COLUMN trigger_type TEXT NOT NULL DEFAULT 'scheduled'",
            [],
        )?;
    }
    for (column, definition) in [
        ("agent_name", "TEXT"),
        ("provider_name", "TEXT"),
        ("model_id", "TEXT"),
        ("output_preview", "TEXT"),
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scheduled_task_executions') WHERE name = ?1)",
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            tx.execute(
                &format!("ALTER TABLE scheduled_task_executions ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    tx.execute(
        "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
        params![21, "scheduling_v21", super::sql_util::now_rfc3339()],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
        params![22, "scheduling_v22", super::sql_util::now_rfc3339()],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at)
         VALUES (?1, ?2, ?3)",
        params![23, "scheduling_v23", super::sql_util::now_rfc3339()],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
        params![SCHEDULING_SCHEMA_VERSION, "scheduling_v24", super::sql_util::now_rfc3339()],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn validate_heartbeat_interval(seconds: u64) -> AppResult<()> {
    if !(HEARTBEAT_MIN_INTERVAL_SECONDS..=HEARTBEAT_MAX_INTERVAL_SECONDS).contains(&seconds) {
        return Err(AppError::BadRequest(format!(
            "heartbeat interval must be between {HEARTBEAT_MIN_INTERVAL_SECONDS} and {HEARTBEAT_MAX_INTERVAL_SECONDS} seconds"
        )));
    }
    Ok(())
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    let id: String = row.get("id")?;
    let agent_id: String = row.get("agent_id")?;
    let source_conversation_id: Option<String> = row.get("source_conversation_id")?;
    let active_execution_id: Option<String> = row.get("active_execution_id")?;
    let latest_execution_conversation_id: Option<String> =
        row.get("latest_execution_conversation_id")?;
    let next_run_at: String = row.get("next_run_at")?;
    let last_run_at: Option<String> = row.get("last_run_at")?;
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;
    Ok(ScheduledTask {
        id: Uuid::parse_str(&id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        agent_id: Uuid::parse_str(&agent_id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        source_conversation_id: source_conversation_id
            .and_then(|value| Uuid::parse_str(&value).ok()),
        schedule_expr: row.get("schedule_expr")?,
        timezone_offset: row.get("timezone_offset")?,
        prompt: row.get("prompt")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        next_run_at: crate::storage::sql_util::parse_datetime(&next_run_at, "next_run_at")
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        last_run_at: last_run_at
            .and_then(|value| crate::storage::sql_util::parse_datetime(&value, "last_run_at").ok()),
        last_status: row.get("last_status")?,
        last_error: row.get("last_error")?,
        active_execution_id: active_execution_id.and_then(|value| Uuid::parse_str(&value).ok()),
        latest_execution_conversation_id: latest_execution_conversation_id
            .and_then(|value| Uuid::parse_str(&value).ok()),
        created_at: crate::storage::sql_util::parse_datetime(&created_at, "created_at")
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        updated_at: crate::storage::sql_util::parse_datetime(&updated_at, "updated_at")
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn execution_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTaskExecution> {
    let id: String = row.get("id")?;
    let task_id: String = row.get("task_id")?;
    let conversation_id: String = row.get("conversation_id")?;
    let scheduled_at: String = row.get("scheduled_at")?;
    let started_at: Option<String> = row.get("started_at")?;
    let finished_at: Option<String> = row.get("finished_at")?;
    Ok(ScheduledTaskExecution {
        id: Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        task_id: Uuid::parse_str(&task_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        conversation_id: Uuid::parse_str(&conversation_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        scheduled_at: crate::storage::sql_util::parse_datetime(&scheduled_at, "scheduled_at")
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        started_at: started_at
            .and_then(|value| crate::storage::sql_util::parse_datetime(&value, "started_at").ok()),
        finished_at: finished_at
            .and_then(|value| crate::storage::sql_util::parse_datetime(&value, "finished_at").ok()),
        status: row.get("status")?,
        error: row.get("error")?,
        trigger_type: row.get("trigger_type")?,
        agent_name: row.get("agent_name")?,
        provider_name: row.get("provider_name")?,
        model_id: row.get("model_id")?,
        output_preview: row.get("output_preview")?,
    })
}

fn heartbeat_execution_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ConversationHeartbeatExecution> {
    let id: String = row.get("id")?;
    let conversation_id: String = row.get("conversation_id")?;
    let scheduled_at: String = row.get("scheduled_at")?;
    let started_at: Option<String> = row.get("started_at")?;
    let finished_at: Option<String> = row.get("finished_at")?;
    Ok(ConversationHeartbeatExecution {
        id: Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        conversation_id: Uuid::parse_str(&conversation_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        scheduled_at: crate::storage::sql_util::parse_datetime(&scheduled_at, "scheduled_at")
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        started_at: started_at
            .and_then(|value| crate::storage::sql_util::parse_datetime(&value, "started_at").ok()),
        finished_at: finished_at
            .and_then(|value| crate::storage::sql_util::parse_datetime(&value, "finished_at").ok()),
        status: row.get("status")?,
        error: row.get("error")?,
        trigger_type: row.get("trigger_type")?,
        agent_name: row.get("agent_name")?,
        provider_name: row.get("provider_name")?,
        model_id: row.get("model_id")?,
        output_preview: row.get("output_preview")?,
    })
}

fn heartbeat_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationHeartbeat> {
    let conversation_id: String = row.get("conversation_id")?;
    let agent_id: String = row.get("agent_id")?;
    let next_fire_at: String = row.get("next_fire_at")?;
    let last_run_at: Option<String> = row.get("last_run_at")?;
    Ok(ConversationHeartbeat {
        conversation_id: Uuid::parse_str(&conversation_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        agent_id: Uuid::parse_str(&agent_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        interval_seconds: row.get::<_, i64>("interval_seconds")? as u64,
        prompt: row.get("prompt")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        next_fire_at: crate::storage::sql_util::parse_datetime(&next_fire_at, "next_fire_at")
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        pending: row.get::<_, i64>("pending")? != 0,
        last_run_at: last_run_at
            .and_then(|value| crate::storage::sql_util::parse_datetime(&value, "last_run_at").ok()),
        last_status: row.get("last_status")?,
        last_error: row.get("last_error")?,
    })
}

const TASK_COLUMNS: &str = "id, agent_id, source_conversation_id, schedule_expr, timezone_offset, prompt, enabled, next_run_at, last_run_at, last_status, last_error, active_execution_id, latest_execution_conversation_id, created_at, updated_at";
const EXECUTION_COLUMNS: &str = "e.id, e.task_id, e.conversation_id, e.scheduled_at, e.started_at, e.finished_at, e.status, e.error, e.trigger_type, e.agent_name, e.provider_name, e.model_id,
    (SELECT CASE WHEN length(cm.content) > 360
        THEN substr(cm.content, 1, 180) || '...' || substr(cm.content, -180)
        ELSE cm.content END
     FROM conversation_messages cm
     WHERE cm.conversation_id = e.conversation_id AND cm.role = 'assistant'
     ORDER BY cm.seq DESC LIMIT 1) AS output_preview";
const HEARTBEAT_COLUMNS: &str = "conversation_id, agent_id, interval_seconds, prompt, enabled, next_fire_at, pending, last_run_at, last_status, last_error";
const HEARTBEAT_EXECUTION_COLUMNS: &str = "e.id, e.conversation_id, e.scheduled_at, e.started_at, e.finished_at, e.status, e.error, e.trigger_type, e.agent_name, e.provider_name, e.model_id,
    (SELECT CASE WHEN length(cm.content) > 360
        THEN substr(cm.content, 1, 180) || '...' || substr(cm.content, -180)
        ELSE cm.content END
     FROM conversation_messages cm
     WHERE cm.conversation_id = e.conversation_id AND cm.role = 'assistant'
       AND cm.created_at >= e.started_at
     ORDER BY cm.seq DESC LIMIT 1) AS output_preview";

pub(crate) fn list_tasks_conn(conn: &Connection) -> AppResult<Vec<ScheduledTask>> {
    let mut statement = conn.prepare(&format!("SELECT {TASK_COLUMNS} FROM scheduled_tasks ORDER BY enabled DESC, next_run_at ASC, created_at DESC"))?;
    let rows = statement.query_map([], task_from_row)?;
    rows.map(|row| row.map_err(AppError::from).and_then(|row| Ok(row)))
        .collect()
}

pub(crate) fn get_task_conn(conn: &Connection, task_id: Uuid) -> AppResult<ScheduledTask> {
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM scheduled_tasks WHERE id = ?1"),
        [task_id.to_string()],
        task_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

pub(crate) fn insert_task_conn(conn: &Connection, task: &ScheduledTask) -> AppResult<()> {
    conn.execute(
        "INSERT INTO scheduled_tasks (id, agent_id, source_conversation_id, schedule_expr, timezone_offset, prompt, enabled, next_run_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            task.id.to_string(), task.agent_id.to_string(), task.source_conversation_id.map(|id| id.to_string()),
            task.schedule_expr, task.timezone_offset, task.prompt, i64::from(task.enabled),
            task.next_run_at.to_rfc3339(), task.created_at.to_rfc3339(), task.updated_at.to_rfc3339()
        ],
    )?;
    Ok(())
}

pub(crate) fn update_task_conn(
    conn: &Connection,
    task_id: Uuid,
    agent_id: Option<Uuid>,
    schedule_expr: Option<String>,
    timezone_offset: Option<String>,
    prompt: Option<String>,
    next_run_at: Option<DateTime<Utc>>,
) -> AppResult<ScheduledTask> {
    let existing = get_task_conn(conn, task_id)?;
    let next = next_run_at.unwrap_or(existing.next_run_at);
    let updated_at = Utc::now();
    conn.execute(
        "UPDATE scheduled_tasks SET agent_id = ?2, schedule_expr = ?3, timezone_offset = ?4, prompt = ?5, next_run_at = ?6, updated_at = ?7 WHERE id = ?1",
        params![
            task_id.to_string(), agent_id.unwrap_or(existing.agent_id).to_string(),
            schedule_expr.unwrap_or(existing.schedule_expr), timezone_offset.unwrap_or(existing.timezone_offset),
            prompt.unwrap_or(existing.prompt), next.to_rfc3339(), updated_at.to_rfc3339()
        ],
    )?;
    get_task_conn(conn, task_id)
}

pub(crate) fn set_task_enabled_conn(
    conn: &Connection,
    task_id: Uuid,
    enabled: bool,
) -> AppResult<ScheduledTask> {
    let changed = conn.execute(
        "UPDATE scheduled_tasks SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
        params![
            task_id.to_string(),
            i64::from(enabled),
            Utc::now().to_rfc3339()
        ],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    get_task_conn(conn, task_id)
}

pub(crate) fn delete_task_conn(conn: &Connection, task_id: Uuid) -> AppResult<()> {
    let changed = conn.execute(
        "DELETE FROM scheduled_tasks WHERE id = ?1",
        [task_id.to_string()],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub(crate) fn list_executions_conn(
    conn: &Connection,
    task_id: Uuid,
) -> AppResult<Vec<ScheduledTaskExecution>> {
    let mut statement = conn.prepare(&format!("SELECT {EXECUTION_COLUMNS} FROM scheduled_task_executions e WHERE e.task_id = ?1 ORDER BY e.scheduled_at DESC"))?;
    let rows = statement.query_map([task_id.to_string()], execution_from_row)?;
    rows.map(|row| row.map_err(AppError::from).and_then(|row| Ok(row)))
        .collect()
}

pub(crate) fn list_heartbeat_executions_conn(
    conn: &Connection,
    conversation_id: Uuid,
) -> AppResult<Vec<ConversationHeartbeatExecution>> {
    let mut statement = conn.prepare(&format!("SELECT {HEARTBEAT_EXECUTION_COLUMNS} FROM conversation_heartbeat_executions e WHERE e.conversation_id = ?1 ORDER BY e.scheduled_at DESC"))?;
    let rows = statement.query_map([conversation_id.to_string()], heartbeat_execution_from_row)?;
    rows.map(|row| row.map_err(AppError::from).and_then(|row| Ok(row)))
        .collect()
}

pub(crate) fn insert_heartbeat_execution_conn(
    conn: &Connection,
    execution: &ConversationHeartbeatExecution,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO conversation_heartbeat_executions (id, conversation_id, scheduled_at, started_at, finished_at, status, error, trigger_type, agent_name, provider_name, model_id, output_preview) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![execution.id.to_string(), execution.conversation_id.to_string(), execution.scheduled_at.to_rfc3339(), execution.started_at.map(|value| value.to_rfc3339()), execution.finished_at.map(|value| value.to_rfc3339()), execution.status, execution.error, execution.trigger_type, execution.agent_name, execution.provider_name, execution.model_id, execution.output_preview],
    )?;
    Ok(())
}

pub(crate) fn finish_heartbeat_execution_conn(
    conn: &Connection,
    execution_id: Uuid,
    conversation_id: Uuid,
    status: &str,
    error: Option<String>,
) -> AppResult<()> {
    let now = Utc::now();
    conn.execute(
        "UPDATE conversation_heartbeat_executions SET finished_at = ?2, status = ?3, error = ?4 WHERE id = ?1",
        params![execution_id.to_string(), now.to_rfc3339(), status, error],
    )?;
    conn.execute(
        "UPDATE conversation_heartbeats SET last_run_at = ?2, last_status = ?3, last_error = ?4 WHERE conversation_id = ?1",
        params![conversation_id.to_string(), now.to_rfc3339(), status, error],
    )?;
    Ok(())
}

pub(crate) fn claim_task_conn(
    conn: &mut Connection,
    now: DateTime<Utc>,
    execution_id: Uuid,
) -> AppResult<Option<ScheduledTask>> {
    let tx = conn.transaction()?;
    let task_id: Option<String> = tx.query_row(
        &format!("SELECT id FROM scheduled_tasks WHERE enabled = 1 AND next_run_at <= ?1 AND active_execution_id IS NULL ORDER BY next_run_at ASC LIMIT 1"),
        [now.to_rfc3339()], |row| row.get(0)
    ).optional()?;
    let Some(task_id) = task_id else {
        tx.commit()?;
        return Ok(None);
    };
    tx.execute(
        "UPDATE scheduled_tasks SET active_execution_id = ?2, last_status = 'claimed', last_error = NULL, updated_at = ?3 WHERE id = ?1 AND active_execution_id IS NULL",
        params![task_id, execution_id.to_string(), now.to_rfc3339()],
    )?;
    let task = tx.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM scheduled_tasks WHERE id = ?1"),
        [&task_id],
        task_from_row,
    )?;
    tx.commit()?;
    Ok(Some(task))
}

pub(crate) fn claim_task_now_conn(
    conn: &mut Connection,
    task_id: Uuid,
    now: DateTime<Utc>,
    execution_id: Uuid,
) -> AppResult<Option<ScheduledTask>> {
    let tx = conn.transaction()?;
    let task_id = task_id.to_string();
    let exists: Option<String> = tx
        .query_row(
            "SELECT id FROM scheduled_tasks WHERE id = ?1 AND active_execution_id IS NULL",
            [&task_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(task_id) = exists else {
        tx.commit()?;
        return Ok(None);
    };
    tx.execute(
        "UPDATE scheduled_tasks SET active_execution_id = ?2, last_status = 'claimed', last_error = NULL, updated_at = ?3 WHERE id = ?1 AND active_execution_id IS NULL",
        params![task_id, execution_id.to_string(), now.to_rfc3339()],
    )?;
    let task = tx.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM scheduled_tasks WHERE id = ?1"),
        [&task_id],
        task_from_row,
    )?;
    tx.commit()?;
    Ok(Some(task))
}

pub(crate) fn insert_execution_conn(
    conn: &Connection,
    execution: &ScheduledTaskExecution,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO scheduled_task_executions (id, task_id, conversation_id, scheduled_at, started_at, finished_at, status, error, trigger_type, agent_name, provider_name, model_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![execution.id.to_string(), execution.task_id.to_string(), execution.conversation_id.to_string(), execution.scheduled_at.to_rfc3339(), execution.started_at.map(|value| value.to_rfc3339()), execution.finished_at.map(|value| value.to_rfc3339()), execution.status, execution.error, execution.trigger_type, execution.agent_name, execution.provider_name, execution.model_id],
    )?;
    Ok(())
}

pub(crate) fn finish_execution_conn(
    conn: &Connection,
    execution_id: Uuid,
    task_id: Uuid,
    conversation_id: Uuid,
    next_run_at: DateTime<Utc>,
    status: &str,
    error: Option<String>,
) -> AppResult<()> {
    let now = Utc::now();
    conn.execute(
        "UPDATE scheduled_task_executions SET finished_at = ?2, status = ?3, error = ?4 WHERE id = ?1",
        params![execution_id.to_string(), now.to_rfc3339(), status, error],
    )?;
    conn.execute(
        "UPDATE scheduled_tasks SET active_execution_id = NULL, last_run_at = ?2, last_status = ?3, last_error = ?4, next_run_at = ?5, latest_execution_conversation_id = ?6, updated_at = ?2 WHERE id = ?1 AND active_execution_id = ?7",
        params![task_id.to_string(), now.to_rfc3339(), status, error, next_run_at.to_rfc3339(), conversation_id.to_string(), execution_id.to_string()],
    )?;
    Ok(())
}

pub(crate) fn fail_claimed_task_conn(
    conn: &Connection,
    task_id: Uuid,
    execution_id: Uuid,
    next_run_at: DateTime<Utc>,
    error: String,
) -> AppResult<()> {
    let now = Utc::now();
    conn.execute(
        "UPDATE scheduled_tasks SET active_execution_id = NULL, last_run_at = ?2, last_status = 'failed', last_error = ?3, next_run_at = ?4, updated_at = ?2 WHERE id = ?1 AND active_execution_id = ?5",
        params![task_id.to_string(), now.to_rfc3339(), error, next_run_at.to_rfc3339(), execution_id.to_string()],
    )?;
    Ok(())
}

pub(crate) fn get_heartbeat_conn(
    conn: &Connection,
    conversation_id: Uuid,
) -> AppResult<ConversationHeartbeat> {
    conn.query_row(
        &format!(
            "SELECT {HEARTBEAT_COLUMNS} FROM conversation_heartbeats WHERE conversation_id = ?1"
        ),
        [conversation_id.to_string()],
        heartbeat_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

pub(crate) fn list_heartbeats_conn(conn: &Connection) -> AppResult<Vec<ConversationHeartbeat>> {
    let mut statement = conn.prepare(&format!("SELECT {HEARTBEAT_COLUMNS} FROM conversation_heartbeats ORDER BY enabled DESC, next_fire_at ASC"))?;
    let rows = statement.query_map([], heartbeat_from_row)?;
    rows.map(|row| row.map_err(AppError::from).and_then(|row| Ok(row)))
        .collect()
}

pub(crate) fn upsert_heartbeat_conn(
    conn: &Connection,
    heartbeat: &ConversationHeartbeat,
) -> AppResult<ConversationHeartbeat> {
    conn.execute(
        "INSERT INTO conversation_heartbeats (conversation_id, agent_id, interval_seconds, prompt, enabled, next_fire_at, pending, last_run_at, last_status, last_error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(conversation_id) DO UPDATE SET agent_id = excluded.agent_id, interval_seconds = excluded.interval_seconds, prompt = excluded.prompt, enabled = excluded.enabled, next_fire_at = excluded.next_fire_at, pending = excluded.pending, last_run_at = excluded.last_run_at, last_status = excluded.last_status, last_error = excluded.last_error",
        params![heartbeat.conversation_id.to_string(), heartbeat.agent_id.to_string(), heartbeat.interval_seconds as i64, heartbeat.prompt, i64::from(heartbeat.enabled), heartbeat.next_fire_at.to_rfc3339(), i64::from(heartbeat.pending), heartbeat.last_run_at.map(|value| value.to_rfc3339()), heartbeat.last_status, heartbeat.last_error],
    )?;
    get_heartbeat_conn(conn, heartbeat.conversation_id)
}

pub(crate) fn set_heartbeat_enabled_conn(
    conn: &Connection,
    conversation_id: Uuid,
    enabled: bool,
) -> AppResult<ConversationHeartbeat> {
    let changed = conn.execute(
        "UPDATE conversation_heartbeats SET enabled = ?2 WHERE conversation_id = ?1",
        params![conversation_id.to_string(), i64::from(enabled)],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    get_heartbeat_conn(conn, conversation_id)
}

pub(crate) fn delete_heartbeat_conn(conn: &Connection, conversation_id: Uuid) -> AppResult<()> {
    let changed = conn.execute(
        "DELETE FROM conversation_heartbeats WHERE conversation_id = ?1",
        [conversation_id.to_string()],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub(crate) fn claim_heartbeats_conn(
    conn: &Connection,
    now: DateTime<Utc>,
) -> AppResult<Vec<ConversationHeartbeat>> {
    let mut statement = conn.prepare(&format!("SELECT {HEARTBEAT_COLUMNS} FROM conversation_heartbeats WHERE enabled = 1 AND (next_fire_at <= ?1 OR pending = 1) ORDER BY next_fire_at ASC"))?;
    let rows = statement.query_map([now.to_rfc3339()], heartbeat_from_row)?;
    let mut due = Vec::new();
    for row in rows {
        let heartbeat = row?;
        conn.execute(
            "UPDATE conversation_heartbeats SET next_fire_at = ?2, pending = 0 WHERE conversation_id = ?1",
            params![heartbeat.conversation_id.to_string(), (now + chrono::Duration::seconds(heartbeat.interval_seconds as i64)).to_rfc3339()],
        )?;
        due.push(heartbeat);
    }
    Ok(due)
}

pub(crate) fn mark_heartbeat_pending_conn(
    conn: &Connection,
    conversation_id: Uuid,
) -> AppResult<()> {
    conn.execute("UPDATE conversation_heartbeats SET pending = 1, last_status = 'pending' WHERE conversation_id = ?1", [conversation_id.to_string()])?;
    Ok(())
}

impl Store {
    pub async fn list_scheduled_tasks(&self) -> AppResult<Vec<ScheduledTask>> {
        self.database.call(|conn| list_tasks_conn(conn)).await
    }

    pub async fn get_scheduled_task(&self, task_id: Uuid) -> AppResult<ScheduledTask> {
        self.database
            .call(move |conn| get_task_conn(conn, task_id))
            .await
    }

    pub async fn insert_scheduled_task(&self, task: ScheduledTask) -> AppResult<ScheduledTask> {
        self.get_agent(task.agent_id).await?;
        self.database
            .call(move |conn| {
                insert_task_conn(conn, &task)?;
                get_task_conn(conn, task.id)
            })
            .await
    }

    pub async fn update_scheduled_task(
        &self,
        task_id: Uuid,
        agent_id: Option<Uuid>,
        schedule_expr: Option<String>,
        timezone_offset: Option<String>,
        prompt: Option<String>,
        next_run_at: Option<DateTime<Utc>>,
    ) -> AppResult<ScheduledTask> {
        if let Some(agent_id) = agent_id {
            self.get_agent(agent_id).await?;
        }
        self.database
            .call(move |conn| {
                update_task_conn(
                    conn,
                    task_id,
                    agent_id,
                    schedule_expr,
                    timezone_offset,
                    prompt,
                    next_run_at,
                )
            })
            .await
    }

    pub async fn set_scheduled_task_enabled(
        &self,
        task_id: Uuid,
        enabled: bool,
    ) -> AppResult<ScheduledTask> {
        self.database
            .call(move |conn| set_task_enabled_conn(conn, task_id, enabled))
            .await
    }

    pub async fn delete_scheduled_task(&self, task_id: Uuid) -> AppResult<()> {
        self.database
            .call(move |conn| delete_task_conn(conn, task_id))
            .await
    }

    pub async fn list_scheduled_task_executions(
        &self,
        task_id: Uuid,
    ) -> AppResult<Vec<ScheduledTaskExecution>> {
        self.get_scheduled_task(task_id).await?;
        self.database
            .call(move |conn| list_executions_conn(conn, task_id))
            .await
    }

    pub async fn list_conversation_heartbeat_executions(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Vec<ConversationHeartbeatExecution>> {
        self.get_conversation_heartbeat(conversation_id).await?;
        self.database
            .call(move |conn| list_heartbeat_executions_conn(conn, conversation_id))
            .await
    }

    pub async fn insert_conversation_heartbeat_execution(
        &self,
        execution: ConversationHeartbeatExecution,
    ) -> AppResult<()> {
        self.database
            .call(move |conn| insert_heartbeat_execution_conn(conn, &execution))
            .await
    }

    pub async fn finish_conversation_heartbeat_execution(
        &self,
        execution_id: Uuid,
        conversation_id: Uuid,
        status: &str,
        error: Option<String>,
    ) -> AppResult<()> {
        let status = status.to_string();
        self.database
            .call(move |conn| {
                finish_heartbeat_execution_conn(conn, execution_id, conversation_id, &status, error)
            })
            .await
    }

    pub async fn claim_scheduled_task(
        &self,
        now: DateTime<Utc>,
        execution_id: Uuid,
    ) -> AppResult<Option<ScheduledTask>> {
        self.database
            .call(move |conn| claim_task_conn(conn, now, execution_id))
            .await
    }

    pub async fn claim_scheduled_task_now(
        &self,
        task_id: Uuid,
        now: DateTime<Utc>,
        execution_id: Uuid,
    ) -> AppResult<Option<ScheduledTask>> {
        self.database
            .call(move |conn| claim_task_now_conn(conn, task_id, now, execution_id))
            .await
    }

    pub async fn insert_scheduled_task_execution(
        &self,
        execution: ScheduledTaskExecution,
    ) -> AppResult<()> {
        self.database
            .call(move |conn| insert_execution_conn(conn, &execution))
            .await
    }

    pub async fn finish_scheduled_task_execution(
        &self,
        execution_id: Uuid,
        task_id: Uuid,
        conversation_id: Uuid,
        next_run_at: DateTime<Utc>,
        status: &str,
        error: Option<String>,
    ) -> AppResult<()> {
        let status = status.to_string();
        self.database
            .call(move |conn| {
                finish_execution_conn(
                    conn,
                    execution_id,
                    task_id,
                    conversation_id,
                    next_run_at,
                    &status,
                    error,
                )
            })
            .await
    }

    pub async fn fail_claimed_scheduled_task(
        &self,
        task_id: Uuid,
        execution_id: Uuid,
        next_run_at: DateTime<Utc>,
        error: String,
    ) -> AppResult<()> {
        self.database
            .call(move |conn| {
                fail_claimed_task_conn(conn, task_id, execution_id, next_run_at, error)
            })
            .await
    }

    pub async fn get_conversation_heartbeat(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<ConversationHeartbeat> {
        self.database
            .call(move |conn| get_heartbeat_conn(conn, conversation_id))
            .await
    }

    pub async fn list_conversation_heartbeats(&self) -> AppResult<Vec<ConversationHeartbeat>> {
        self.database.call(|conn| list_heartbeats_conn(conn)).await
    }

    pub async fn upsert_conversation_heartbeat(
        &self,
        heartbeat: ConversationHeartbeat,
    ) -> AppResult<ConversationHeartbeat> {
        self.get_conversation(heartbeat.conversation_id).await?;
        validate_heartbeat_interval(heartbeat.interval_seconds)?;
        self.database
            .call(move |conn| upsert_heartbeat_conn(conn, &heartbeat))
            .await
    }

    pub async fn set_conversation_heartbeat_enabled(
        &self,
        conversation_id: Uuid,
        enabled: bool,
    ) -> AppResult<ConversationHeartbeat> {
        self.database
            .call(move |conn| set_heartbeat_enabled_conn(conn, conversation_id, enabled))
            .await
    }

    pub async fn delete_conversation_heartbeat(&self, conversation_id: Uuid) -> AppResult<()> {
        self.database
            .call(move |conn| delete_heartbeat_conn(conn, conversation_id))
            .await
    }

    pub async fn claim_due_heartbeats(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<ConversationHeartbeat>> {
        self.database
            .call(move |conn| claim_heartbeats_conn(conn, now))
            .await
    }

    pub async fn mark_heartbeat_pending(&self, conversation_id: Uuid) -> AppResult<()> {
        self.database
            .call(move |conn| mark_heartbeat_pending_conn(conn, conversation_id))
            .await
    }
}
