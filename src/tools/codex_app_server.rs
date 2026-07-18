use std::{
    sync::{
        Arc, Mutex as StdMutex, Weak,
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
const CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_EXEC_REQUEST_TIMEOUT: Duration = Duration::from_secs(60 * 60 + 30);
const APP_SERVER_IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60);
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
    activity: StdMutex<ConnectionActivityState>,
    shutdown_tx: StdMutex<Option<oneshot::Sender<()>>>,
    closed: AtomicBool,
}

#[derive(Debug, Default)]
struct ConnectionActivityState {
    active_requests: usize,
    generation: u64,
    closing: bool,
}

impl ConnectionActivityState {
    fn try_begin(&mut self) -> bool {
        if self.closing {
            return false;
        }
        self.active_requests = self.active_requests.saturating_add(1);
        self.generation = self.generation.wrapping_add(1);
        true
    }

    fn finish(&mut self) -> Option<u64> {
        self.active_requests = self.active_requests.saturating_sub(1);
        self.generation = self.generation.wrapping_add(1);
        (self.active_requests == 0 && !self.closing).then_some(self.generation)
    }

    fn idle_generation(&self) -> Option<u64> {
        (self.active_requests == 0 && !self.closing).then_some(self.generation)
    }

    fn try_mark_idle_closing(&mut self, expected_generation: u64) -> bool {
        if self.closing || self.active_requests != 0 || self.generation != expected_generation {
            return false;
        }
        self.closing = true;
        self.generation = self.generation.wrapping_add(1);
        true
    }

    fn mark_closing(&mut self) {
        self.closing = true;
        self.generation = self.generation.wrapping_add(1);
    }
}

impl CodexConnection {
    fn is_accepting_requests(&self) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        !self
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .closing
    }

    fn try_begin_activity(&self) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        activity.try_begin()
    }

    fn finish_activity(&self) -> Option<u64> {
        self.activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish()
    }

    fn idle_generation(&self) -> Option<u64> {
        self.activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .idle_generation()
    }

    fn try_mark_idle_closing(&self, expected_generation: u64) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        self.activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_mark_idle_closing(expected_generation)
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_closing();
    }

    fn request_shutdown(&self) {
        if let Some(sender) = self
            .shutdown_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = sender.send(());
        }
    }
}

struct ConnectionActivityGuard {
    server: Weak<CodexAppServer>,
    connection: Arc<CodexConnection>,
    idle_timeout: Duration,
}

impl ConnectionActivityGuard {
    fn begin(server: &Arc<CodexAppServer>, connection: Arc<CodexConnection>) -> Option<Self> {
        connection.try_begin_activity().then(|| Self {
            server: Arc::downgrade(server),
            connection,
            idle_timeout: server.idle_timeout,
        })
    }
}

impl Drop for ConnectionActivityGuard {
    fn drop(&mut self) {
        let Some(generation) = self.connection.finish_activity() else {
            return;
        };
        CodexAppServer::schedule_idle_shutdown(
            self.server.clone(),
            Arc::downgrade(&self.connection),
            self.idle_timeout,
            generation,
        );
    }
}

struct PendingRequestGuard {
    connection: Arc<CodexConnection>,
    id: u64,
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        self.connection.pending.remove(&self.id);
    }
}

pub struct CodexAppServer {
    runtime: Arc<CodexRuntimeManager>,
    connection: RwLock<Option<Arc<CodexConnection>>>,
    start_lock: Mutex<()>,
    next_request_id: AtomicU64,
    notifications: broadcast::Sender<CodexNotification>,
    idle_timeout: Duration,
}

impl CodexAppServer {
    pub fn new(runtime: Arc<CodexRuntimeManager>) -> Arc<Self> {
        Self::new_with_idle_timeout(runtime, APP_SERVER_IDLE_TIMEOUT)
    }

