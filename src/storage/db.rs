use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use chrono::Utc;
use rusqlite::{Connection, params};

use super::sql_util::{now_rfc3339, parse_datetime, parse_optional_datetime};
use uuid::Uuid;

use crate::{
    domain::{
        ApiKeyStrategy, IpProxy, IpProxyProtocol, IpProxyStatus, ModelConfig, ModelProvider,
        ProviderType,
    },
    error::{AppError, AppResult},
};

pub const DB_FILE_NAME: &str = "weiagent.db";
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const BASE_SCHEMA_VERSION: i64 = 1;
const PROVIDER_PROXY_SCHEMA_VERSION: i64 = 5;
const PROVIDER_KEY_STRATEGY_SCHEMA_VERSION: i64 = 6;
const PROVIDER_IMAGE_ENDPOINTS_SCHEMA_VERSION: i64 = 7;
const LLM_REQUEST_KIND_SCHEMA_VERSION: i64 = 8;

/// Multiple API keys are stored newline-joined in the legacy `api_key` column,
/// so single-key rows written before key pools existed stay valid as-is.
const API_KEY_SEPARATOR: char = '\n';

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
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
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
    has_payload INTEGER NOT NULL DEFAULT 0 CHECK (has_payload IN (0, 1)),
    is_compaction INTEGER NOT NULL DEFAULT 0 CHECK (is_compaction IN (0, 1))
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
    key_strategy TEXT NOT NULL DEFAULT 'failover',
    models_json TEXT NOT NULL,
    model_configs_json TEXT NOT NULL,
    extra_body_json TEXT NOT NULL DEFAULT 'null',
    prompt_cache_json TEXT NOT NULL DEFAULT '{}',
    cache_capabilities_json TEXT NOT NULL DEFAULT 'null',
    proxy_url TEXT,
    image_generation_url TEXT,
    image_edit_url TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ip_proxies (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE,
    protocol TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    is_pinned INTEGER NOT NULL DEFAULT 0 CHECK (is_pinned IN (0, 1)),
    status TEXT NOT NULL DEFAULT 'unchecked',
    latency_ms INTEGER CHECK (latency_ms IS NULL OR latency_ms >= 0),
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
CREATE UNIQUE INDEX IF NOT EXISTS idx_ip_proxies_name_unique
    ON ip_proxies(name COLLATE NOCASE);
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
    super::migrations::initialize(&mut conn)?;
    Ok(conn)
}

