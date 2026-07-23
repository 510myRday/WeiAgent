use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use chrono::Utc;
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::{
    domain::{IpProxy, IpProxyProtocol, IpProxyStatus, ModelConfig, ModelProvider, ProviderType},
    error::{AppError, AppResult},
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
    prompt_cache_json TEXT NOT NULL DEFAULT '{}',
    cache_capabilities_json TEXT NOT NULL DEFAULT 'null',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ip_proxies (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE,
    protocol TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'unchecked',
    latency_ms INTEGER,
    last_checked_at TEXT,
    last_error TEXT,
    ip_address TEXT,
    country TEXT,
    country_code TEXT,
    region TEXT,
    city TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ip_proxies_enabled_status
    ON ip_proxies(enabled, status);
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
    // Older installations have the provider table already; add cache columns
    // without disturbing their existing rows.
    let _ = conn.execute(
        "ALTER TABLE model_providers ADD COLUMN prompt_cache_json TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE model_providers ADD COLUMN cache_capabilities_json TEXT NOT NULL DEFAULT 'null'",
        [],
    );
    for column in [
        "ip_address TEXT",
        "country TEXT",
        "country_code TEXT",
        "region TEXT",
        "city TEXT",
    ] {
        let _ = conn.execute(&format!("ALTER TABLE ip_proxies ADD COLUMN {column}"), []);
    }
    repair_ip_proxy_names(&conn)?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_ip_proxies_name_unique
         ON ip_proxies(name COLLATE NOCASE)",
        [],
    )?;
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

pub fn read_ip_proxies(data_dir: &Path) -> AppResult<Vec<IpProxy>> {
    let conn = open_data_dir(data_dir)?;
    read_ip_proxies_conn(&conn)
}

pub fn write_ip_proxies(data_dir: &Path, proxies: &[IpProxy]) -> AppResult<()> {
    let mut conn = open_data_dir(data_dir)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM ip_proxies", [])?;
    for proxy in proxies {
        insert_ip_proxy_conn(&tx, proxy)?;
    }
    tx.commit()?;
    Ok(())
}

pub fn read_providers_conn(conn: &Connection) -> AppResult<Vec<ModelProvider>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_type, name, base_url, api_key, models_json,
                model_configs_json, extra_body_json, prompt_cache_json, cache_capabilities_json, created_at, updated_at
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
            prompt_cache_json: row.get(8)?,
            cache_capabilities_json: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
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
    let prompt_cache_json = serde_json::to_string(&provider.prompt_cache)?;
    let cache_capabilities_json = serde_json::to_string(&provider.cache_capabilities)?;
    conn.execute(
        "INSERT INTO model_providers (
            id, provider_type, name, base_url, api_key, models_json,
            model_configs_json, extra_body_json, prompt_cache_json, cache_capabilities_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            provider.id.to_string(),
            provider_type,
            provider.name,
            provider.base_url,
            provider.api_key,
            models_json,
            model_configs_json,
            extra_body_json,
            prompt_cache_json,
            cache_capabilities_json,
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
    prompt_cache_json: String,
    cache_capabilities_json: String,
    created_at: String,
    updated_at: String,
}

fn row_to_provider(row: ProviderRow) -> AppResult<ModelProvider> {
    let provider_type: ProviderType =
        serde_json::from_value(serde_json::Value::String(row.provider_type))?;
    let models: Vec<String> = serde_json::from_str(&row.models_json)?;
    let model_configs: Vec<ModelConfig> = serde_json::from_str(&row.model_configs_json)?;
    let extra_body: serde_json::Value = serde_json::from_str(&row.extra_body_json)?;
    let prompt_cache = serde_json::from_str(&row.prompt_cache_json).unwrap_or_default();
    let cache_capabilities = serde_json::from_str(&row.cache_capabilities_json).unwrap_or(None);
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
        prompt_cache,
        cache_capabilities,
        created_at,
        updated_at,
    })
}

