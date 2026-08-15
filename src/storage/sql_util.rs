//! Shared SQLite value conversion helpers for the storage modules.
//!
//! Errors mention the caller-provided `field` name so corruption reports
//! point at the offending column.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub(crate) fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

pub(crate) fn i64_to_bool(value: i64, field: &str) -> AppResult<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(AppError::Database(format!(
            "invalid boolean value {value} for {field}"
        ))),
    }
}

pub(crate) fn usize_to_i64(value: usize) -> AppResult<i64> {
    i64::try_from(value)
        .map_err(|_| AppError::Database("collection position exceeds SQLite range".to_string()))
}

pub(crate) fn u64_to_i64(value: u64, field: &str) -> AppResult<i64> {
    i64::try_from(value)
        .map_err(|_| AppError::Database(format!("{field} exceeds SQLite integer range")))
}

pub(crate) fn i64_to_u64(value: i64, field: &str) -> AppResult<u64> {
    u64::try_from(value)
        .map_err(|_| AppError::Database(format!("invalid negative value for {field}")))
}

pub(crate) fn i64_to_u32(value: i64, field: &str) -> AppResult<u32> {
    u32::try_from(value)
        .map_err(|_| AppError::Database(format!("invalid integer value for {field}")))
}

pub(crate) fn i64_to_u16(value: i64, field: &str) -> AppResult<u16> {
    u16::try_from(value)
        .map_err(|_| AppError::Database(format!("invalid integer value for {field}")))
}

pub(crate) fn parse_datetime(value: &str, field: &str) -> AppResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| AppError::Database(format!("invalid {field}: {error}")))
}

/// Canonical representation for SQLite TEXT timestamp columns.
pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn parse_optional_datetime(
    value: Option<String>,
    field: &str,
) -> AppResult<Option<DateTime<Utc>>> {
    value
        .as_deref()
        .map(|value| parse_datetime(value, field))
        .transpose()
}

pub(crate) fn parse_uuid(value: &str, field: &str) -> AppResult<Uuid> {
    Uuid::parse_str(value).map_err(|error| AppError::Database(format!("invalid {field}: {error}")))
}

pub(crate) fn table_columns(conn: &rusqlite::Connection, table: &str) -> AppResult<Vec<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }
    Ok(columns)
}

pub(crate) fn column_exists(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> AppResult<bool> {
    Ok(table_columns(conn, table)?
        .iter()
        .any(|name| name == column))
}

pub(crate) fn enum_value<T: serde::Serialize>(value: &T) -> AppResult<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::Database("enum did not serialize to a string".to_string()))
}
