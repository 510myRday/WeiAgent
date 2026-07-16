use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, mpsc};
use uuid::Uuid;

use crate::{
    domain::{ProcessInfo, ProcessStatus, TerminalPermissionMode, TerminalToolConfig, ToolResult},
    error::{AppError, AppResult},
    tools::{
        codex_app_server::{CodexAppServer, CodexNotification},
        runtime::{CodexExecutable, CodexRuntimeManager},
    },
};

pub const MAX_TERMINAL_TIMEOUT_MS: u64 = 120_000;
pub const MAX_SESSION_WAIT_MS: u64 = 30_000;
const MAX_OUTPUT_BYTES: usize = 1_000_000;
const MAX_LLM_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const FINISHED_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_RUNNING_PROCESSES: usize = 32;
const MAX_RUNNING_PROCESSES_PER_AGENT: usize = 8;
const MAX_PROCESS_LIFETIME_MS: u64 = 60 * 60 * 1_000;
const MAX_RETAINED_PROCESSES: usize = 256;
const OUTPUT_CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPermissions {
    #[default]
    UseDefault,
    RequireEscalated,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSessionAction {
    Poll,
    Write,
    Submit,
    Interrupt,
    Eof,
    Terminate,
    Resize,
}

pub struct ExecuteTerminalRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub background: Option<bool>,
    pub tty: Option<bool>,
    pub sandbox_permissions: SandboxPermissions,
    pub approved: bool,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalAccess {
    Sandbox,
    FullAccess,
}

#[derive(Debug, Clone)]
struct BackendExecRequest {
    process_id: String,
    command: String,
    workspace_root: PathBuf,
    cwd: PathBuf,
    access: TerminalAccess,
    tty: bool,
}

#[derive(Debug)]
struct BackendOutput {
    stream: OutputStream,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputStream {
    Stdout,
    Stderr,
}

#[async_trait]
trait TerminalBackend: Send + Sync {
    async fn execute(
        &self,
        request: BackendExecRequest,
        output: mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32>;
    async fn write(&self, process_id: &str, data: &[u8], close_stdin: bool) -> AppResult<()>;
    async fn terminate(&self, process_id: &str) -> AppResult<()>;
    async fn resize(&self, process_id: &str, rows: u16, cols: u16) -> AppResult<()>;
}

struct CodexTerminalBackend {
    runtime: Arc<CodexRuntimeManager>,
    app_server: Arc<CodexAppServer>,
    execution_paths: DashMap<String, CodexExecutionPath>,
    sandbox_processes: DashMap<String, Arc<SandboxProcess>>,
}

struct SandboxProcess {
    pid: u32,
}

#[derive(Clone, Copy)]
enum CodexExecutionPath {
    AppServer,
    WindowsSandboxCli,
}

impl CodexTerminalBackend {
    fn new(runtime: Arc<CodexRuntimeManager>) -> Self {
        Self {
            app_server: CodexAppServer::new(runtime.clone()),
            runtime,
            execution_paths: DashMap::new(),
            sandbox_processes: DashMap::new(),
        }
    }

    async fn execute_command_exec(
        &self,
        request: &BackendExecRequest,
        output: &mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        let (command, sandbox_policy, tty) = build_codex_exec(request);
        let runtime_label = match request.access {
            TerminalAccess::Sandbox => "codex_sandbox",
            TerminalAccess::FullAccess => "codex_full_access",
        };
        let mut notifications = self.app_server.subscribe();
        let call = self.app_server.request(
            "command/exec",
            json!({
                "command": command,
                "processId": request.process_id,
                "tty": tty,
                "streamStdin": true,
                "streamStdoutStderr": true,
                "disableOutputCap": true,
                "timeoutMs": MAX_PROCESS_LIFETIME_MS,
                "cwd": os_command_path(&request.cwd),
                "env": crate::tools::runtime::sanitized_environment_overrides(runtime_label),
                "size": tty.then_some(json!({ "rows": 24, "cols": 120 })),
                "sandboxPolicy": sandbox_policy,
            }),
        );
        tokio::pin!(call);
        loop {
            tokio::select! {
                result = &mut call => {
                    let result = result?;
                    drain_codex_output(&mut notifications, &request.process_id, output).await?;
                    let exit_code = result
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| AppError::Process(
                            "Codex command/exec response did not contain exitCode".to_string()
                        ))?;
                    return Ok(exit_code as i32);
                }
                notification = notifications.recv() => {
                    match notification {
                        Ok(notification) => {
                            forward_codex_output(notification, &request.process_id, output).await?;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            report_missed_output(output, count).await?;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err(AppError::Process(
                                "Codex app-server notification channel closed".to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    async fn execute_windows_sandbox(
        &self,
        request: &BackendExecRequest,
        output: &mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        use tokio::io::AsyncReadExt;

        let executable = self.runtime.executable().await.ok_or_else(|| {
            AppError::Process(
                "Codex runtime is unavailable. Install or refresh it in Terminal settings."
                    .to_string(),
            )
        })?;
        let argv = build_windows_sandbox_argv(request, &executable);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| AppError::Process("Codex sandbox command was empty".to_string()))?;
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .current_dir(&request.workspace_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        crate::tools::runtime::sanitize_runtime_environment(&mut command, "codex_sandbox");
        let mut child = command.spawn().map_err(|error| {
            AppError::Process(format!("failed to start Codex sandbox: {error}"))
        })?;
        let pid = child
            .id()
            .ok_or_else(|| AppError::Process("Codex sandbox process has no pid".to_string()))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        self.sandbox_processes
            .insert(request.process_id.clone(), Arc::new(SandboxProcess { pid }));
        let stdout_task = stdout.map(|mut stdout| {
            let output = output.clone();
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                loop {
                    match stdout.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if output
                                .send(BackendOutput {
                                    stream: OutputStream::Stdout,
                                    data: buffer[..read].to_vec(),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
        });
        let stderr_task = stderr.map(|mut stderr| {
            let output = output.clone();
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                loop {
                    match stderr.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if output
                                .send(BackendOutput {
                                    stream: OutputStream::Stderr,
                                    data: buffer[..read].to_vec(),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
        });
        let status = match tokio::time::timeout(
            Duration::from_millis(MAX_PROCESS_LIFETIME_MS),
            child.wait(),
        )
        .await
        {
            Ok(status) => status?,
            Err(_) => {
                let _ = tokio::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output()
                    .await;
                let _ = child.wait().await;
                self.sandbox_processes.remove(&request.process_id);
                return Err(AppError::Process(
                    "Codex sandbox process exceeded one hour".to_string(),
                ));
            }
        };
        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }
        self.sandbox_processes.remove(&request.process_id);
        Ok(status.code().unwrap_or(-1))
    }

    async fn sandbox_process(&self, process_id: &str) -> Option<Arc<SandboxProcess>> {
        for _ in 0..20 {
            if let Some(process) = self
                .sandbox_processes
                .get(process_id)
                .map(|entry| entry.value().clone())
            {
                return Some(process);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }

    async fn execution_path(&self, process_id: &str) -> Option<CodexExecutionPath> {
        for _ in 0..20 {
            if let Some(path) = self
                .execution_paths
                .get(process_id)
                .map(|entry| *entry.value())
            {
                return Some(path);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }

    async fn request_process_control(&self, method: &str, params: Value) -> AppResult<()> {
        self.app_server.request(method, params).await.map(|_| ())
    }
}

#[async_trait]
impl TerminalBackend for CodexTerminalBackend {
    async fn execute(
        &self,
        request: BackendExecRequest,
        output: mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        let path = if cfg!(windows) && request.access == TerminalAccess::Sandbox {
            CodexExecutionPath::WindowsSandboxCli
        } else {
            CodexExecutionPath::AppServer
        };
        self.execution_paths
            .insert(request.process_id.clone(), path);
        let result = match path {
            CodexExecutionPath::AppServer => self.execute_command_exec(&request, &output).await,
            CodexExecutionPath::WindowsSandboxCli => {
                self.execute_windows_sandbox(&request, &output).await
            }
        };
        self.execution_paths.remove(&request.process_id);
        result
    }

    async fn write(&self, process_id: &str, data: &[u8], close_stdin: bool) -> AppResult<()> {
        match self.execution_path(process_id).await {
            Some(CodexExecutionPath::WindowsSandboxCli) => Err(AppError::BadRequest(
                "Codex Windows sandbox commands are non-interactive; stdin continuation requires an approved full-access session"
                    .to_string(),
            )),
            _ => {
                self.request_process_control(
                    "command/exec/write",
                    json!({
                        "processId": process_id,
                        "deltaBase64": (!data.is_empty()).then(|| BASE64_STANDARD.encode(data)),
                        "closeStdin": close_stdin,
                    }),
                )
                .await
            }
        }
    }

    async fn terminate(&self, process_id: &str) -> AppResult<()> {
        match self.execution_path(process_id).await {
            Some(CodexExecutionPath::WindowsSandboxCli) => {
                let process = self.sandbox_process(process_id).await.ok_or_else(|| {
                    AppError::Process(format!(
                        "Codex sandbox process '{process_id}' was not found"
                    ))
                })?;
                tokio::process::Command::new("taskkill")
                    .args(["/PID", &process.pid.to_string(), "/T", "/F"])
                    .output()
                    .await?;
                Ok(())
            }
            _ => {
                self.request_process_control(
                    "command/exec/terminate",
                    json!({ "processId": process_id }),
                )
                .await
            }
        }
    }

    async fn resize(&self, process_id: &str, rows: u16, cols: u16) -> AppResult<()> {
        match self.execution_path(process_id).await {
            Some(CodexExecutionPath::WindowsSandboxCli) => Err(AppError::BadRequest(
                "Codex Windows sandbox sessions do not support PTY resize".to_string(),
            )),
            _ => {
                self.request_process_control(
                    "command/exec/resize",
                    json!({
                        "processId": process_id,
                        "size": { "rows": rows, "cols": cols },
                    }),
                )
                .await
            }
        }
    }
}

async fn report_missed_output(output: &mpsc::Sender<BackendOutput>, count: u64) -> AppResult<()> {
    output
        .send(BackendOutput {
            stream: OutputStream::Stderr,
            data: format!("\n[WeiAgent missed {count} Codex output events]\n").into_bytes(),
        })
        .await
        .map_err(|_| AppError::Process("terminal output receiver closed".to_string()))
}

async fn drain_codex_output(
    notifications: &mut tokio::sync::broadcast::Receiver<CodexNotification>,
    process_id: &str,
    output: &mpsc::Sender<BackendOutput>,
) -> AppResult<()> {
    loop {
        match notifications.try_recv() {
            Ok(notification) => forward_codex_output(notification, process_id, output).await?,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(count)) => {
                report_missed_output(output, count).await?;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return Ok(()),
        }
    }
}

async fn forward_codex_output(
    notification: CodexNotification,
    process_id: &str,
    output: &mpsc::Sender<BackendOutput>,
) -> AppResult<()> {
    if notification.method != "command/exec/outputDelta"
        || notification.params["processId"].as_str() != Some(process_id)
    {
        return Ok(());
    }
    forward_encoded_output(&notification.params, output).await
}

async fn forward_encoded_output(
    params: &Value,
    output: &mpsc::Sender<BackendOutput>,
) -> AppResult<()> {
    let Some(encoded) = params["deltaBase64"].as_str() else {
        return Ok(());
    };
    let data = BASE64_STANDARD.decode(encoded).map_err(|error| {
        AppError::Process(format!("invalid output from Codex process: {error}"))
    })?;
    let stream = match params["stream"].as_str() {
        Some("stderr") => OutputStream::Stderr,
        _ => OutputStream::Stdout,
    };
    output
        .send(BackendOutput { stream, data })
        .await
        .map_err(|_| AppError::Process("terminal output receiver closed".to_string()))
}

fn build_codex_exec(request: &BackendExecRequest) -> (Vec<String>, Value, bool) {
    let policy = match request.access {
        TerminalAccess::Sandbox => json!({
            "type": "workspaceWrite",
            "writableRoots": [request.workspace_root],
            "networkAccess": false,
        }),
        TerminalAccess::FullAccess => json!({ "type": "dangerFullAccess" }),
    };
    (
        build_codex_shell_argv(&request.command, &request.cwd),
        policy,
        request.tty,
    )
}

fn build_windows_sandbox_argv(
    request: &BackendExecRequest,
    executable: &CodexExecutable,
) -> Vec<String> {
    let mut command = vec![executable.program.to_string_lossy().to_string()];
    command.extend([
        "-c".to_string(),
        "windows.sandbox=\"unelevated\"".to_string(),
        "-c".to_string(),
        "features.remote_plugin=false".to_string(),
        "-c".to_string(),
        "features.plugins=false".to_string(),
        "-c".to_string(),
        "features.apps=false".to_string(),
        "-c".to_string(),
        "features.hooks=false".to_string(),
        "sandbox".to_string(),
        "--permissions-profile".to_string(),
        ":workspace".to_string(),
        "-C".to_string(),
        os_command_path(&request.workspace_root),
    ]);
    command.extend(build_codex_shell_argv(&request.command, &request.cwd));
    command
}

/// Builds the host-shell argv passed to Codex. Production code never spawns this argv directly;
/// Codex app-server or `codex sandbox` owns the actual shell process and its permissions.
fn build_codex_shell_argv(shell_command: &str, cwd: &Path) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "powershell.exe".to_string(),
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            windows_powershell_cmd_script(shell_command, Some(cwd)),
        ]
    } else {
        vec![
            "sh".to_string(),
            "-lc".to_string(),
            format!("cd -- {} && {}", quote_unix_path(cwd), shell_command),
        ]
    }
}

pub struct TerminalManager {
    backend: Arc<dyn TerminalBackend>,
    processes: DashMap<String, Arc<ProcessRecord>>,
    spawn_lock: Mutex<()>,
}

struct ProcessMetadata {
    agent_id: Uuid,
    conversation_id: Uuid,
    tool_call_id: String,
    command: String,
    cwd: String,
    runtime: String,
    tty: bool,
    detached: bool,
}

struct ProcessState {
    status: ProcessStatus,
    exit_code: Option<i32>,
    finished_at: Option<DateTime<Utc>>,
    duration_ms: Option<u64>,
}

struct ProcessRecord {
    process_id: String,
    metadata: ProcessMetadata,
    output: Mutex<OutputBuffer>,
    state: Mutex<ProcessState>,
    started_at: DateTime<Utc>,
    started_instant: Instant,
    detached: AtomicBool,
    stop_requested: AtomicBool,
    changed: Notify,
}

struct OutputChunk {
    start: u64,
    end: u64,
    stream: OutputStream,
    data: Vec<u8>,
}

#[derive(Default)]
struct OutputBuffer {
    chunks: VecDeque<OutputChunk>,
    next_cursor: u64,
    bytes: usize,
}

struct OutputRead {
    stdout: String,
    stderr: String,
    cursor: u64,
}

impl OutputBuffer {
    fn append(&mut self, stream: OutputStream, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let start = self.next_cursor;
        self.next_cursor = self.next_cursor.saturating_add(data.len() as u64);
        self.bytes = self.bytes.saturating_add(data.len());
        self.chunks.push_back(OutputChunk {
            start,
            end: self.next_cursor,
            stream,
            data,
        });
        while self.bytes > MAX_OUTPUT_BYTES {
            let Some(mut front) = self.chunks.pop_front() else {
                break;
            };
            let excess = self.bytes - MAX_OUTPUT_BYTES;
            if excess >= front.data.len() {
                self.bytes -= front.data.len();
                continue;
            }
            front.data.drain(..excess);
            front.start = front.start.saturating_add(excess as u64);
            self.bytes -= excess;
            self.chunks.push_front(front);
        }
    }

    fn read_since(&self, cursor: u64) -> OutputRead {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for chunk in &self.chunks {
            if chunk.end <= cursor {
                continue;
            }
            let skip = cursor.saturating_sub(chunk.start) as usize;
            let data = &chunk.data[skip.min(chunk.data.len())..];
            match chunk.stream {
                OutputStream::Stdout => stdout.extend_from_slice(data),
                OutputStream::Stderr => stderr.extend_from_slice(data),
            }
        }
        OutputRead {
            stdout: decode_process_output(&stdout),
            stderr: decode_process_output(&stderr),
            cursor: self.next_cursor,
        }
    }

    fn read_all(&self) -> OutputRead {
        let cursor = self.chunks.front().map(|chunk| chunk.start).unwrap_or(0);
        self.read_since(cursor)
    }
}

impl TerminalManager {
    pub fn new(runtime: Arc<CodexRuntimeManager>) -> Arc<Self> {
        Arc::new(Self {
            backend: Arc::new(CodexTerminalBackend::new(runtime)),
            processes: DashMap::new(),
            spawn_lock: Mutex::new(()),
        })
    }

    #[cfg(test)]
    pub fn new_for_tests() -> Arc<Self> {
        Arc::new(Self {
            backend: Arc::new(TestTerminalBackend::default()),
            processes: DashMap::new(),
            spawn_lock: Mutex::new(()),
        })
    }

    async fn spawn(
        self: &Arc<Self>,
        request: BackendExecRequest,
        metadata: ProcessMetadata,
    ) -> AppResult<String> {
        let _guard = self.spawn_lock.lock().await;
        self.cleanup_finished().await;
        if self.running_count(None).await >= MAX_RUNNING_PROCESSES {
            return Err(AppError::Process(format!(
                "global running process limit ({MAX_RUNNING_PROCESSES}) reached"
            )));
        }
        if self.running_count(Some(metadata.agent_id)).await >= MAX_RUNNING_PROCESSES_PER_AGENT {
            return Err(AppError::Process(format!(
                "agent running process limit ({MAX_RUNNING_PROCESSES_PER_AGENT}) reached"
            )));
        }

        let process_id = request.process_id.clone();
        let detached = metadata.detached;
        let record = Arc::new(ProcessRecord {
            process_id: process_id.clone(),
            metadata,
            output: Mutex::new(OutputBuffer::default()),
            state: Mutex::new(ProcessState {
                status: ProcessStatus::Running,
                exit_code: None,
                finished_at: None,
                duration_ms: None,
            }),
            started_at: Utc::now(),
            started_instant: Instant::now(),
            detached: AtomicBool::new(detached),
            stop_requested: AtomicBool::new(false),
            changed: Notify::new(),
        });
        self.processes.insert(process_id.clone(), record.clone());

        let backend = self.backend.clone();
        let (output_tx, mut output_rx) = mpsc::channel::<BackendOutput>(OUTPUT_CHANNEL_CAPACITY);
        let output_record = record.clone();
        let output_task = tokio::spawn(async move {
            while let Some(output) = output_rx.recv().await {
                output_record
                    .output
                    .lock()
                    .await
                    .append(output.stream, output.data);
                output_record.changed.notify_waiters();
            }
        });

        tokio::spawn(async move {
            let outcome = backend.execute(request, output_tx).await;
            // The backend can finish immediately after sending its final output chunk. Wait for
            // the receiver to drain before publishing the terminal state, otherwise foreground
            // callers may observe Completed and remove the record before the tail is captured.
            let _ = output_task.await;
            let mut state = record.state.lock().await;
            state.finished_at = Some(Utc::now());
            state.duration_ms = Some(record.started_instant.elapsed().as_millis() as u64);
            match outcome {
                Ok(exit_code) => {
                    state.exit_code = Some(exit_code);
                    state.status = if record.stop_requested.load(Ordering::SeqCst) {
                        ProcessStatus::Stopped
                    } else if exit_code == 0 {
                        ProcessStatus::Completed
                    } else {
                        ProcessStatus::Failed
                    };
                }
                Err(error) => {
                    state.status = if record.stop_requested.load(Ordering::SeqCst) {
                        ProcessStatus::Stopped
                    } else {
                        ProcessStatus::Failed
                    };
                    drop(state);
                    record.output.lock().await.append(
                        OutputStream::Stderr,
                        format!("\nTerminal backend failed: {error}\n").into_bytes(),
                    );
                    record.changed.notify_waiters();
                    return;
                }
            }
            drop(state);
            record.changed.notify_waiters();
        });
        Ok(process_id)
    }

    pub async fn get(&self, id: &str) -> Option<ProcessInfo> {
        let record = self.record(id)?;
        Some(snapshot(&record).await)
    }

    pub async fn list(&self, agent_id: Option<Uuid>) -> Vec<ProcessInfo> {
        self.cleanup_finished().await;
        let records = self
            .processes
            .iter()
            .filter(|entry| {
                entry.detached.load(Ordering::SeqCst)
                    && agent_id.is_none_or(|id| entry.metadata.agent_id == id)
            })
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        let mut result = Vec::with_capacity(records.len());
        for record in records {
            result.push(snapshot(&record).await);
        }
        result.sort_by_key(|item| std::cmp::Reverse(item.started_at));
        result
    }

    pub fn promote_to_background(&self, id: &str) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        record.detached.store(true, Ordering::SeqCst);
        true
    }

    pub fn remove(&self, id: &str) {
        self.processes.remove(id);
    }

    pub async fn read_since(
        &self,
        id: &str,
        cursor: u64,
        wait: Duration,
    ) -> Option<(ProcessInfo, String, String, u64)> {
        let record = self.record(id)?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = record.changed.notified();
            let output = record.output.lock().await.read_since(cursor);
            let info = snapshot(&record).await;
            if output.cursor > cursor
                || info.status != ProcessStatus::Running
                || tokio::time::Instant::now() >= deadline
            {
                return Some((info, output.stdout, output.stderr, output.cursor));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let output = record.output.lock().await.read_since(cursor);
                return Some((
                    snapshot(&record).await,
                    output.stdout,
                    output.stderr,
                    output.cursor,
                ));
            }
        }
    }

    pub async fn wait_for_completion(&self, id: &str, timeout: Duration) -> Option<ProcessInfo> {
        let record = self.record(id)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = record.changed.notified();
            let current = snapshot(&record).await;
            if current.status != ProcessStatus::Running {
                return Some(current);
            }
            if tokio::time::Instant::now() >= deadline {
                return Some(current);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Some(snapshot(&record).await);
            }
        }
    }

    pub async fn write(&self, id: &str, data: &[u8], close_stdin: bool) -> AppResult<ProcessInfo> {
        let record = self
            .record(id)
            .ok_or_else(|| AppError::Process(format!("terminal process '{id}' was not found")))?;
        if record.state.lock().await.status != ProcessStatus::Running {
            return Ok(snapshot(&record).await);
        }
        self.backend.write(id, data, close_stdin).await?;
        Ok(snapshot(&record).await)
    }

    pub async fn interrupt(&self, id: &str) -> AppResult<ProcessInfo> {
        let record = self
            .record(id)
            .ok_or_else(|| AppError::Process(format!("terminal process '{id}' was not found")))?;
        if record.state.lock().await.status != ProcessStatus::Running {
            return Ok(snapshot(&record).await);
        }
        if record.metadata.tty {
            self.backend.write(id, &[3], false).await?;
        } else {
            record.stop_requested.store(true, Ordering::SeqCst);
            self.backend.terminate(id).await?;
        }
        Ok(snapshot(&record).await)
    }

    pub async fn terminate(&self, id: &str) -> AppResult<Option<ProcessInfo>> {
        let Some(record) = self.record(id) else {
            return Ok(None);
        };
        if record.state.lock().await.status != ProcessStatus::Running {
            return Ok(Some(snapshot(&record).await));
        }
        record.stop_requested.store(true, Ordering::SeqCst);
        self.backend.terminate(id).await?;
        Ok(self.wait_for_completion(id, Duration::from_secs(5)).await)
    }

    pub async fn resize(&self, id: &str, rows: u16, cols: u16) -> AppResult<ProcessInfo> {
        let record = self
            .record(id)
            .ok_or_else(|| AppError::Process(format!("terminal process '{id}' was not found")))?;
        if !record.metadata.tty {
            return Err(AppError::BadRequest(
                "only PTY terminal sessions can be resized".to_string(),
            ));
        }
        self.backend.resize(id, rows, cols).await?;
        Ok(snapshot(&record).await)
    }

    pub async fn stop_for_conversation(&self, conversation_id: Uuid) {
        let ids = self
            .processes
            .iter()
            .filter(|entry| {
                entry.metadata.conversation_id == conversation_id
                    && !entry.detached.load(Ordering::SeqCst)
            })
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        self.terminate_all(ids).await;
    }

    pub async fn stop_all_for_conversation(&self, conversation_id: Uuid) {
        let ids = self
            .processes
            .iter()
            .filter(|entry| entry.metadata.conversation_id == conversation_id)
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        self.terminate_all(ids).await;
    }

    pub async fn stop_for_agent(&self, agent_id: Uuid) {
        let ids = self
            .processes
            .iter()
            .filter(|entry| entry.metadata.agent_id == agent_id)
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        self.terminate_all(ids).await;
    }

    async fn terminate_all(&self, ids: Vec<String>) {
        futures_util::future::join_all(ids.iter().map(|id| self.terminate(id))).await;
    }

    pub async fn cleanup_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
        loop {
            interval.tick().await;
            self.cleanup_finished().await;
        }
    }

    fn record(&self, id: &str) -> Option<Arc<ProcessRecord>> {
        self.processes.get(id).map(|entry| entry.value().clone())
    }

    async fn running_count(&self, agent_id: Option<Uuid>) -> usize {
        let records = self
            .processes
            .iter()
            .filter(|entry| agent_id.is_none_or(|id| entry.metadata.agent_id == id))
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        let mut count = 0;
        for record in records {
            if record.state.lock().await.status == ProcessStatus::Running {
                count += 1;
            }
        }
        count
    }

    async fn cleanup_finished(&self) {
        let records = self
            .processes
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();
        for (id, record) in records {
            let state = record.state.lock().await;
            if state.status != ProcessStatus::Running
                && record.started_instant.elapsed() > FINISHED_RETENTION
            {
                drop(state);
                self.processes.remove(&id);
            }
        }
        let retained = self
            .processes
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();
        let mut finished = Vec::new();
        for (id, record) in retained {
            if record.state.lock().await.status != ProcessStatus::Running {
                finished.push((id, record.started_at));
            }
        }
        if finished.len() > MAX_RETAINED_PROCESSES {
            finished.sort_by_key(|(_, started_at)| *started_at);
            let remove_count = finished.len() - MAX_RETAINED_PROCESSES;
            for (id, _) in finished.into_iter().take(remove_count) {
                self.processes.remove(&id);
            }
        }
    }
}

pub async fn execute_terminal(
    request: &ExecuteTerminalRequest,
    config: &TerminalToolConfig,
    manager: &Arc<TerminalManager>,
) -> ToolResult {
    if !config.enabled {
        return ToolResult::Blocked {
            reason: "Terminal tool is disabled".to_string(),
        };
    }
    if request.command.trim().is_empty() {
        return ToolResult::Blocked {
            reason: "Command cannot be empty".to_string(),
        };
    }
    if request.command.len() > 8_000 {
        return ToolResult::Blocked {
            reason: "Command is too long".to_string(),
        };
    }

    let escalation_requested = request.sandbox_permissions == SandboxPermissions::RequireEscalated;
    let full_access_mode = config.permission_mode == TerminalPermissionMode::FullAccess;
    let allow_outside_workspace = full_access_mode || escalation_requested;
    let workspace = match resolve_workspace(request.cwd.as_deref(), config, allow_outside_workspace)
    {
        Ok(workspace) => workspace,
        Err(WorkspaceResolutionError::Invalid(message)) => {
            return ToolResult::Error { message };
        }
        Err(WorkspaceResolutionError::Blocked(reason)) => {
            return ToolResult::Blocked { reason };
        }
    };

    let context_granted =
        escalation_requested && config.has_context_grant(&request.command, request.cwd.as_deref());
    if !full_access_mode && escalation_requested && !context_granted && !request.approved {
        return ToolResult::PendingApproval {
            command: request.command.clone(),
            approval_id: Uuid::new_v4().to_string(),
        };
    }

    let access = if full_access_mode || escalation_requested {
        TerminalAccess::FullAccess
    } else {
        TerminalAccess::Sandbox
    };
    let tty = request.tty.unwrap_or(false);
    if cfg!(windows) && access == TerminalAccess::Sandbox && tty {
        return ToolResult::Blocked {
            reason: "Codex Windows sandbox cannot provide a PTY. Use tty=false for a short non-interactive command, or request full access with a justification when an interactive terminal is required."
                .to_string(),
        };
    }
    if cfg!(windows) && access == TerminalAccess::Sandbox && request.background.unwrap_or(false) {
        return ToolResult::Blocked {
            reason: "Codex Windows sandbox does not support observable background or interactive sessions. Run this as a short foreground command, or request full access with a justification for long-lived work."
                .to_string(),
        };
    }
    let process_id = Uuid::new_v4().to_string();
    let runtime = match access {
        TerminalAccess::Sandbox => "codex_sandbox",
        TerminalAccess::FullAccess => "codex_full_access",
    }
    .to_string();
    if let Err(error) = manager
        .spawn(
            BackendExecRequest {
                process_id: process_id.clone(),
                command: request.command.clone(),
                workspace_root: workspace.root.clone(),
                cwd: workspace.cwd.clone(),
                access,
                tty,
            },
            ProcessMetadata {
                agent_id: request.agent_id,
                conversation_id: request.conversation_id,
                tool_call_id: request.tool_call_id.clone(),
                command: request.command.clone(),
                cwd: workspace.cwd.to_string_lossy().to_string(),
                runtime: runtime.clone(),
                tty,
                detached: request.background.unwrap_or(false),
            },
        )
        .await
    {
        return ToolResult::Error {
            message: format!("Failed to start terminal command: {error}"),
        };
    }

    let mut lease = TerminalLease::new(manager.clone(), process_id.clone());
    if request.background.unwrap_or(false) {
        lease.disarm();
        return ToolResult::Running {
            process_id,
            message: format!("Command started in a Codex-managed background session ({runtime})"),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            runtime: Some(runtime),
            output_cursor: 0,
        };
    }

    let timeout_ms = request
        .timeout_ms
        .unwrap_or(config.default_timeout_ms)
        .clamp(1, MAX_TERMINAL_TIMEOUT_MS);
    let result = match manager
        .wait_for_completion(&process_id, Duration::from_millis(timeout_ms))
        .await
    {
        Some(info) if info.status == ProcessStatus::Running => {
            if cfg!(windows) && access == TerminalAccess::Sandbox {
                let _ = manager.terminate(&process_id).await;
                manager.remove(&process_id);
                ToolResult::Error {
                    message: format!(
                        "Codex Windows sandbox command exceeded {timeout_ms}ms and was stopped. Request full access with a justification for background or interactive work."
                    ),
                }
            } else {
                manager.promote_to_background(&process_id);
                let cursor = info.output_cursor;
                ToolResult::Running {
                    process_id,
                    message: format!(
                        "Command exceeded {timeout_ms}ms and continues in a Codex-managed session"
                    ),
                    stdout: truncate_for_llm(info.stdout),
                    stderr: truncate_for_llm(info.stderr),
                    duration_ms: info.duration_ms,
                    runtime: Some(runtime),
                    output_cursor: cursor,
                }
            }
        }
        Some(info) => {
            let result = process_info_to_result(info);
            manager.remove(&process_id);
            result
        }
        None => ToolResult::Error {
            message: "Terminal session disappeared before completion".to_string(),
        },
    };
    lease.disarm();
    result
}

pub fn process_info_to_result(info: ProcessInfo) -> ToolResult {
    process_read_to_result(info.clone(), info.stdout, info.stderr, info.output_cursor)
}

pub fn process_read_to_result(
    info: ProcessInfo,
    stdout: String,
    stderr: String,
    output_cursor: u64,
) -> ToolResult {
    match info.status {
        ProcessStatus::Completed | ProcessStatus::Failed => ToolResult::Completed {
            stdout: truncate_for_llm(stdout),
            stderr: truncate_for_llm(stderr),
            exit_code: info.exit_code.unwrap_or(-1),
            duration_ms: info.duration_ms,
            runtime: Some(info.runtime),
            output_cursor,
        },
        ProcessStatus::Running => ToolResult::Running {
            process_id: info.process_id,
            message: "Terminal session is still running".to_string(),
            stdout: truncate_for_llm(stdout),
            stderr: truncate_for_llm(stderr),
            duration_ms: info.duration_ms,
            runtime: Some(info.runtime),
            output_cursor,
        },
        ProcessStatus::Stopped => ToolResult::Rejected {
            reason: "Terminal session was stopped".to_string(),
        },
    }
}

struct TerminalLease {
    manager: Arc<TerminalManager>,
    process_id: String,
    armed: bool,
}

impl TerminalLease {
    fn new(manager: Arc<TerminalManager>, process_id: String) -> Self {
        Self {
            manager,
            process_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let manager = self.manager.clone();
        let process_id = self.process_id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = manager.terminate(&process_id).await;
            });
        }
    }
}

struct ResolvedWorkspace {
    root: PathBuf,
    cwd: PathBuf,
}

enum WorkspaceResolutionError {
    Invalid(String),
    Blocked(String),
}

fn resolve_workspace(
    cwd: Option<&str>,
    config: &TerminalToolConfig,
    allow_outside_workspace: bool,
) -> Result<ResolvedWorkspace, WorkspaceResolutionError> {
    let roots = if config.allowed_cwd_roots.is_empty() {
        vec![std::env::current_dir().map_err(|error| {
            WorkspaceResolutionError::Invalid(format!(
                "Failed to resolve server working directory: {error}"
            ))
        })?]
    } else {
        config.allowed_cwd_roots.iter().map(PathBuf::from).collect()
    };
    let roots = roots
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err(WorkspaceResolutionError::Invalid(
            "No valid terminal workspace root is configured".to_string(),
        ));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .and_then(|path| std::fs::canonicalize(path).ok());
    if roots.iter().any(|root| {
        root.parent().is_none()
            || home
                .as_ref()
                .is_some_and(|home| crate::fsutil::paths_equal(home, root))
    }) {
        return Err(WorkspaceResolutionError::Blocked(
            "Filesystem roots and the entire user home directory cannot be terminal workspace roots"
                .to_string(),
        ));
    }

    let requested = match cwd.map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => roots[0].join(path),
        None => roots[0].clone(),
    };
    let canonical_cwd = std::fs::canonicalize(&requested).map_err(|error| {
        WorkspaceResolutionError::Invalid(format!(
            "Working directory '{}' does not exist or cannot be accessed: {error}. Retry without cwd to use the default workspace '{}'.",
            requested.display(),
            roots[0].display()
        ))
    })?;
    let matching_root = roots
        .iter()
        .find(|root| crate::fsutil::path_is_within(&canonical_cwd, root))
        .cloned();
    if matching_root.is_none() && !allow_outside_workspace {
        return Err(WorkspaceResolutionError::Blocked(format!(
            "Working directory '{}' is outside the configured Codex sandbox roots. Retry with sandbox_permissions='require_escalated' if the user asked to work there.",
            canonical_cwd.display()
        )));
    }
    Ok(ResolvedWorkspace {
        root: matching_root.unwrap_or_else(|| roots[0].clone()),
        cwd: canonical_cwd,
    })
}

async fn snapshot(record: &ProcessRecord) -> ProcessInfo {
    let state = record.state.lock().await;
    let output = record.output.lock().await.read_all();
    ProcessInfo {
        process_id: record.process_id.clone(),
        agent_id: record.metadata.agent_id,
        conversation_id: record.metadata.conversation_id,
        tool_call_id: record.metadata.tool_call_id.clone(),
        command: record.metadata.command.clone(),
        cwd: Some(record.metadata.cwd.clone()),
        status: state.status.clone(),
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: state.exit_code,
        started_at: record.started_at,
        finished_at: state.finished_at,
        duration_ms: state
            .duration_ms
            .unwrap_or_else(|| record.started_instant.elapsed().as_millis() as u64),
        runtime: record.metadata.runtime.clone(),
        tty: record.metadata.tty,
        output_cursor: output.cursor,
    }
}

fn truncate_for_llm(value: String) -> String {
    if value.len() <= MAX_LLM_TOOL_OUTPUT_BYTES {
        return value;
    }
    let start = value.len() - MAX_LLM_TOOL_OUTPUT_BYTES;
    let tail = String::from_utf8_lossy(&value.as_bytes()[start..]);
    format!("[output truncated; showing last {MAX_LLM_TOOL_OUTPUT_BYTES} bytes]\n{tail}")
}

fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn windows_utf8_command(command: &str) -> String {
    format!("chcp 65001>nul & {command}")
}

fn windows_powershell_cmd_script(shell_command: &str, cwd: Option<&Path>) -> String {
    let shell_command = windows_utf8_command(shell_command);
    let set_location = match cwd {
        Some(cwd) => format!(
            "Set-Location -LiteralPath {}; ",
            quote_powershell_literal(&windows_command_path(cwd))
        ),
        None => String::new(),
    };
    format!(
        "$ErrorActionPreference = 'Stop'; $OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); {set_location}$command = {}; & cmd.exe /D /S /C $command; exit $LASTEXITCODE",
        quote_powershell_literal(&shell_command)
    )
}

fn quote_unix_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn os_command_path(path: &Path) -> String {
    if cfg!(windows) {
        windows_command_path(path)
    } else {
        path.to_string_lossy().to_string()
    }
}

fn windows_command_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    value
        .strip_prefix(r"\\?\")
        .unwrap_or(value.as_ref())
        .to_string()
}

fn decode_process_output(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(value) = std::str::from_utf8(bytes) {
        return value.to_string();
    }

    #[cfg(windows)]
    {
        let mut fallback = None;
        for code_page in windows_decode_code_page_candidates() {
            if let Some(encoding) = encoding_for_windows_code_page(code_page) {
                let (cow, _, had_errors) = encoding.decode(bytes);
                if !had_errors {
                    return cow.into_owned();
                }
                if fallback.is_none() {
                    fallback = Some(cow.into_owned());
                }
            }
        }
        fallback.unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
    }

    #[cfg(not(windows))]
    {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(windows)]
fn windows_decode_code_page_candidates() -> Vec<u32> {
    let [oem, acp] = windows_fallback_code_pages();
    let mut pages = Vec::with_capacity(4);
    for code_page in [oem, acp, 936] {
        if matches!(code_page, 932 | 936 | 949 | 950 | 54936) && !pages.contains(&code_page) {
            pages.push(code_page);
        }
    }
    if !pages.contains(&936) {
        pages.push(936);
    }
    for code_page in [oem, acp] {
        if !pages.contains(&code_page) {
            pages.push(code_page);
        }
    }
    pages
}

#[cfg(windows)]
fn windows_fallback_code_pages() -> [u32; 2] {
    unsafe { [GetOEMCP(), GetACP()] }
}

#[cfg(windows)]
fn encoding_for_windows_code_page(code_page: u32) -> Option<&'static encoding_rs::Encoding> {
    match code_page {
        65001 => Some(encoding_rs::UTF_8),
        936 => Some(encoding_rs::GBK),
        54936 => Some(encoding_rs::GB18030),
        950 => Some(encoding_rs::BIG5),
        932 => Some(encoding_rs::SHIFT_JIS),
        949 => Some(encoding_rs::EUC_KR),
        1250 => Some(encoding_rs::WINDOWS_1250),
        1251 => Some(encoding_rs::WINDOWS_1251),
        1252 => Some(encoding_rs::WINDOWS_1252),
        1253 => Some(encoding_rs::WINDOWS_1253),
        1254 => Some(encoding_rs::WINDOWS_1254),
        1255 => Some(encoding_rs::WINDOWS_1255),
        1256 => Some(encoding_rs::WINDOWS_1256),
        1257 => Some(encoding_rs::WINDOWS_1257),
        1258 => Some(encoding_rs::WINDOWS_1258),
        20866 => Some(encoding_rs::KOI8_R),
        21866 => Some(encoding_rs::KOI8_U),
        28591 => Some(encoding_rs::WINDOWS_1252),
        28592 => Some(encoding_rs::ISO_8859_2),
        28605 => Some(encoding_rs::ISO_8859_15),
        _ => encoding_rs::Encoding::for_label(format!("windows-{code_page}").as_bytes())
            .or_else(|| encoding_rs::Encoding::for_label(format!("cp{code_page}").as_bytes())),
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetACP() -> u32;
    fn GetOEMCP() -> u32;
}

#[cfg(test)]
#[derive(Default)]
struct TestTerminalBackend {
    processes: DashMap<String, Arc<TestProcess>>,
}

#[cfg(test)]
struct TestProcess {
    pid: u32,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
}

#[cfg(test)]
#[async_trait]
impl TerminalBackend for TestTerminalBackend {
    async fn execute(
        &self,
        request: BackendExecRequest,
        output: mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        use tokio::io::AsyncReadExt;

        let argv = build_codex_shell_argv(&request.command, &request.cwd);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| AppError::Process("test terminal command was empty".to_string()))?;
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .current_dir(&request.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| AppError::Process("test process has no pid".to_string()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        self.processes.insert(
            request.process_id.clone(),
            Arc::new(TestProcess {
                pid,
                stdin: Mutex::new(stdin),
            }),
        );
        let stdout_task = stdout.map(|mut stdout| {
            let output = output.clone();
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                loop {
                    match stdout.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if output
                                .send(BackendOutput {
                                    stream: OutputStream::Stdout,
                                    data: buffer[..read].to_vec(),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
        });
        let stderr_task = stderr.map(|mut stderr| {
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                loop {
                    match stderr.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if output
                                .send(BackendOutput {
                                    stream: OutputStream::Stderr,
                                    data: buffer[..read].to_vec(),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
        });
        let status = child.wait().await?;
        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }
        self.processes.remove(&request.process_id);
        Ok(status.code().unwrap_or(-1))
    }

    async fn write(&self, process_id: &str, data: &[u8], close_stdin: bool) -> AppResult<()> {
        use tokio::io::AsyncWriteExt;

        let process = self
            .processes
            .get(process_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| AppError::Process("test process was not found".to_string()))?;
        let mut stdin = process.stdin.lock().await;
        if let Some(writer) = stdin.as_mut() {
            writer.write_all(data).await?;
            writer.flush().await?;
        }
        if close_stdin {
            *stdin = None;
        }
        Ok(())
    }

    async fn terminate(&self, process_id: &str) -> AppResult<()> {
        let process = self
            .processes
            .get(process_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| AppError::Process("test process was not found".to_string()))?;
        if cfg!(windows) {
            tokio::process::Command::new("taskkill")
                .args(["/PID", &process.pid.to_string(), "/T", "/F"])
                .output()
                .await?;
        } else {
            tokio::process::Command::new("kill")
                .args(["-KILL", &process.pid.to_string()])
                .output()
                .await?;
        }
        Ok(())
    }

    async fn resize(&self, _process_id: &str, _rows: u16, _cols: u16) -> AppResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ImmediateOutputBackend;

    #[async_trait]
    impl TerminalBackend for ImmediateOutputBackend {
        async fn execute(
            &self,
            _request: BackendExecRequest,
            output: mpsc::Sender<BackendOutput>,
        ) -> AppResult<i32> {
            output
                .send(BackendOutput {
                    stream: OutputStream::Stdout,
                    data: b"final-output".to_vec(),
                })
                .await
                .map_err(|_| AppError::Process("test output receiver closed".to_string()))?;
            Ok(0)
        }

        async fn write(
            &self,
            _process_id: &str,
            _data: &[u8],
            _close_stdin: bool,
        ) -> AppResult<()> {
            Ok(())
        }

        async fn terminate(&self, _process_id: &str) -> AppResult<()> {
            Ok(())
        }

        async fn resize(&self, _process_id: &str, _rows: u16, _cols: u16) -> AppResult<()> {
            Ok(())
        }
    }

    #[test]
    fn output_buffer_returns_incremental_chunks() {
        let mut output = OutputBuffer::default();
        output.append(OutputStream::Stdout, b"one".to_vec());
        let first = output.read_since(0);
        assert_eq!(first.stdout, "one");
        output.append(OutputStream::Stderr, b"two".to_vec());
        let second = output.read_since(first.cursor);
        assert_eq!(second.stdout, "");
        assert_eq!(second.stderr, "two");
    }

    #[test]
    fn decode_process_output_keeps_valid_utf8() {
        assert_eq!(
            decode_process_output("hello world".as_bytes()),
            "hello world"
        );
    }

    #[tokio::test]
    async fn completion_waits_for_final_output_to_be_buffered() {
        let root = tempfile::tempdir().unwrap();
        let process_id = Uuid::new_v4().to_string();
        let manager = Arc::new(TerminalManager {
            backend: Arc::new(ImmediateOutputBackend),
            processes: DashMap::new(),
            spawn_lock: Mutex::new(()),
        });
        manager
            .spawn(
                BackendExecRequest {
                    process_id: process_id.clone(),
                    command: "immediate".to_string(),
                    workspace_root: root.path().to_path_buf(),
                    cwd: root.path().to_path_buf(),
                    access: TerminalAccess::FullAccess,
                    tty: false,
                },
                ProcessMetadata {
                    agent_id: Uuid::new_v4(),
                    conversation_id: Uuid::new_v4(),
                    tool_call_id: "immediate-output".to_string(),
                    command: "immediate".to_string(),
                    cwd: root.path().to_string_lossy().to_string(),
                    runtime: "test".to_string(),
                    tty: false,
                    detached: false,
                },
            )
            .await
            .unwrap();

        let info = manager
            .wait_for_completion(&process_id, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(info.status, ProcessStatus::Completed);
        assert_eq!(info.stdout, "final-output");
    }

    #[tokio::test]
    async fn background_session_accepts_stdin_and_returns_incremental_output() {
        let root = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new_for_tests();
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let command = if cfg!(windows) {
            r#"powershell -NoProfile -Command "Write-Output 'Prompt:'; $x=[Console]::In.ReadLine(); Write-Output ('Got:' + $x)""#
        } else {
            "printf 'Prompt:'; read value; printf 'Got:%s\\n' \"$value\""
        };
        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: command.to_string(),
                cwd: None,
                timeout_ms: Some(5_000),
                background: Some(true),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "stdin-test".to_string(),
            },
            &config,
            &manager,
        )
        .await;
        let ToolResult::Running { process_id, .. } = result else {
            panic!("expected running process");
        };
        let (prompt_info, prompt, prompt_error, cursor) = manager
            .read_since(&process_id, 0, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(
            prompt.contains("Prompt"),
            "unexpected prompt: {prompt:?}, stderr: {prompt_error:?}, status: {:?}",
            prompt_info.status
        );
        manager.write(&process_id, b"alpha\n", false).await.unwrap();
        let info = manager
            .wait_for_completion(&process_id, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(info.status, ProcessStatus::Completed);
        let (_, output, _, _) = manager
            .read_since(&process_id, cursor, Duration::ZERO)
            .await
            .unwrap();
        assert!(
            output.contains("Got:alpha"),
            "unexpected output: {output:?}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_sandbox_rejects_background_sessions() {
        let root = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new_for_tests();
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::RequestApproval,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo background".to_string(),
                cwd: None,
                timeout_ms: Some(5_000),
                background: Some(true),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "windows-sandbox-background".to_string(),
            },
            &config,
            &manager,
        )
        .await;
        let ToolResult::Blocked { reason } = result else {
            panic!("expected Windows sandbox background command to be blocked: {result:?}");
        };
        assert!(reason.contains("foreground"));
        assert!(manager.list(None).await.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires an installed Codex runtime"]
    async fn real_codex_backend_runs_full_access_and_sandbox_command() {
        let data = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("weiagent-codex-integration-data");
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let workspace = std::env::current_dir().unwrap();
        let runtime = Arc::new(CodexRuntimeManager::new(&data).unwrap());
        let status = runtime.initialize().await.unwrap();
        assert!(
            status.available,
            "Codex runtime unavailable: {:?}",
            status.error
        );
        let manager = TerminalManager::new(runtime.clone());

        let full_config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![workspace.to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let full = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo codex-full-access".to_string(),
                cwd: None,
                timeout_ms: Some(30_000),
                background: Some(false),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "real-full".to_string(),
            },
            &full_config,
            &manager,
        )
        .await;
        let ToolResult::Completed { stdout, .. } = full else {
            panic!("full access command failed: {full:?}");
        };
        assert!(stdout.contains("codex-full-access"));

        let interactive_command = if cfg!(windows) {
            r#"powershell -NoProfile -Command "Write-Output 'Prompt'; $x=[Console]::In.ReadLine(); Write-Output ('Got:' + $x)""#
        } else {
            "printf 'Prompt\\n'; read value; printf 'Got:%s\\n' \"$value\""
        };
        let interactive = execute_terminal(
            &ExecuteTerminalRequest {
                command: interactive_command.to_string(),
                cwd: None,
                timeout_ms: Some(30_000),
                background: Some(true),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "real-interactive".to_string(),
            },
            &full_config,
            &manager,
        )
        .await;
        let ToolResult::Running { process_id, .. } = interactive else {
            panic!("full access interactive command did not start: {interactive:?}");
        };
        let (_, prompt, prompt_error, cursor) = manager
            .read_since(&process_id, 0, Duration::from_secs(20))
            .await
            .unwrap();
        assert!(
            prompt.contains("Prompt"),
            "unexpected interactive prompt: {prompt:?}, stderr: {prompt_error:?}"
        );
        manager.write(&process_id, b"alpha\n", false).await.unwrap();
        let info = manager
            .wait_for_completion(&process_id, Duration::from_secs(20))
            .await
            .unwrap();
        assert_eq!(info.status, ProcessStatus::Completed);
        let (_, output, error_output, _) = manager
            .read_since(&process_id, cursor, Duration::ZERO)
            .await
            .unwrap();
        assert!(
            output.contains("Got:alpha"),
            "unexpected interactive output: {output:?}, stderr: {error_output:?}"
        );

        let sandbox_config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::RequestApproval,
            allowed_cwd_roots: vec![workspace.to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let sandbox = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo codex-sandbox".to_string(),
                cwd: None,
                timeout_ms: Some(30_000),
                background: Some(false),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "real-sandbox".to_string(),
            },
            &sandbox_config,
            &manager,
        )
        .await;
        let ToolResult::Completed { stdout, .. } = sandbox else {
            panic!("sandbox command failed: {sandbox:?}");
        };
        assert!(stdout.contains("codex-sandbox"));
    }
}