pub fn read_ip_proxies_conn(conn: &Connection) -> AppResult<Vec<IpProxy>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, url, protocol, enabled, status, latency_ms,
                last_checked_at, last_error, ip_address, country, country_code,
                region, city, created_at, updated_at
         FROM ip_proxies
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(IpProxyRow {
            id: row.get(0)?,
            name: row.get(1)?,
            url: row.get(2)?,
            protocol: row.get(3)?,
            enabled: row.get(4)?,
            status: row.get(5)?,
            latency_ms: row.get(6)?,
            last_checked_at: row.get(7)?,
            last_error: row.get(8)?,
            ip_address: row.get(9)?,
            country: row.get(10)?,
            country_code: row.get(11)?,
            region: row.get(12)?,
            city: row.get(13)?,
            created_at: row.get(14)?,
            updated_at: row.get(15)?,
        })
    })?;

    let mut proxies = Vec::new();
    for row in rows {
        proxies.push(row_to_ip_proxy(row?)?);
    }
    Ok(proxies)
}

fn insert_ip_proxy_conn(conn: &Connection, proxy: &IpProxy) -> AppResult<()> {
    let protocol = enum_value(&proxy.protocol, "http")?;
    let status = enum_value(&proxy.status, "unchecked")?;
    let latency_ms = proxy
        .latency_ms
        .map(i64::try_from)
        .transpose()
        .map_err(|error| AppError::Database(error.to_string()))?;
    conn.execute(
        "INSERT INTO ip_proxies (
            id, name, url, protocol, enabled, status, latency_ms,
            last_checked_at, last_error, ip_address, country, country_code,
            region, city, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            proxy.id.to_string(),
            proxy.name,
            proxy.url,
            protocol,
            i64::from(proxy.enabled),
            status,
            latency_ms,
            proxy.last_checked_at.map(|value| value.to_rfc3339()),
            proxy.last_error,
            proxy.ip_address,
            proxy.country,
            proxy.country_code,
            proxy.region,
            proxy.city,
            proxy.created_at.to_rfc3339(),
            proxy.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

struct IpProxyRow {
    id: String,
    name: String,
    url: String,
    protocol: String,
    enabled: i64,
    status: String,
    latency_ms: Option<i64>,
    last_checked_at: Option<String>,
    last_error: Option<String>,
    ip_address: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    city: Option<String>,
    created_at: String,
    updated_at: String,
}

fn row_to_ip_proxy(row: IpProxyRow) -> AppResult<IpProxy> {
    let protocol: IpProxyProtocol =
        serde_json::from_value(serde_json::Value::String(row.protocol))?;
    let status: IpProxyStatus = serde_json::from_value(serde_json::Value::String(row.status))?;
    let latency_ms = row
        .latency_ms
        .map(u64::try_from)
        .transpose()
        .map_err(|error| AppError::Database(error.to_string()))?;
    Ok(IpProxy {
        id: Uuid::parse_str(&row.id).map_err(|error| AppError::Database(error.to_string()))?,
        name: row.name,
        url: row.url,
        protocol,
        enabled: row.enabled != 0,
        status,
        latency_ms,
        last_checked_at: parse_optional_datetime(row.last_checked_at)?,
        last_error: row.last_error,
        ip_address: row.ip_address,
        country: row.country,
        country_code: row.country_code,
        region: row.region,
        city: row.city,
        created_at: parse_datetime(&row.created_at)?,
        updated_at: parse_datetime(&row.updated_at)?,
    })
}

fn enum_value<T: serde::Serialize>(value: &T, fallback: &str) -> AppResult<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or(fallback)
        .to_string())
}

fn parse_datetime(value: &str) -> AppResult<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| AppError::Database(error.to_string()))
}

fn parse_optional_datetime(value: Option<String>) -> AppResult<Option<chrono::DateTime<Utc>>> {
    value.as_deref().map(parse_datetime).transpose()
}

fn repair_ip_proxy_names(conn: &Connection) -> AppResult<()> {
    let rows = {
        let mut statement =
            conn.prepare("SELECT id, name FROM ip_proxies ORDER BY created_at ASC, id ASC")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut used = HashSet::new();

    for (id, original_name) in rows {
        let base_name = if original_name.trim().is_empty() {
            "proxy".to_string()
        } else {
            original_name.trim().to_string()
        };
        let mut name = base_name.clone();
        let mut index = 1_u64;
        while !used.insert(name.to_lowercase()) {
            name = format!("{base_name}_{index}");
            index += 1;
        }
        if name != original_name {
            conn.execute(
                "UPDATE ip_proxies SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![name, Utc::now().to_rfc3339(), id],
            )?;
        }
    }
    Ok(())
}
