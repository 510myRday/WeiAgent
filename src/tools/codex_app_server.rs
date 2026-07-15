use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{Mutex, RwLock, broadcast, oneshot},
};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    tools::runtime::CodexRuntimeManager,
};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const NOTIFICATION_BUFFER: usize = 4096;

#[derive(Debug, Clone)]
pub struct CodexNotification {
    pub method: String,
    pub params: Value,
}

struct CodexConnection {
    id: Uuid,
    stdin: Mutex<ChildStdin>,
    pending: DashMap<u64, oneshot::Sender<Result<Value, String>>>,
    closed: AtomicBool,
}

pub struct CodexAppServer {
    runtime: Arc<CodexRuntimeManager>,
    connection: RwLock<Option<Arc<CodexConnection>>>,
    start_lock: Mutex<()>,
    next_request_id: AtomicU64,
    notifications: broadcast::Sender<CodexNotification>,
}

impl CodexAppServer {
    pub fn new(runtime: Arc<CodexRuntimeManager>) -> Arc<Self> {
        let (notifications, _) = broadcast::channel(NOTIFICATION_BUFFER);
        Arc::new(Self {
            runtime,
            connection: RwLock::new(None),
            start_lock: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
            notifications,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CodexNotification> {
        self.notifications.subscribe()
    }

    pub async fn request(self: &Arc<Self>, method: &str, params: Value) -> AppResult<Value> {
        let connection = self.ensure_connection().await?;
        match self
            .request_on_connection(&connection, method, params)
            .await
        {
            Ok(value) => Ok(value),
            Err(error) => {
                if connection.closed.load(Ordering::SeqCst) {
                    self.clear_connection(connection.id).await;
                }
                Err(error)
            }
        }
    }

    async fn ensure_connection(self: &Arc<Self>) -> AppResult<Arc<CodexConnection>> {
        if let Some(connection) = self.connection.read().await.clone()
            && !connection.closed.load(Ordering::SeqCst)
        {
            return Ok(connection);
        }

        let _guard = self.start_lock.lock().await;
        if let Some(connection) = self.connection.read().await.clone()
            && !connection.closed.load(Ordering::SeqCst)
        {
            return Ok(connection);
        }

        let executable = self.runtime.executable().await.ok_or_else(|| {
            AppError::Process(
                "Codex runtime is unavailable. Install or refresh it in Terminal settings."
                    .to_string(),
            )
        })?;
        let app_server_home = executable.codex_home.join("app-server");
        tokio::fs::create_dir_all(&app_server_home).await?;
        let mut command = Command::new(&executable.program);
        if cfg!(windows) {
            command.args(["-c", "windows.sandbox=\"unelevated\""]);
        }
        command
            .args(["-c", "features.remote_plugin=false"])
            .args(["-c", "features.plugins=false"])
            .args(["-c", "features.apps=false"])
            .args(["-c", "features.hooks=false"])
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        crate::tools::runtime::sanitize_runtime_environment(&mut command, "codex_app_server");
        command.env("CODEX_HOME", app_server_home);

        let mut child = command.spawn().map_err(|error| {
            AppError::Process(format!(
                "failed to start Codex app-server '{}': {error}",
                executable.program.display()
            ))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AppError::Process("Codex app-server stdin is unavailable".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::Process("Codex app-server stdout is unavailable".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AppError::Process("Codex app-server stderr is unavailable".to_string())
        })?;
        let connection = Arc::new(CodexConnection {
            id: Uuid::new_v4(),
            stdin: Mutex::new(stdin),
            pending: DashMap::new(),
            closed: AtomicBool::new(false),
        });

        spawn_stdout_reader(
            Arc::downgrade(self),
            connection.clone(),
            stdout,
            self.notifications.clone(),
        );
        spawn_stderr_reader(stderr);

        let initialize = self.request_on_connection(
            &connection,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "weiagent",
                    "title": "WeiAgent",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true,
                    "optOutNotificationMethods": [
                        "remoteControl/status/changed"
                    ]
                }
            }),
        );
        tokio::time::timeout(INITIALIZE_TIMEOUT, initialize)
            .await
            .map_err(|_| {
                AppError::Process("Codex app-server initialization timed out".to_string())
            })??;
        self.notify_on_connection(&connection, "initialized", json!({}))
            .await?;
        // Keep ownership of the child locally until initialization succeeds. With kill_on_drop,
        // any timeout/protocol failure tears down the half-started server instead of leaking it.
        spawn_child_waiter(Arc::downgrade(self), connection.clone(), child);
        *self.connection.write().await = Some(connection.clone());
        Ok(connection)
    }

    async fn request_on_connection(
        &self,
        connection: &Arc<CodexConnection>,
        method: &str,
        params: Value,
    ) -> AppResult<Value> {
        if connection.closed.load(Ordering::SeqCst) {
            return Err(AppError::Process(
                "Codex app-server connection is closed".to_string(),
            ));
        }
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        connection.pending.insert(id, sender);
        let message = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = write_message(connection, &message).await {
            connection.pending.remove(&id);
            return Err(error);
        }
        match receiver.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(AppError::Process(format!(
                "Codex app-server {method} failed: {message}"
            ))),
            Err(_) => Err(AppError::Process(format!(
                "Codex app-server {method} response channel closed"
            ))),
        }
    }

    async fn notify_on_connection(
        &self,
        connection: &Arc<CodexConnection>,
        method: &str,
        params: Value,
    ) -> AppResult<()> {
        write_message(
            connection,
            &json!({
                "method": method,
                "params": params,
            }),
        )
        .await
    }

    async fn clear_connection(&self, id: Uuid) {
        let mut current = self.connection.write().await;
        if current
            .as_ref()
            .is_some_and(|connection| connection.id == id)
        {
            *current = None;
        }
    }

    async fn connection_closed(&self, connection: &Arc<CodexConnection>, reason: String) {
        if connection.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let ids = connection
            .pending
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>();
        for id in ids {
            if let Some((_, sender)) = connection.pending.remove(&id) {
                let _ = sender.send(Err(reason.clone()));
            }
        }
        self.clear_connection(connection.id).await;
    }
}

async fn write_message(connection: &CodexConnection, message: &Value) -> AppResult<()> {
    let mut serialized = serde_json::to_vec(message)?;
    serialized.push(b'\n');
    let mut stdin = connection.stdin.lock().await;
    stdin.write_all(&serialized).await.map_err(|error| {
        AppError::Process(format!("failed to write to Codex app-server: {error}"))
    })?;
    stdin.flush().await.map_err(|error| {
        AppError::Process(format!("failed to flush Codex app-server request: {error}"))
    })
}

fn spawn_stdout_reader(
    server: Weak<CodexAppServer>,
    connection: Arc<CodexConnection>,
    stdout: tokio::process::ChildStdout,
    notifications: broadcast::Sender<CodexNotification>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let message = match serde_json::from_str::<Value>(&line) {
                        Ok(message) => message,
                        Err(error) => {
                            tracing::warn!(%error, "invalid JSON from Codex app-server");
                            continue;
                        }
                    };
                    if let Some(id) = message.get("id").and_then(Value::as_u64) {
                        if let Some((_, sender)) = connection.pending.remove(&id) {
                            let response = if let Some(error) = message.get("error") {
                                Err(format_rpc_error(error))
                            } else {
                                Ok(message.get("result").cloned().unwrap_or(Value::Null))
                            };
                            let _ = sender.send(response);
                        }
                        continue;
                    }
                    if let Some(method) = message.get("method").and_then(Value::as_str) {
                        let _ = notifications.send(CodexNotification {
                            method: method.to_string(),
                            params: message.get("params").cloned().unwrap_or(Value::Null),
                        });
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "failed reading Codex app-server stdout");
                    break;
                }
            }
        }
        if let Some(server) = server.upgrade() {
            server
                .connection_closed(&connection, "Codex app-server stdout closed".to_string())
                .await;
        }
    });
}

fn spawn_stderr_reader(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(message = %line, "Codex app-server");
        }
    });
}

fn spawn_child_waiter(
    server: Weak<CodexAppServer>,
    connection: Arc<CodexConnection>,
    mut child: tokio::process::Child,
) {
    tokio::spawn(async move {
        let reason = match child.wait().await {
            Ok(status) => format!("Codex app-server exited with {status}"),
            Err(error) => format!("failed waiting for Codex app-server: {error}"),
        };
        if let Some(server) = server.upgrade() {
            server.connection_closed(&connection, reason).await;
        }
    });
}

fn format_rpc_error(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown JSON-RPC error");
    match code {
        Some(code) => format!("{message} (code {code})"),
        None => message.to_string(),
    }
}