pub(super) fn migrate_base_schema(conn: &mut Connection) -> AppResult<()> {
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
    if !applied {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_SQL)?;
        tx.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (?1, ?2, ?3)",
            params![BASE_SCHEMA_VERSION, "base_schema_v1", now_rfc3339()],
        )?;
        tx.commit()?;
    }

    let proxy_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [PROVIDER_PROXY_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !proxy_applied {
        let tx = conn.transaction()?;
        let has_column = tx
            .prepare("PRAGMA table_info(model_providers)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "proxy_url");
        if !has_column {
            tx.execute("ALTER TABLE model_providers ADD COLUMN proxy_url TEXT", [])?;
        }
        tx.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (?1, ?2, ?3)",
            params![
                PROVIDER_PROXY_SCHEMA_VERSION,
                "model_provider_proxy_url_v5",
                now_rfc3339()
            ],
        )?;
        tx.commit()?;
    }

    let key_strategy_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [PROVIDER_KEY_STRATEGY_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !key_strategy_applied {
        let tx = conn.transaction()?;
        let has_column = tx
            .prepare("PRAGMA table_info(model_providers)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "key_strategy");
        if !has_column {
            tx.execute(
                "ALTER TABLE model_providers ADD COLUMN key_strategy TEXT NOT NULL DEFAULT 'failover'",
                [],
            )?;
        }
        tx.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (?1, ?2, ?3)",
            params![
                PROVIDER_KEY_STRATEGY_SCHEMA_VERSION,
                "model_provider_key_strategy_v6",
                now_rfc3339()
            ],
        )?;
        tx.commit()?;
    }

    let image_endpoints_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [PROVIDER_IMAGE_ENDPOINTS_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    let image_columns = conn
        .prepare("PRAGMA table_info(model_providers)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let has_image_generation_url = image_columns
        .iter()
        .any(|name| name == "image_generation_url");
    let has_image_edit_url = image_columns.iter().any(|name| name == "image_edit_url");
    if !image_endpoints_applied || !has_image_generation_url || !has_image_edit_url {
        let tx = conn.transaction()?;
        if !has_image_generation_url {
            tx.execute(
                "ALTER TABLE model_providers ADD COLUMN image_generation_url TEXT",
                [],
            )?;
        }
        if !has_image_edit_url {
            tx.execute(
                "ALTER TABLE model_providers ADD COLUMN image_edit_url TEXT",
                [],
            )?;
        }
        if !image_endpoints_applied {
            tx.execute(
                "INSERT INTO app_schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    PROVIDER_IMAGE_ENDPOINTS_SCHEMA_VERSION,
                    "model_provider_image_endpoints_v7",
                    now_rfc3339()
                ],
            )?;
        }
        tx.commit()?;
    }

    let compaction_kind_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [LLM_REQUEST_KIND_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    let compaction_columns = conn
        .prepare("PRAGMA table_info(llm_requests)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let has_compaction_column = compaction_columns
        .iter()
        .any(|name| name == "is_compaction");
    if !compaction_kind_applied || !has_compaction_column {
        let tx = conn.transaction()?;
        if !has_compaction_column {
            tx.execute(
                "ALTER TABLE llm_requests ADD COLUMN is_compaction INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_llm_requests_compaction_created
             ON llm_requests(is_compaction, created_at_ms DESC)",
            [],
        )?;
        if !compaction_kind_applied {
            tx.execute(
                "INSERT INTO app_schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    LLM_REQUEST_KIND_SCHEMA_VERSION,
                    "llm_request_kind_v8",
                    now_rfc3339()
                ],
            )?;
        }
        tx.commit()?;
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
        "SELECT id, provider_type, name, base_url, api_key, key_strategy, models_json,
                model_configs_json, extra_body_json, prompt_cache_json, cache_capabilities_json, proxy_url,
                image_generation_url, image_edit_url, created_at, updated_at
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
            key_strategy: row.get(5)?,
            models_json: row.get(6)?,
            model_configs_json: row.get(7)?,
            extra_body_json: row.get(8)?,
            prompt_cache_json: row.get(9)?,
            cache_capabilities_json: row.get(10)?,
            proxy_url: row.get(11)?,
            image_generation_url: row.get(12)?,
            image_edit_url: row.get(13)?,
            created_at: row.get(14)?,
            updated_at: row.get(15)?,
        })
    })?;

    let mut providers = Vec::new();
    for row in rows {
        providers.push(row_to_provider(row?)?);
    }
    Ok(providers)
}

pub(super) fn upsert_provider_conn(conn: &Connection, provider: &ModelProvider) -> AppResult<()> {
    let provider_type = enum_value(&provider.provider_type)?;
    let key_strategy = enum_value(&provider.key_strategy)?;
    let api_key = join_api_keys(&provider.api_keys);
    let models_json = serde_json::to_string(&provider.models)?;
    let model_configs_json = serde_json::to_string(&provider.model_configs)?;
    let extra_body_json = serde_json::to_string(&provider.extra_body)?;
    let prompt_cache_json = serde_json::to_string(&provider.prompt_cache)?;
    let cache_capabilities_json = serde_json::to_string(&provider.cache_capabilities)?;
    conn.execute(
        "INSERT INTO model_providers (
            id, provider_type, name, base_url, api_key, key_strategy, models_json,
            model_configs_json, extra_body_json, prompt_cache_json, cache_capabilities_json, proxy_url,
            image_generation_url, image_edit_url, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(id) DO UPDATE SET
            provider_type = excluded.provider_type,
            name = excluded.name,
            base_url = excluded.base_url,
            api_key = excluded.api_key,
            key_strategy = excluded.key_strategy,
            models_json = excluded.models_json,
            model_configs_json = excluded.model_configs_json,
            extra_body_json = excluded.extra_body_json,
            prompt_cache_json = excluded.prompt_cache_json,
            cache_capabilities_json = excluded.cache_capabilities_json,
            proxy_url = excluded.proxy_url,
            image_generation_url = excluded.image_generation_url,
            image_edit_url = excluded.image_edit_url,
            updated_at = excluded.updated_at",
        params![
            provider.id.to_string(),
            provider_type,
            provider.name,
            provider.base_url,
            api_key,
            key_strategy,
            models_json,
            model_configs_json,
            extra_body_json,
            prompt_cache_json,
            cache_capabilities_json,
            provider.proxy_url,
            provider.image_generation_url,
            provider.image_edit_url,
            provider.created_at.to_rfc3339(),
            provider.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn join_api_keys(api_keys: &[String]) -> String {
    api_keys
        .iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>()
        .join(&API_KEY_SEPARATOR.to_string())
}

fn split_api_keys(api_key: &str) -> Vec<String> {
    api_key
        .split(API_KEY_SEPARATOR)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
        .collect()
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
    key_strategy: String,
    models_json: String,
    model_configs_json: String,
    extra_body_json: String,
    prompt_cache_json: String,
    cache_capabilities_json: String,
    proxy_url: Option<String>,
    image_generation_url: Option<String>,
    image_edit_url: Option<String>,
    created_at: String,
    updated_at: String,
}

fn row_to_provider(row: ProviderRow) -> AppResult<ModelProvider> {
    let provider_type: ProviderType =
        serde_json::from_value(serde_json::Value::String(row.provider_type))?;
    // Tolerate unknown strategy values so a listing never bricks on bad data.
    let key_strategy: ApiKeyStrategy =
        serde_json::from_value(serde_json::Value::String(row.key_strategy)).unwrap_or_default();
    let models: Vec<String> = serde_json::from_str(&row.models_json)?;
    let model_configs: Vec<ModelConfig> = serde_json::from_str(&row.model_configs_json)?;
    let extra_body: serde_json::Value = serde_json::from_str(&row.extra_body_json)?;
    let prompt_cache = serde_json::from_str(&row.prompt_cache_json)?;
    let cache_capabilities = serde_json::from_str(&row.cache_capabilities_json)?;
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
        api_keys: split_api_keys(&row.api_key),
        key_strategy,
        models,
        model_configs,
        extra_body,
        prompt_cache,
        cache_capabilities,
        proxy_url: row.proxy_url,
        image_generation_url: row.image_generation_url,
        image_edit_url: row.image_edit_url,
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
    let protocol = enum_value(&proxy.protocol)?;
    let status = enum_value(&proxy.status)?;
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
        last_checked_at: parse_optional_datetime(row.last_checked_at, "last_checked_at")?,
        last_error: row.last_error,
        ip_address: row.ip_address,
        country: row.country,
        country_code: row.country_code,
        region: row.region,
        city: row.city,
        api_quality: serde_json::from_str(&row.quality_json).unwrap_or_default(),
        created_at: parse_datetime(&row.created_at, "created_at")?,
        updated_at: parse_datetime(&row.updated_at, "updated_at")?,
    })
}

fn enum_value<T: serde::Serialize>(value: &T) -> AppResult<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::Database("enum did not serialize to a string".to_string()))
}
