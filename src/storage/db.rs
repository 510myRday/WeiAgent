use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use chrono::Utc;
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::{
    domain::{IpProxy, IpProxyProtocol, IpProxyStatus, ModelConfig, ModelProvider, ProviderType},
    error::{AppError, AppResult},
};

pub const DB_FILE_NAME: &str = "weiagent.db";
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const BASE_SCHEMA_VERSION: i64 = 1;

type DatabaseJob = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

enum DatabaseCommand {
    Run(DatabaseJob),
    Shutdown,
}

struct DatabaseInner {
    path: PathBuf,
    sender: mpsc::Sender<DatabaseCommand>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for DatabaseInner {
    fn drop(&mut self) {
        let _ = self.sender.send(DatabaseCommand::Shutdown);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

impl Database {
    pub fn open(data_dir: &Path) -> AppResult<Self> {
        let path = db_path(data_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let (sender, receiver) = mpsc::channel::<DatabaseCommand>();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker_path = path.clone();
        let worker = thread::Builder::new()
            .name("weiagent-database".to_string())
            .spawn(move || {
                let mut connection = match open_initialized_connection(&worker_path) {
                    Ok(connection) => {
                        let _ = ready_sender.send(Ok(()));
                        connection
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };

                while let Ok(command) = receiver.recv() {
                    match command {
                        DatabaseCommand::Run(job) => job(&mut connection),
                        DatabaseCommand::Shutdown => break,
                    }
                }
            })
            .map_err(AppError::Storage)?;

        ready_receiver.recv().map_err(|_| {
            AppError::Database("database worker stopped during initialization".to_string())
        })??;

        Ok(Self {
            inner: Arc::new(DatabaseInner {
                path,
                sender,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub async fn call<T, F>(&self, operation: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
    {
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        self.inner
            .sender
            .send(DatabaseCommand::Run(Box::new(move |connection| {
                let _ = result_sender.send(operation(connection));
            })))
            .map_err(|_| AppError::Database("database worker is not running".to_string()))?;
        result_receiver.await.map_err(|_| {
            AppError::Database("database worker dropped a pending operation".to_string())
        })?
    }

    pub async fn shutdown(&self) -> AppResult<()> {
        self.inner
            .sender
            .send(DatabaseCommand::Shutdown)
            .map_err(|_| AppError::Database("database worker is not running".to_string()))?;
        let worker = self
            .inner
            .worker
            .lock()
            .map_err(|_| AppError::Database("database worker lock is poisoned".to_string()))?
            .take();
        if let Some(worker) = worker {
            tokio::task::spawn_blocking(move || worker.join())
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .map_err(|_| AppError::Database("database worker panicked".to_string()))?;
        }
        Ok(())
    }
}

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
    is_pinned INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'unchecked',
    latency_ms INTEGER,
    last_checked_at TEXT,
    last_error TEXT,
    ip_address TEXT,
    country TEXT,
    country_code TEXT,
    region TEXT,
    city TEXT,
    quality_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ip_proxies_enabled_status
    ON ip_proxies(enabled, status);
";

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_FILE_NAME)
}

fn configure_connection(conn: &Connection) -> AppResult<()> {
    conn.busy_timeout(DATABASE_BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

fn open_initialized_connection(db_path: &Path) -> AppResult<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open(db_path)?;
    configure_connection(&conn)?;
    migrate_base_schema(&mut conn)?;
    let data_dir = db_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    super::conversation_db::migrate_conversations(&mut conn, &data_dir)?;
    Ok(conn)
}

fn migrate_base_schema(conn: &mut Connection) -> AppResult<()> {
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
        [BASE_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        ensure_column(
            conn,
            "ip_proxies",
            "is_pinned",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            conn,
            "ip_proxies",
            "quality_json",
            "TEXT NOT NULL DEFAULT '{}'",
        )?;
        return Ok(());
    }

    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA_SQL)?;
    ensure_column(
        &tx,
        "model_providers",
        "prompt_cache_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )?;
    ensure_column(
        &tx,
        "model_providers",
        "cache_capabilities_json",
        "TEXT NOT NULL DEFAULT 'null'",
    )?;
    for (name, definition) in [
        ("ip_address", "TEXT"),
        ("country", "TEXT"),
        ("country_code", "TEXT"),
        ("region", "TEXT"),
        ("city", "TEXT"),
        ("is_pinned", "INTEGER NOT NULL DEFAULT 0"),
        ("quality_json", "TEXT NOT NULL DEFAULT '{}'"),
    ] {
        ensure_column(&tx, "ip_proxies", name, definition)?;
    }
    repair_ip_proxy_names(&tx)?;
    tx.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_ip_proxies_name_unique
         ON ip_proxies(name COLLATE NOCASE)",
        [],
    )?;
    tx.execute(
        "INSERT INTO app_schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, ?3)",
        params![
            BASE_SCHEMA_VERSION,
            "base_schema_v1",
            Utc::now().to_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> AppResult<()> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    if !columns.contains(column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub fn open_connection(db_path: &Path) -> AppResult<Connection> {
    open_initialized_connection(db_path)
}

#[cfg(test)]
pub fn open_data_dir(data_dir: &Path) -> AppResult<Connection> {
    open_connection(&db_path(data_dir))
}

/*
 * Connection-level query helpers below are intentionally synchronous. They are
 * only invoked by Database::call on the dedicated database worker thread.
 */

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

pub(super) fn upsert_provider_conn(conn: &Connection, provider: &ModelProvider) -> AppResult<()> {
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
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
            provider_type = excluded.provider_type,
            name = excluded.name,
            base_url = excluded.base_url,
            api_key = excluded.api_key,
            models_json = excluded.models_json,
            model_configs_json = excluded.model_configs_json,
            extra_body_json = excluded.extra_body_json,
            prompt_cache_json = excluded.prompt_cache_json,
            cache_capabilities_json = excluded.cache_capabilities_json,
            updated_at = excluded.updated_at",
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

pub(super) fn delete_provider_conn(conn: &Connection, id: Uuid) -> AppResult<bool> {
    Ok(conn.execute(
        "DELETE FROM model_providers WHERE id = ?1",
        [id.to_string()],
    )? > 0)
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
        "SELECT id, name, url, protocol, enabled, is_pinned, status, latency_ms,
                last_checked_at, last_error, ip_address, country, country_code,
                region, city, quality_json, created_at, updated_at
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
            is_pinned: row.get(5)?,
            status: row.get(6)?,
            latency_ms: row.get(7)?,
            last_checked_at: row.get(8)?,
            last_error: row.get(9)?,
            ip_address: row.get(10)?,
            country: row.get(11)?,
            country_code: row.get(12)?,
            region: row.get(13)?,
            city: row.get(14)?,
            quality_json: row.get(15)?,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    })?;

    let mut proxies = Vec::new();
    for row in rows {
        proxies.push(row_to_ip_proxy(row?)?);
    }
    Ok(proxies)
}

pub(super) fn upsert_ip_proxy_conn(conn: &Connection, proxy: &IpProxy) -> AppResult<()> {
    let protocol = enum_value(&proxy.protocol, "http")?;
    let status = enum_value(&proxy.status, "unchecked")?;
    let latency_ms = proxy
        .latency_ms
        .map(i64::try_from)
        .transpose()
        .map_err(|error| AppError::Database(error.to_string()))?;
    let quality_json = serde_json::to_string(&proxy.api_quality)?;
    conn.execute(
        "INSERT INTO ip_proxies (
            id, name, url, protocol, enabled, is_pinned, status, latency_ms,
            last_checked_at, last_error, ip_address, country, country_code,
            region, city, quality_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            url = excluded.url,
            protocol = excluded.protocol,
            enabled = excluded.enabled,
            is_pinned = excluded.is_pinned,
            status = excluded.status,
            latency_ms = excluded.latency_ms,
            last_checked_at = excluded.last_checked_at,
            last_error = excluded.last_error,
            ip_address = excluded.ip_address,
            country = excluded.country,
            country_code = excluded.country_code,
            region = excluded.region,
            city = excluded.city,
            quality_json = excluded.quality_json,
            updated_at = excluded.updated_at",
        params![
            proxy.id.to_string(),
            proxy.name,
            proxy.url,
            protocol,
            i64::from(proxy.enabled),
            i64::from(proxy.is_pinned),
            status,
            latency_ms,
            proxy.last_checked_at.map(|value| value.to_rfc3339()),
            proxy.last_error,
            proxy.ip_address,
            proxy.country,
            proxy.country_code,
            proxy.region,
            proxy.city,
            quality_json,
            proxy.created_at.to_rfc3339(),
            proxy.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn delete_ip_proxy_conn(conn: &Connection, id: Uuid) -> AppResult<bool> {
    Ok(conn.execute("DELETE FROM ip_proxies WHERE id = ?1", [id.to_string()])? > 0)
}

pub(super) fn upsert_ip_proxies_conn(conn: &mut Connection, proxies: &[IpProxy]) -> AppResult<()> {
    let tx = conn.transaction()?;
    for proxy in proxies {
        upsert_ip_proxy_conn(&tx, proxy)?;
    }
    tx.commit()?;
    Ok(())
}

struct IpProxyRow {
    id: String,
    name: String,
    url: String,
    protocol: String,
    enabled: i64,
    is_pinned: i64,
    status: String,
    latency_ms: Option<i64>,
    last_checked_at: Option<String>,
    last_error: Option<String>,
    ip_address: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    city: Option<String>,
    quality_json: String,
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
        is_pinned: row.is_pinned != 0,
        status,
        latency_ms,
        last_checked_at: parse_optional_datetime(row.last_checked_at)?,
        last_error: row.last_error,
        ip_address: row.ip_address,
        country: row.country,
        country_code: row.country_code,
        region: row.region,
        city: row.city,
        api_quality: serde_json::from_str(&row.quality_json).unwrap_or_default(),
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
