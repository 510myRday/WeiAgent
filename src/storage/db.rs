use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

const DATABASE_QUEUE_CAPACITY: usize = 256;
const DATABASE_READ_POOL_SIZE: usize = 4;

use rusqlite::{Connection, params};
use tokio::sync::Semaphore;

use super::sql_util::{column_exists, now_rfc3339};

use crate::error::{AppError, AppResult};

pub const DB_FILE_NAME: &str = "weiagent.db";
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const BASE_SCHEMA_VERSION: i64 = 1;
const PROVIDER_PROXY_SCHEMA_VERSION: i64 = 5;
const PROVIDER_KEY_STRATEGY_SCHEMA_VERSION: i64 = 6;
const PROVIDER_IMAGE_ENDPOINTS_SCHEMA_VERSION: i64 = 7;
const LLM_REQUEST_KIND_SCHEMA_VERSION: i64 = 8;
const PROVIDER_KEY_LABELS_SCHEMA_VERSION: i64 = 25;

/// Multiple API keys are stored newline-joined in the legacy `api_key` column,
/// so single-key rows written before key pools existed stay valid as-is.
type DatabaseJob = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

enum DatabaseCommand {
    Run(DatabaseJob),
    Shutdown,
}

struct DatabaseInner {
    path: PathBuf,
    sender: mpsc::SyncSender<DatabaseCommand>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    read_connections: Arc<Mutex<Vec<Connection>>>,
    read_slots: Arc<Semaphore>,
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

        let (sender, receiver) = mpsc::sync_channel::<DatabaseCommand>(DATABASE_QUEUE_CAPACITY);
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
                read_connections: Arc::new(Mutex::new(Vec::with_capacity(DATABASE_READ_POOL_SIZE))),
                read_slots: Arc::new(Semaphore::new(DATABASE_READ_POOL_SIZE)),
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
        let sender = self.inner.sender.clone();
        tokio::task::spawn_blocking(move || {
            sender.send(DatabaseCommand::Run(Box::new(move |connection| {
                let _ = result_sender.send(operation(connection));
            })))
        })
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
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

    pub async fn read<T, F>(&self, operation: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> AppResult<T> + Send + 'static,
    {
        let permit = self
            .inner
            .read_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::Database("database read pool is closed".to_string()))?;
        let path = self.inner.path.clone();
        let connections = self.inner.read_connections.clone();
        tokio::task::spawn_blocking(move || {
            let connection = connections
                .lock()
                .map_err(|_| AppError::Database("database read pool lock is poisoned".to_string()))?
                .pop()
                .map(Ok)
                .unwrap_or_else(|| open_read_connection(&path));
            let result = match connection {
                Ok(connection) => {
                    let result = operation(&connection);
                    if let Ok(mut pool) = connections.lock() {
                        pool.push(connection);
                    }
                    result
                }
                Err(error) => Err(error),
            };
            drop(permit);
            result
        })
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
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
    api_key_labels_json TEXT NOT NULL DEFAULT '[]',
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))?;
    }
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
        let has_column = column_exists(&tx, "model_providers", "proxy_url")?;
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
        let has_column = column_exists(&tx, "model_providers", "key_strategy")?;
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
    let has_image_generation_url = column_exists(conn, "model_providers", "image_generation_url")?;
    let has_image_edit_url = column_exists(conn, "model_providers", "image_edit_url")?;
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
    let has_compaction_column = column_exists(conn, "llm_requests", "is_compaction")?;
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

    let key_labels_applied = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM app_schema_migrations WHERE version = ?1
        )",
        [PROVIDER_KEY_LABELS_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    let has_key_labels_column = column_exists(conn, "model_providers", "api_key_labels_json")?;
    if !key_labels_applied || !has_key_labels_column {
        let tx = conn.transaction()?;
        if !has_key_labels_column {
            tx.execute(
                "ALTER TABLE model_providers ADD COLUMN api_key_labels_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        if !key_labels_applied {
            tx.execute(
                "INSERT INTO app_schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    PROVIDER_KEY_LABELS_SCHEMA_VERSION,
                    "model_provider_key_labels_v25",
                    now_rfc3339()
                ],
            )?;
        }
        tx.commit()?;
    }
    Ok(())
}

fn open_read_connection(db_path: &Path) -> AppResult<Connection> {
    let connection = Connection::open(db_path)?;
    connection.busy_timeout(DATABASE_BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

#[cfg(test)]
pub fn open_connection(db_path: &Path) -> AppResult<Connection> {
    open_initialized_connection(db_path)
}

#[cfg(test)]
pub fn open_data_dir(data_dir: &Path) -> AppResult<Connection> {
    open_connection(&db_path(data_dir))
}