    fn new_with_idle_timeout(
        runtime: Arc<CodexRuntimeManager>,
        idle_timeout: Duration,
    ) -> Arc<Self> {
        let (notifications, _) = broadcast::channel(NOTIFICATION_BUFFER);
        Arc::new(Self {
            runtime,
            connection: RwLock::new(None),
            start_lock: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
            notifications,
            idle_timeout,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CodexNotification> {
        self.notifications.subscribe()
    }

    pub async fn request(self: &Arc<Self>, method: &str, params: Value) -> AppResult<Value> {
        loop {
            let connection = self.ensure_connection().await?;
            let Some(_activity) = ConnectionActivityGuard::begin(self, connection.clone()) else {
                self.clear_connection(connection.id).await;
                continue;
            };
            return match self
                .request_on_connection(&connection, method, params.clone())
                .await
            {
                Ok(value) => Ok(value),
                Err(error) => {
                    if connection.closed.load(Ordering::SeqCst) {
                        self.clear_connection(connection.id).await;
                    }
                    Err(error)
                }
            };
        }
    }

    async fn ensure_connection(self: &Arc<Self>) -> AppResult<Arc<CodexConnection>> {
        if let Some(connection) = self.connection.read().await.clone()
            && connection.is_accepting_requests()
        {
            return Ok(connection);
        }

        let _guard = self.start_lock.lock().await;
        if let Some(connection) = self.connection.read().await.clone()
            && connection.is_accepting_requests()
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
        self.runtime
            .apply_runtime_environment(&mut command, "codex_app_server")
            .await;
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
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let connection = Arc::new(CodexConnection {
            id: Uuid::new_v4(),
            stdin: Mutex::new(stdin),
            pending: DashMap::new(),
            activity: StdMutex::new(ConnectionActivityState::default()),
            shutdown_tx: StdMutex::new(Some(shutdown_tx)),
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
        spawn_child_waiter(Arc::downgrade(self), connection.clone(), child, shutdown_rx);
        *self.connection.write().await = Some(connection.clone());
        if let Some(generation) = connection.idle_generation() {
            Self::schedule_idle_shutdown(
                Arc::downgrade(self),
                Arc::downgrade(&connection),
                self.idle_timeout,
                generation,
            );
        }
        Ok(connection)
    }

    fn schedule_idle_shutdown(
        server: Weak<Self>,
        connection: Weak<CodexConnection>,
        idle_timeout: Duration,
        generation: u64,
    ) {
        tokio::spawn(async move {
            tokio::time::sleep(idle_timeout).await;
            let Some(connection) = connection.upgrade() else {
                return;
            };
            if !connection.try_mark_idle_closing(generation) {
                return;
            }
            let Some(server) = server.upgrade() else {
                connection.request_shutdown();
                return;
            };
            server.shutdown_idle_connection(connection).await;
        });
    }

    async fn shutdown_idle_connection(&self, connection: Arc<CodexConnection>) {
        let removed = {
            let mut current = self.connection.write().await;
            if current
                .as_ref()
                .is_some_and(|candidate| candidate.id == connection.id)
            {
                *current = None;
                true
            } else {
                false
            }
        };
        if removed {
            tracing::debug!(
                idle_seconds = self.idle_timeout.as_secs(),
                "stopping idle Codex app-server"
            );
        }
        // The connection may already have been replaced after its idle timer
        // fired. It is still stale and must be stopped even when it is no
        // longer stored as the current connection.
        connection.request_shutdown();
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
        let _pending_guard = PendingRequestGuard {
            connection: connection.clone(),
            id,
        };
        let message = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        write_message(connection, &message).await?;
        let response = tokio::time::timeout(request_timeout(method), receiver)
            .await
            .map_err(|_| {
                AppError::Process(format!("Codex app-server {method} request timed out"))
            })?;
        match response {
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
        connection.mark_closed();
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

fn request_timeout(method: &str) -> Duration {
    match method {
        "command/exec" => COMMAND_EXEC_REQUEST_TIMEOUT,
        "initialize" => INITIALIZE_TIMEOUT,
        _ => CONTROL_REQUEST_TIMEOUT,
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
    shutdown_rx: oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let reason = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => format!("Codex app-server exited with {status}"),
                Err(error) => format!("failed waiting for Codex app-server: {error}"),
            },
            _ = shutdown_rx => match child.kill().await {
                Ok(()) => "Codex app-server stopped after being idle".to_string(),
                Err(error) => format!("failed stopping idle Codex app-server: {error}"),
            },
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

#[cfg(test)]
mod tests {
    use super::ConnectionActivityState;

    #[test]
    fn idle_generation_is_invalidated_by_new_activity() {
        let mut state = ConnectionActivityState::default();
        let initial_generation = state.idle_generation().unwrap();

        assert!(state.try_begin());
        assert!(!state.try_mark_idle_closing(initial_generation));

        let next_generation = state.finish().unwrap();
        assert!(state.try_mark_idle_closing(next_generation));
    }

    #[test]
    fn idle_shutdown_waits_for_all_concurrent_requests() {
        let mut state = ConnectionActivityState::default();
        assert!(state.try_begin());
        assert!(state.try_begin());

        assert_eq!(state.finish(), None);
        let idle_generation = state.finish().unwrap();
        assert!(state.try_mark_idle_closing(idle_generation));
        assert!(!state.try_begin());
    }

    #[test]
    fn stale_idle_timer_cannot_close_a_reused_connection() {
        let mut state = ConnectionActivityState::default();
        assert!(state.try_begin());
        let stale_generation = state.finish().unwrap();

        assert!(state.try_begin());
        let current_generation = state.finish().unwrap();

        assert!(!state.try_mark_idle_closing(stale_generation));
        assert!(state.try_mark_idle_closing(current_generation));
    }
}
