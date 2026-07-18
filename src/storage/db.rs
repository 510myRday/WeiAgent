use std::{
    path::{Path, PathBuf},
};

use chrono::Utc;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::{
    domain::{ModelConfig, ModelProvider, ProviderType},
    error::AppResult,
};

pub const DB_FILE_NAME: &str = "weiagent.db";

pub const SCHEMA_SQL: &str = "
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS llm_requests (
    id TEXT PRIMARY KEY NOT NULL,
    created_at TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    agent_id TEXT,
    agent_name TEXT NOT NULL,
    conversation_id TEXT,
    provider_name TEXT NOT NULL,
    model_id TEXT NOT NULL,
    status_code INTEGER,
    first_token_ms INTEGER,
    duration_ms INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    cost_usd REAL,
    error TEXT,
    has_payload INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS llm_payloads (
    request_id TEXT PRIMARY KEY NOT NULL,
    raw_request TEXT NOT NULL DEFAULT '',
    raw_response TEXT,
    FOREIGN KEY (request_id) REFERENCES llm_requests(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_llm_requests_created_at_ms
    ON llm_requests(created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_llm_requests_agent_id_created
    ON llm_requests(agent_id, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_llm_requests_agent_name_created
    ON llm_requests(agent_name, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS model_providers (
    id TEXT PRIMARY KEY NOT NULL,
    provider_type TEXT NOT NULL,
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    api_key TEXT NOT NULL DEFAULT '',
    models_json TEXT NOT NULL,
    model_configs_json TEXT NOT NULL,
    extra_body_json TEXT NOT NULL DEFAULT 'null',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_FILE_NAME)
}

pub fn open_connection(db_path: &Path) -> AppResult<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

pub fn open_data_dir(data_dir: &Path) -> AppResult<Connection> {
    open_connection(&db_path(data_dir))
}

pub fn ensure_schema(data_dir: &Path) -> AppResult<()> {
    let _conn = open_data_dir(data_dir)?;
    Ok(())
}

pub fn read_providers(data_dir: &Path) -> AppResult<Vec<ModelProvider>> {
    let conn = open_data_dir(data_dir)?;
    read_providers_conn(&conn)
}

pub fn write_providers(data_dir: &Path, providers: &[ModelProvider]) -> AppResult<()> {
    let mut conn = open_data_dir(data_dir)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM model_providers", [])?;
    for provider in providers {
        insert_provider_conn(&tx, provider)?;
    }
    tx.commit()?;
    Ok(())
}

pub fn read_providers_conn(conn: &Connection) -> AppResult<Vec<ModelProvider>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_type, name, base_url, api_key, models_json,
                model_configs_json, extra_body_json, created_at, updated_at
         FROM model_providers
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ProviderRow {
            id: row.get(0)?,
            provider_type: row.get(1)?,
            name: row.get(2)?,
            base_url: row.get(3)?,
            api_key: row.get(4)?,
            models_json: row.get(5)?,
            model_configs_json: row.get(6)?,
            extra_body_json: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    })?;

    let mut providers = Vec::new();
    for row in rows {
        providers.push(row_to_provider(row?)?);
    }
    Ok(providers)
}

fn insert_provider_conn(conn: &Connection, provider: &ModelProvider) -> AppResult<()> {
    let provider_type = serde_json::to_value(&provider.provider_type)?
        .as_str()
        .unwrap_or("openai_compatible")
        .to_string();
    let models_json = serde_json::to_string(&provider.models)?;
    let model_configs_json = serde_json::to_string(&provider.model_configs)?;
    let extra_body_json = serde_json::to_string(&provider.extra_body)?;
    conn.execute(
        "INSERT INTO model_providers (
            id, provider_type, name, base_url, api_key, models_json,
            model_configs_json, extra_body_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            provider.id.to_string(),
            provider_type,
            provider.name,
            provider.base_url,
            provider.api_key,
            models_json,
            model_configs_json,
            extra_body_json,
            provider.created_at.to_rfc3339(),
            provider.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

struct ProviderRow {
    id: String,
    provider_type: String,
    name: String,
    base_url: String,
    api_key: String,
    models_json: String,
    model_configs_json: String,
    extra_body_json: String,
    created_at: String,
    updated_at: String,
}

fn row_to_provider(row: ProviderRow) -> AppResult<ModelProvider> {
    let provider_type: ProviderType =
        serde_json::from_value(serde_json::Value::String(row.provider_type))?;
    let models: Vec<String> = serde_json::from_str(&row.models_json)?;
    let model_configs: Vec<ModelConfig> = serde_json::from_str(&row.model_configs_json)?;
    let extra_body: serde_json::Value = serde_json::from_str(&row.extra_body_json)?;
    let created_at = chrono::DateTime::parse_from_rfc3339(&row.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|error| crate::error::AppError::Database(error.to_string()))?;
    let updated_at = chrono::DateTime::parse_from_rfc3339(&row.updated_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|error| crate::error::AppError::Database(error.to_string()))?;
    Ok(ModelProvider {
        id: Uuid::parse_str(&row.id)
            .map_err(|error| crate::error::AppError::Database(error.to_string()))?,
        provider_type,
        name: row.name,
        base_url: row.base_url,
        api_key: row.api_key,
        models,
        model_configs,
        extra_body,
        created_at,
        updated_at,
    })
}
