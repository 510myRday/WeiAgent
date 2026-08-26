use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use portable_pty::{Child as PtyChild, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Deserialize;
use tokio::sync::{Mutex, Notify, mpsc};
use uuid::Uuid;

use crate::{
    domain::{ProcessInfo, ProcessStatus, TerminalPermissionMode, TerminalToolConfig, ToolResult},
    error::{AppError, AppResult},
    tools::{
        process_job::{TerminateMode, terminate_process},
        shell::{HostShellKind, selected_host_shell},
        workspace::{self, WorkspaceResolutionError},
    },
};

pub const MAX_TERMINAL_TIMEOUT_MS: u64 = 120_000;
pub const MAX_SESSION_WAIT_MS: u64 = 60_000;
const MAX_OUTPUT_BYTES: usize = 1_000_000;
const APPROX_BYTES_PER_TOKEN: usize = 4;
const MAX_LLM_TOOL_OUTPUT_TOKENS: usize = 10_000;
const MAX_LLM_TOOL_OUTPUT_BYTES: usize = MAX_LLM_TOOL_OUTPUT_TOKENS * APPROX_BYTES_PER_TOKEN;
const FINISHED_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_RUNNING_PROCESSES: usize = 32;
const MAX_RUNNING_PROCESSES_PER_AGENT: usize = 8;
const MAX_PROCESS_LIFETIME_MS: u64 = 60 * 60 * 1_000;
const MAX_RETAINED_PROCESSES: usize = 256;
const OUTPUT_CHANNEL_CAPACITY: usize = 256;

#[async_trait]
pub trait TerminalEnvironment: Send + Sync {
    async fn managed_bin_paths(&self) -> Vec<PathBuf>;
}

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

#[derive(Debug, Clone)]
struct BackendExecRequest {
    process_id: String,
    command: String,
    cwd: PathBuf,
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

struct LocalTerminalBackend {
    environment: Arc<dyn TerminalEnvironment>,
    host_processes: DashMap<String, Arc<HostProcess>>,
    pty_processes: DashMap<String, Arc<PtyProcess>>,
}

struct HostProcess {
    pid: u32,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
}

struct PtyProcess {
    master: std::sync::Mutex<Box<dyn MasterPty + Send>>,
    writer: Arc<std::sync::Mutex<Option<Box<dyn Write + Send>>>>,
    child: std::sync::Mutex<Box<dyn PtyChild + Send + Sync>>,
}

impl LocalTerminalBackend {
    fn new(environment: Arc<dyn TerminalEnvironment>) -> Self {
        Self {
            environment,
            host_processes: DashMap::new(),
            pty_processes: DashMap::new(),
        }
    }

    async fn execute_host_process(
        &self,
        request: &BackendExecRequest,
        output: &mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        use tokio::io::AsyncReadExt;

        let argv = build_shell_argv(&request.command, &request.cwd);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| AppError::Process("Host command was empty".to_string()))?;
        let mut std_command = std::process::Command::new(program);
        std_command
            .args(args)
            .current_dir(&request.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            std_command.process_group(0);
        }
        let mut command = tokio::process::Command::from(std_command);
        command.kill_on_drop(false);
        if let Some(path) = managed_path(self.environment.as_ref()).await {
            command.env("PATH", path);
        }
        let mut child = command
            .spawn()
            .map_err(|error| AppError::Process(format!("failed to start host command: {error}")))?;
        let pid = child
            .id()
            .ok_or_else(|| AppError::Process("Windows host process has no pid".to_string()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        self.host_processes.insert(
            request.process_id.clone(),
            Arc::new(HostProcess {
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
                terminate_host_process_group(pid).await?;
                let _ = child.wait().await;
                self.host_processes.remove(&request.process_id);
                return Err(AppError::Process(
                    "Host command exceeded one hour".to_string(),
                ));
            }
        };
        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }
        self.host_processes.remove(&request.process_id);
        Ok(status.code().unwrap_or(-1))
    }

    async fn execute_pty_process(
        &self,
        request: &BackendExecRequest,
        output: &mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        let argv = build_pty_shell_argv(&request.command, &request.cwd);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| AppError::Process("Host command was empty".to_string()))?;
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(&request.cwd);
        if let Some(path) = managed_path(self.environment.as_ref()).await {
            command.env("PATH", path);
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| AppError::Process(format!("failed to open PTY: {error}")))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| AppError::Process(format!("failed to start PTY command: {error}")))?;
        // The slave handle belongs to the child side. Keeping it open in the parent can prevent
        // ConPTY from completing input and shutdown on Windows.
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| AppError::Process(format!("failed to read PTY output: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| AppError::Process(format!("failed to open PTY input: {error}")))?;
        let process = Arc::new(PtyProcess {
            master: std::sync::Mutex::new(pair.master),
            writer: Arc::new(std::sync::Mutex::new(Some(writer))),
            child: std::sync::Mutex::new(child),
        });
        self.pty_processes
            .insert(request.process_id.clone(), process.clone());

        let output = output.clone();
        let reader_writer = Arc::downgrade(&process.writer);
        let _reader_thread =
            std::thread::spawn(move || read_pty_output(reader, output, reader_writer));

        let wait_process = process.clone();
        let status = tokio::task::spawn_blocking(move || {
            let mut child = wait_process
                .child
                .lock()
                .map_err(|_| AppError::Process("PTY child lock was poisoned".to_string()))?;
            child.wait().map_err(|error| {
                AppError::Process(format!("failed waiting for PTY command: {error}"))
            })
        })
        .await
        .map_err(|error| AppError::Process(format!("PTY wait task failed: {error}")))??;
        close_pty_writer(&process);
        self.pty_processes.remove(&request.process_id);
        drop(process);
        Ok(status.exit_code() as i32)
    }

    async fn host_process(&self, process_id: &str) -> Option<Arc<HostProcess>> {
        for _ in 0..20 {
            if let Some(process) = self
                .host_processes
                .get(process_id)
                .map(|entry| entry.value().clone())
            {
                return Some(process);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }

    async fn pty_process(&self, process_id: &str) -> Option<Arc<PtyProcess>> {
        for _ in 0..20 {
            if let Some(process) = self
                .pty_processes
                .get(process_id)
                .map(|entry| entry.value().clone())
            {
                return Some(process);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }
}

#[async_trait]
impl TerminalBackend for LocalTerminalBackend {
    async fn execute(
        &self,
        request: BackendExecRequest,
        output: mpsc::Sender<BackendOutput>,
    ) -> AppResult<i32> {
        if request.tty {
            self.execute_pty_process(&request, &output).await
        } else {
            self.execute_host_process(&request, &output).await
        }
    }

    async fn write(&self, process_id: &str, data: &[u8], close_stdin: bool) -> AppResult<()> {
        if let Some(process) = self.pty_processes.get(process_id) {
            return write_pty_process_async(process.value().clone(), data.to_vec(), close_stdin)
                .await;
        }
        if !self.host_processes.contains_key(process_id)
            && let Some(process) = self.pty_process(process_id).await
        {
            return write_pty_process_async(process, data.to_vec(), close_stdin).await;
        }
        use tokio::io::AsyncWriteExt;

        let process = self.host_process(process_id).await.ok_or_else(|| {
            AppError::Process(format!("host process '{process_id}' was not found"))
        })?;
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
        if let Some(process) = self.pty_processes.get(process_id) {
            return terminate_pty_process_async(process.value().clone()).await;
        }
        if !self.host_processes.contains_key(process_id)
            && let Some(process) = self.pty_process(process_id).await
        {
            return terminate_pty_process_async(process).await;
        }
        let process = self.host_process(process_id).await.ok_or_else(|| {
            AppError::Process(format!("host process '{process_id}' was not found"))
        })?;
        terminate_host_process_group(process.pid).await
    }

    async fn resize(&self, process_id: &str, rows: u16, cols: u16) -> AppResult<()> {
        let process = self.pty_process(process_id).await.ok_or_else(|| {
            AppError::Process(format!("PTY process '{process_id}' was not found"))
        })?;
        tokio::task::spawn_blocking(move || resize_pty_process(&process, rows, cols))
            .await
            .map_err(|error| AppError::Process(format!("PTY resize task failed: {error}")))?
    }
}

fn write_pty_process(process: &PtyProcess, data: &[u8], close_stdin: bool) -> AppResult<()> {
    let mut writer = process
        .writer
        .lock()
        .map_err(|_| AppError::Process("PTY writer lock was poisoned".to_string()))?;
    if let Some(writer) = writer.as_mut() {
        writer.write_all(data)?;
        writer.flush()?;
    }
    if close_stdin {
        *writer = None;
    }
    Ok(())
}

fn close_pty_writer(process: &PtyProcess) {
    if let Ok(mut writer) = process.writer.lock() {
        *writer = None;
    }
}

async fn write_pty_process_async(
    process: Arc<PtyProcess>,
    data: Vec<u8>,
    close_stdin: bool,
) -> AppResult<()> {
    tokio::task::spawn_blocking(move || write_pty_process(&process, &data, close_stdin))
        .await
        .map_err(|error| AppError::Process(format!("PTY write task failed: {error}")))?
}

fn terminate_pty_process(process: &PtyProcess) -> AppResult<()> {
    let mut child = process
        .child
        .lock()
        .map_err(|_| AppError::Process("PTY child lock was poisoned".to_string()))?;
    child.kill()?;
    Ok(())
}

async fn terminate_pty_process_async(process: Arc<PtyProcess>) -> AppResult<()> {
    tokio::task::spawn_blocking(move || terminate_pty_process(&process))
        .await
        .map_err(|error| AppError::Process(format!("PTY terminate task failed: {error}")))?
}

fn resize_pty_process(process: &PtyProcess, rows: u16, cols: u16) -> AppResult<()> {
    let master = process
        .master
        .lock()
        .map_err(|_| AppError::Process("PTY master lock was poisoned".to_string()))?;
    master
        .resize(PtySize {
            rows: rows.clamp(1, 500),
            cols: cols.clamp(1, 500),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| AppError::Process(format!("failed to resize PTY: {error}")))
}

/// Returns the process PATH with managed runtime directories placed first.
///
/// Both terminal commands and MCP stdio processes use this path so installed
/// runtimes remain available even when WeiAgent was launched without the
/// user's interactive shell profile.
pub async fn managed_path(environment: &dyn TerminalEnvironment) -> Option<OsString> {
    let current = std::env::var_os("PATH");
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf));
    managed_path_from_with_executable(environment, current.as_deref(), executable_dir.as_deref())
        .await
}

pub async fn managed_path_from(
    environment: &dyn TerminalEnvironment,
    current: Option<&OsStr>,
) -> Option<OsString> {
    managed_path_from_with_executable(environment, current, None).await
}

async fn managed_path_from_with_executable(
    environment: &dyn TerminalEnvironment,
    current: Option<&OsStr>,
    executable_dir: Option<&Path>,
) -> Option<OsString> {
    let mut paths = environment.managed_bin_paths().await;
    if let Some(executable_dir) = executable_dir {
        paths.push(executable_dir.to_path_buf());
    }
    #[cfg(windows)]
    append_windows_system_paths(&mut paths);
    if let Some(current) = current {
        paths.extend(std::env::split_paths(current));
    }
    let mut unique_paths = Vec::with_capacity(paths.len());
    for path in paths {
        if !unique_paths.iter().any(|current| current == &path) {
            unique_paths.push(path);
        }
    }
    std::env::join_paths(unique_paths).ok()
}

#[cfg(windows)]
fn append_windows_system_paths(paths: &mut Vec<PathBuf>) {
    let mut system_paths = Vec::new();
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        let system_root = PathBuf::from(system_root);
        system_paths.push(system_root.join("System32"));
        system_paths.push(system_root);
    }
    if let Some(comspec) = std::env::var_os("ComSpec")
        && let Some(system32) = Path::new(&comspec).parent()
    {
        system_paths.push(system32.to_path_buf());
    }

    paths.extend(system_paths);
}

fn read_pty_output(
    mut reader: Box<dyn Read + Send>,
    output: mpsc::Sender<BackendOutput>,
    writer: std::sync::Weak<std::sync::Mutex<Option<Box<dyn Write + Send>>>>,
) {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                respond_to_terminal_queries(&buffer[..read], &writer);
                if output
                    .blocking_send(BackendOutput {
                        stream: OutputStream::Stdout,
                        data: buffer[..read].to_vec(),
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

fn respond_to_terminal_queries(
    data: &[u8],
    writer: &std::sync::Weak<std::sync::Mutex<Option<Box<dyn Write + Send>>>>,
) {
    if !data.windows(4).any(|window| window == b"\x1b[6n") {
        return;
    }
    let Some(writer) = writer.upgrade() else {
        return;
    };
    let Ok(mut writer) = writer.lock() else {
        return;
    };
    if let Some(writer) = writer.as_mut() {
        let _ = writer.write_all(b"\x1b[1;1R");
        let _ = writer.flush();
    }
}

async fn terminate_host_process_group(pid: u32) -> AppResult<()> {
    terminate_process(pid, TerminateMode::GroupGraceful).await
}

/// Builds the native host-shell argv used by the local process backend.
fn build_shell_argv(shell_command: &str, cwd: &Path) -> Vec<String> {
    let shell = selected_host_shell();
    match shell.kind() {
        #[cfg(windows)]
        HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => vec![
            shell.executable().to_string_lossy().to_string(),
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            windows_powershell_script(shell_command, cwd),
        ],
        #[cfg(windows)]
        HostShellKind::Cmd => vec![
            shell.executable().to_string_lossy().to_string(),
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            windows_cmd_script(shell_command, cwd),
        ],
        #[cfg(not(windows))]
        HostShellKind::PosixSh => vec![
            shell.executable().to_string_lossy().to_string(),
            "-lc".to_string(),
            format!("cd -- {} && {}", quote_unix_path(cwd), shell_command),
        ],
    }
}

fn build_pty_shell_argv(shell_command: &str, cwd: &Path) -> Vec<String> {
    build_shell_argv(shell_command, cwd)
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum AnsiState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

#[derive(Default)]
struct AnsiStripper {
    state: AnsiState,
}

impl AnsiStripper {
    fn strip(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());
        for &byte in data {
            match self.state {
                AnsiState::Ground => {
                    if byte == 0x1b {
                        self.state = AnsiState::Escape;
                    } else {
                        output.push(byte);
                    }
                }
                AnsiState::Escape => {
                    self.state = match byte {
                        b'[' => AnsiState::Csi,
                        b']' => AnsiState::Osc,
                        b'P' | b'X' | b'^' | b'_' => AnsiState::String,
                        0x20..=0x2f => AnsiState::Escape,
                        _ => AnsiState::Ground,
                    };
                }
                AnsiState::Csi => {
                    if !(0x20..=0x3f).contains(&byte) {
                        self.state = AnsiState::Ground;
                    }
                }
                AnsiState::Osc => match byte {
                    0x07 => self.state = AnsiState::Ground,
                    0x1b => self.state = AnsiState::OscEscape,
                    _ => {}
                },
                AnsiState::OscEscape => {
                    self.state = if byte == b'\\' {
                        AnsiState::Ground
                    } else if byte == 0x1b {
                        AnsiState::OscEscape
                    } else {
                        AnsiState::Osc
                    };
                }
                AnsiState::String => {
                    if byte == 0x1b {
                        self.state = AnsiState::StringEscape;
                    }
                }
                AnsiState::StringEscape => {
                    self.state = if byte == b'\\' {
                        AnsiState::Ground
                    } else if byte == 0x1b {
                        AnsiState::StringEscape
                    } else {
                        AnsiState::String
                    };
                }
            }
        }
        output
    }
}

#[derive(Default)]
struct OutputBuffer {
    chunks: VecDeque<OutputChunk>,
    next_cursor: u64,
    bytes: usize,
    stdout_ansi: AnsiStripper,
    stderr_ansi: AnsiStripper,
}

struct OutputRead {
    stdout: String,
    stderr: String,
    cursor: u64,
}

impl OutputBuffer {
    fn append(&mut self, stream: OutputStream, data: Vec<u8>) {
        let data = match stream {
            OutputStream::Stdout => self.stdout_ansi.strip(&data),
            OutputStream::Stderr => self.stderr_ansi.strip(&data),
        };
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
    pub fn new(environment: Arc<dyn TerminalEnvironment>) -> Arc<Self> {
        Arc::new(Self {
            backend: Arc::new(LocalTerminalBackend::new(environment)),
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
        record.stop_requested.store(true, Ordering::SeqCst);
        if record.metadata.tty {
            self.backend.write(id, &[3], false).await?;
        } else {
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
        if let Some(info) = self.wait_for_completion(id, Duration::from_secs(5)).await
            && info.status == ProcessStatus::Running
        {
            self.mark_stopped(&record).await;
        }
        Ok(Some(snapshot(&record).await))
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

    async fn mark_stopped(&self, record: &ProcessRecord) {
        let mut state = record.state.lock().await;
        if state.status == ProcessStatus::Running {
            state.status = ProcessStatus::Stopped;
            state.finished_at = Some(Utc::now());
            state.duration_ms = Some(record.started_instant.elapsed().as_millis() as u64);
        }
        drop(state);
        record.changed.notify_waiters();
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
    default_workspace: &Path,
    manager: &Arc<TerminalManager>,
) -> ToolResult {
    if !config.enabled {
        return ToolResult::Blocked {
            reason: "Terminal tool is disabled".to_string(),
            code: None,
            details: None,
        };
    }
    if request.command.trim().is_empty() {
        return ToolResult::Blocked {
            reason: "Command cannot be empty".to_string(),
            code: None,
            details: None,
        };
    }
    if request.command.len() > 8_000 {
        return ToolResult::Blocked {
            reason: "Command is too long".to_string(),
            code: None,
            details: None,
        };
    }

    let escalation_requested = request.sandbox_permissions == SandboxPermissions::RequireEscalated;
    let full_access_mode = config.permission_mode == TerminalPermissionMode::FullAccess;
    let context_granted = config.has_context_grant(&request.command, request.cwd.as_deref());
    let allow_outside_workspace =
        full_access_mode || escalation_requested || context_granted || request.approved;
    let workspace = match workspace::resolve_workspace_async(
        request.cwd.clone(),
        config.allowed_cwd_roots.clone(),
        default_workspace.to_path_buf(),
        allow_outside_workspace,
        workspace::WorkspaceResolveOptions {
            expand_home: false,
            blocked_message:
                "Filesystem roots and the entire user home directory cannot be terminal workspace roots",
        },
    )
    .await
    {
        Ok(workspace) => workspace,
        Err(WorkspaceResolutionError::Invalid(message)) => {
            return ToolResult::Error { message };
        }
        Err(WorkspaceResolutionError::Outside) => {
            return ToolResult::PendingApproval {
                command: request.command.clone(),
                approval_id: Uuid::new_v4().to_string(),
            };
        }
        Err(WorkspaceResolutionError::Blocked(reason)) => {
            return ToolResult::Blocked {
                reason,
                code: None,
                details: None,
            };
        }
    };

    if !full_access_mode && escalation_requested && !context_granted && !request.approved {
        return ToolResult::PendingApproval {
            command: request.command.clone(),
            approval_id: Uuid::new_v4().to_string(),
        };
    }

    let tty = request.tty.unwrap_or(false);
    let process_id = Uuid::new_v4().to_string();
    let runtime = "local_shell".to_string();
    if let Err(error) = manager
        .spawn(
            BackendExecRequest {
                process_id: process_id.clone(),
                command: request.command.clone(),
                cwd: workspace.cwd.clone(),
                tty,
            },
            ProcessMetadata {
                agent_id: request.agent_id,
                conversation_id: request.conversation_id,
                tool_call_id: request.tool_call_id.clone(),
                command: request.command.clone(),
                cwd: crate::fsutil::display_path(&workspace.cwd),
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
            message: format!("Command started in a local background session ({runtime})"),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            runtime: Some(runtime),
            cwd: Some(crate::fsutil::display_path(&workspace.cwd)),
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
            manager.promote_to_background(&process_id);
            let cursor = info.output_cursor;
            let (stdout, stderr) = truncate_terminal_output(info.stdout, info.stderr);
            ToolResult::Running {
                process_id,
                message: format!(
                    "Command exceeded {timeout_ms}ms and continues in a local session"
                ),
                stdout,
                stderr,
                duration_ms: info.duration_ms,
                runtime: Some(runtime),
                cwd: info.cwd.clone(),
                output_cursor: cursor,
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
    let (stdout, stderr) = truncate_terminal_output(stdout, stderr);
    match info.status {
        ProcessStatus::Completed | ProcessStatus::Failed => ToolResult::Completed {
            stdout,
            stderr,
            exit_code: info.exit_code.unwrap_or(-1),
            duration_ms: info.duration_ms,
            runtime: Some(info.runtime),
            cwd: info.cwd.clone(),
            output_cursor,
            structured_content: None,
            is_error: info.status == ProcessStatus::Failed,
        },
        ProcessStatus::Running => ToolResult::Running {
            process_id: info.process_id,
            message: "Terminal session is still running".to_string(),
            stdout,
            stderr,
            duration_ms: info.duration_ms,
            runtime: Some(info.runtime),
            cwd: info.cwd,
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

#[cfg(test)]
fn resolve_terminal_workspace(
    cwd: Option<&str>,
    config: &TerminalToolConfig,
    default_workspace: &Path,
    allow_outside_workspace: bool,
) -> Result<workspace::ResolvedWorkspace, WorkspaceResolutionError> {
    workspace::resolve_workspace(
        cwd,
        &config.allowed_cwd_roots,
        default_workspace,
        allow_outside_workspace,
        workspace::WorkspaceResolveOptions {
            expand_home: false,
            blocked_message: "Filesystem roots and the entire user home directory cannot be terminal workspace roots",
        },
    )
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

fn truncate_terminal_output(stdout: String, stderr: String) -> (String, String) {
    let total_bytes = stdout.len().saturating_add(stderr.len());
    if total_bytes <= MAX_LLM_TOOL_OUTPUT_BYTES {
        return (stdout, stderr);
    }

    let stdout_budget = proportional_budget(stdout.len(), total_bytes);
    let stderr_budget = MAX_LLM_TOOL_OUTPUT_BYTES.saturating_sub(stdout_budget);
    (
        truncate_middle_for_llm(stdout, stdout_budget),
        truncate_middle_for_llm(stderr, stderr_budget),
    )
}

fn proportional_budget(stream_bytes: usize, total_bytes: usize) -> usize {
    if stream_bytes == 0 || total_bytes == 0 {
        return 0;
    }
    MAX_LLM_TOOL_OUTPUT_BYTES
        .saturating_mul(stream_bytes)
        .checked_div(total_bytes)
        .unwrap_or(0)
        .min(stream_bytes)
}

fn truncate_middle_for_llm(value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }

    let original_bytes = value.len();
    let original_token_count = original_bytes.div_ceil(APPROX_BYTES_PER_TOKEN);
    let omitted_bytes = original_bytes.saturating_sub(max_bytes);
    let prefix_budget = max_bytes.div_ceil(2);
    let suffix_budget = max_bytes.saturating_sub(prefix_budget);
    let prefix_end = utf8_floor_boundary(&value, prefix_budget);
    let suffix_start = utf8_ceil_boundary(&value, original_bytes.saturating_sub(suffix_budget));
    let marker = format!(
        "\n[output truncated: original_token_count={original_token_count}, original_bytes={original_bytes}, omitted_bytes={omitted_bytes}]\n"
    );
    format!("{}{marker}{}", &value[..prefix_end], &value[suffix_start..])
}

fn utf8_floor_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn utf8_ceil_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(windows)]
fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn windows_utf8_command(command: &str) -> String {
    format!("chcp 65001>nul & {command}")
}

#[cfg(windows)]
fn quote_cmd_path(path: &Path) -> String {
    format!("\"{}\"", windows_command_path(path).replace('"', "\"\""))
}

#[cfg(windows)]
fn windows_cmd_script(shell_command: &str, cwd: &Path) -> String {
    windows_utf8_command(&format!("cd /d {} && {shell_command}", quote_cmd_path(cwd)))
}

#[cfg(windows)]
fn windows_powershell_script(shell_command: &str, cwd: &Path) -> String {
    let set_location = format!(
        "Set-Location -LiteralPath {}; ",
        quote_powershell_literal(&windows_command_path(cwd))
    );
    format!(
        "$ErrorActionPreference = 'Stop'; $OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); $global:LASTEXITCODE = 0; {set_location}$command = {}; & ([ScriptBlock]::Create($command)); exit $LASTEXITCODE",
        quote_powershell_literal(shell_command)
    )
}

#[cfg(not(windows))]
fn quote_unix_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
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
        decode_windows_output_with_code_pages(bytes, &windows_decode_code_page_candidates())
    }

    #[cfg(not(windows))]
    {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(windows)]
fn decode_windows_output_with_code_pages(bytes: &[u8], code_pages: &[u32]) -> String {
    let mut fallback = None;
    for &code_page in code_pages {
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

#[cfg(windows)]
fn windows_decode_code_page_candidates() -> Vec<u32> {
    let [oem, acp] = windows_fallback_code_pages();
    windows_decode_code_page_candidates_for(oem, acp)
}

#[cfg(windows)]
fn windows_decode_code_page_candidates_for(oem: u32, acp: u32) -> Vec<u32> {
    let mut pages = Vec::with_capacity(4);
    for code_page in [oem, acp, 936, 54936] {
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

        let argv = build_shell_argv(&request.command, &request.cwd);
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
        let _ = terminate_process(process.pid, TerminateMode::Force).await;
        Ok(())
    }

    async fn resize(&self, _process_id: &str, _rows: u16, _cols: u16) -> AppResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyTerminalEnvironment;

    #[async_trait]
    impl TerminalEnvironment for EmptyTerminalEnvironment {
        async fn managed_bin_paths(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    struct FixedTerminalEnvironment {
        paths: Vec<PathBuf>,
    }

    #[async_trait]
    impl TerminalEnvironment for FixedTerminalEnvironment {
        async fn managed_bin_paths(&self) -> Vec<PathBuf> {
            self.paths.clone()
        }
    }

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

    #[cfg(windows)]
    #[test]
    fn selected_windows_shell_drives_the_executable_and_command_syntax() {
        let cwd = Path::new(r"D:\workspace path");
        let argv = build_shell_argv("Write-Output 'ok'", cwd);
        let shell = selected_host_shell();

        assert_eq!(Path::new(&argv[0]), shell.executable());
        match shell.kind() {
            HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => {
                assert!(argv.iter().any(|arg| arg == "-Command"));
                let script = argv.last().unwrap();
                assert!(script.contains("[ScriptBlock]::Create"));
                assert!(script.contains("Set-Location -LiteralPath"));
                assert!(!script.contains("cmd.exe /D /S /C"));
            }
            HostShellKind::Cmd => {
                assert!(argv.iter().any(|arg| arg == "/C"));
                let script = argv.last().unwrap();
                assert!(script.contains("chcp 65001>nul"));
                assert!(script.contains(r#"cd /d "D:\workspace path""#));
            }
        }
    }

    #[tokio::test]
    async fn managed_path_places_runtime_directories_before_existing_path() {
        let managed = PathBuf::from("managed-runtime-bin");
        let existing = PathBuf::from("existing-bin");
        let system = PathBuf::from("system-bin");
        let environment = FixedTerminalEnvironment {
            paths: vec![managed.clone(), existing.clone(), managed.clone()],
        };
        let current = std::env::join_paths([&existing, &system]).expect("join test PATH");

        let path = managed_path_from(&environment, Some(&current))
            .await
            .expect("join managed PATH");
        let entries = std::env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(entries.first(), Some(&managed));
        assert_eq!(entries.get(1), Some(&existing));
        assert!(entries.contains(&system));
        assert_eq!(entries.iter().filter(|entry| *entry == &managed).count(), 1);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn managed_path_places_windows_system_directories_before_shadowing_tools() {
        let environment = EmptyTerminalEnvironment;
        let shadowing_tool = PathBuf::from(r"C:\rtools43\usr\bin");
        let current =
            std::env::join_paths([shadowing_tool.as_path(), Path::new(r"C:\Windows\System32")])
                .expect("join test PATH");

        let path = managed_path_from(&environment, Some(&current))
            .await
            .expect("join test PATH");
        let entries = std::env::split_paths(&path).collect::<Vec<_>>();
        let system32 =
            PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32");

        assert_eq!(entries.first(), Some(&system32));
        assert!(
            entries.iter().position(|entry| entry == &system32).unwrap()
                < entries
                    .iter()
                    .position(|entry| entry == &shadowing_tool)
                    .unwrap()
        );
    }

    #[tokio::test]
    async fn managed_path_includes_weiagent_executable_directory() {
        let executable_dir = PathBuf::from(r"D:\WeiAgent\target\debug");
        let current =
            std::env::join_paths([Path::new(r"C:\Windows\System32")]).expect("join test PATH");
        let path = managed_path_from_with_executable(
            &EmptyTerminalEnvironment,
            Some(&current),
            Some(&executable_dir),
        )
        .await
        .expect("join executable PATH");
        let entries = std::env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(entries.first(), Some(&executable_dir));
        assert!(entries.contains(&PathBuf::from(r"C:\Windows\System32")));
    }

    #[tokio::test]
    async fn selected_shell_executes_native_syntax_in_the_requested_cwd() {
        let root = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new_for_tests();
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let command = match selected_host_shell().kind() {
            #[cfg(windows)]
            HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => {
                "Write-Output ((Get-Location).Path); 1..3 | Select-Object -First 2"
            }
            #[cfg(windows)]
            HostShellKind::Cmd => "cd & (for %i in (1 2) do @echo %i)",
            #[cfg(not(windows))]
            HostShellKind::PosixSh => "pwd; printf '1\\n2\\n'",
        };

        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: command.to_string(),
                cwd: None,
                timeout_ms: Some(5_000),
                background: Some(false),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "selected-shell-native-syntax".to_string(),
            },
            &config,
            root.path(),
            &manager,
        )
        .await;

        let ToolResult::Completed {
            stdout, exit_code, ..
        } = result
        else {
            panic!("selected shell command failed: {result:?}");
        };
        assert_eq!(exit_code, 0);
        assert!(stdout.contains('1'));
        assert!(stdout.contains('2'));
        assert!(
            stdout
                .to_ascii_lowercase()
                .contains(&root.path().to_string_lossy().to_ascii_lowercase()),
            "expected cwd {} in output: {stdout:?}",
            root.path().display()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn bare_cmd_uses_windows_command_prompt_when_path_contains_rtools_cmd() {
        let root = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new(Arc::new(EmptyTerminalEnvironment));
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };

        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: "cmd /c echo terminal-cmd-ok".to_string(),
                cwd: None,
                timeout_ms: Some(5_000),
                background: Some(false),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "bare-cmd-resolution".to_string(),
            },
            &config,
            root.path(),
            &manager,
        )
        .await;

        let ToolResult::Completed {
            stdout, exit_code, ..
        } = result
        else {
            panic!("expected bare cmd command to complete: {result:?}");
        };
        assert_eq!(exit_code, 0);
        assert!(stdout.to_ascii_lowercase().contains("terminal-cmd-ok"));
    }

    #[tokio::test]
    async fn pty_session_accepts_input_and_resize() {
        let root = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new(Arc::new(EmptyTerminalEnvironment));
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let command = match selected_host_shell().kind() {
            #[cfg(windows)]
            HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => {
                "$value = Read-Host value; Write-Output ('echo:' + $value)"
            }
            #[cfg(windows)]
            HostShellKind::Cmd => "set /p value= & echo echo:%value%",
            #[cfg(not(windows))]
            HostShellKind::PosixSh => "read value; printf 'echo:%s\\n' \"$value\"",
        };
        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: command.to_string(),
                cwd: None,
                timeout_ms: Some(5_000),
                background: Some(true),
                tty: Some(true),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "pty-session-test".to_string(),
            },
            &config,
            root.path(),
            &manager,
        )
        .await;
        let ToolResult::Running { process_id, .. } = result else {
            panic!("expected a running PTY session, got {result:?}");
        };

        let newline: &[u8] = if cfg!(windows) {
            b"hello\r\n"
        } else {
            b"hello\n"
        };
        let resize_result =
            tokio::time::timeout(Duration::from_secs(2), manager.resize(&process_id, 30, 100))
                .await;
        resize_result
            .expect("PTY resize should return")
            .expect("PTY resize should succeed");
        let write_result = tokio::time::timeout(
            Duration::from_secs(2),
            manager.write(&process_id, newline, false),
        )
        .await;
        if write_result.is_err() {
            let _ = manager.terminate(&process_id).await;
            panic!("PTY write should return");
        }
        write_result.unwrap().unwrap();
        let info = manager
            .wait_for_completion(&process_id, Duration::from_secs(5))
            .await
            .expect("PTY session should finish");
        if info.status == ProcessStatus::Running {
            let _ = manager.terminate(&process_id).await;
            panic!("PTY session stayed running; output: {}", info.stdout);
        }
        assert_eq!(info.status, ProcessStatus::Completed);
        assert!(
            info.stdout.contains("echo:hello"),
            "output: {}",
            info.stdout
        );
        manager.remove(&process_id);
    }

    #[test]
    fn empty_roots_use_explicit_default_workspace() {
        let data_dir = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig::default();
        let workspace = match resolve_terminal_workspace(None, &config, data_dir.path(), false) {
            Ok(workspace) => workspace,
            Err(_) => {
                panic!("expected the dedicated directory to resolve as the default workspace")
            }
        };
        let expected = std::fs::canonicalize(data_dir.path()).unwrap();

        assert_eq!(workspace.cwd, expected);
    }

    #[test]
    fn configured_roots_are_additive_and_do_not_replace_the_default_workspace() {
        let default_workspace = tempfile::tempdir().unwrap();
        let configured_root = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            allowed_cwd_roots: vec![configured_root.path().display().to_string()],
            ..TerminalToolConfig::default()
        };
        let expected_default = std::fs::canonicalize(default_workspace.path()).unwrap();
        let expected_configured = std::fs::canonicalize(configured_root.path()).unwrap();

        let implicit =
            resolve_terminal_workspace(None, &config, default_workspace.path(), false).unwrap();
        assert_eq!(implicit.cwd, expected_default);

        let relative =
            resolve_terminal_workspace(Some("."), &config, default_workspace.path(), false)
                .unwrap();
        assert_eq!(relative.cwd, expected_default);

        let explicit = resolve_terminal_workspace(
            Some(configured_root.path().to_string_lossy().as_ref()),
            &config,
            default_workspace.path(),
            false,
        )
        .unwrap();
        assert_eq!(explicit.cwd, expected_configured);
    }

    #[tokio::test]
    async fn outside_workspace_requests_approval_with_default_permissions() {
        let default_workspace = tempfile::tempdir().unwrap();
        let outside_workspace = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new_for_tests();
        let config = TerminalToolConfig::default();

        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo outside".to_string(),
                cwd: Some(outside_workspace.path().display().to_string()),
                timeout_ms: Some(5_000),
                background: Some(false),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "outside-workspace-approval".to_string(),
            },
            &config,
            default_workspace.path(),
            &manager,
        )
        .await;

        assert!(matches!(result, ToolResult::PendingApproval { .. }));
    }

    #[tokio::test]
    async fn approved_outside_workspace_command_executes_with_default_permissions() {
        let default_workspace = tempfile::tempdir().unwrap();
        let outside_workspace = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new_for_tests();
        let config = TerminalToolConfig::default();

        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo approved".to_string(),
                cwd: Some(outside_workspace.path().display().to_string()),
                timeout_ms: Some(5_000),
                background: Some(false),
                tty: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: true,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "approved-outside-workspace".to_string(),
            },
            &config,
            default_workspace.path(),
            &manager,
        )
        .await;

        let ToolResult::Completed {
            stdout, exit_code, ..
        } = result
        else {
            panic!("approved outside-workspace command failed: {result:?}");
        };
        assert_eq!(exit_code, 0);
        assert!(stdout.to_ascii_lowercase().contains("approved"));
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
    fn output_buffer_strips_ansi_sequences_across_chunks_and_streams() {
        let mut output = OutputBuffer::default();
        output.append(OutputStream::Stdout, b"\x1b[3".to_vec());
        output.append(
            OutputStream::Stdout,
            b"2mgreen\x1b[0m \x1b]8;;https://example.com\x07link\x1b]8;;\x07".to_vec(),
        );
        output.append(OutputStream::Stderr, b"\x1b[31merror\x1b[39m".to_vec());

        let read = output.read_all();
        assert_eq!(read.stdout, "green link");
        assert_eq!(read.stderr, "error");
        assert!(!read.stdout.contains('\x1b'));
        assert!(!read.stderr.contains('\x1b'));
    }

    #[test]
    fn ansi_filter_keeps_non_escape_bytes_unchanged() {
        let bytes = [0x81, 0x9b, b'A'];
        let mut stripper = AnsiStripper::default();
        assert_eq!(stripper.strip(&bytes), bytes);
    }

    #[test]
    fn terminal_output_uses_shared_model_budget() {
        let stdout = "a".repeat(MAX_LLM_TOOL_OUTPUT_BYTES);
        let stderr = "b".repeat(MAX_LLM_TOOL_OUTPUT_BYTES);
        let (stdout, stderr) = truncate_terminal_output(stdout, stderr);

        assert!(stdout.starts_with(&"a".repeat(64)));
        assert!(stdout.ends_with(&"a".repeat(64)));
        assert!(stderr.starts_with(&"b".repeat(64)));
        assert!(stderr.ends_with(&"b".repeat(64)));
        assert!(stdout.contains("original_token_count=10000"));
        assert!(stderr.contains("omitted_bytes=20000"));
        assert!(stdout.len() + stderr.len() < MAX_LLM_TOOL_OUTPUT_BYTES + 512);
    }

    #[test]
    fn terminal_output_truncation_preserves_utf8_prefix_and_suffix() {
        let value = format!("BEGIN-{}-END", "中文输出".repeat(MAX_LLM_TOOL_OUTPUT_BYTES));
        let (stdout, stderr) = truncate_terminal_output(value, String::new());

        assert!(stdout.starts_with("BEGIN-"));
        assert!(stdout.ends_with("-END"));
        assert!(stdout.contains("[output truncated:"));
        assert!(stdout.contains("original_bytes="));
        assert!(stdout.contains("omitted_bytes="));
        assert!(stderr.is_empty());
    }

    #[test]
    fn decode_process_output_keeps_valid_utf8() {
        assert_eq!(
            decode_process_output("hello world".as_bytes()),
            "hello world"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_encoding_fallback_prefers_system_pages_before_chinese_fallbacks() {
        assert_eq!(
            windows_decode_code_page_candidates_for(437, 1252),
            vec![437, 1252, 936, 54936]
        );
        assert_eq!(
            windows_decode_code_page_candidates_for(936, 936),
            vec![936, 54936]
        );

        let (gbk, _, had_errors) = encoding_rs::GBK.encode("中文输出");
        assert!(!had_errors);
        assert_eq!(
            decode_windows_output_with_code_pages(&gbk, &[65001, 936, 54936]),
            "中文输出"
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
                    cwd: root.path().to_path_buf(),
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
        let command = match selected_host_shell().kind() {
            #[cfg(windows)]
            HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => {
                "Write-Output 'Prompt:'; $x=[Console]::In.ReadLine(); Write-Output ('Got:' + $x)"
            }
            #[cfg(windows)]
            HostShellKind::Cmd => r#"cmd.exe /V:ON /D /S /C "set /p x=Prompt: & echo Got:!x!""#,
            #[cfg(not(windows))]
            HostShellKind::PosixSh => "printf 'Prompt:'; read value; printf 'Got:%s\\n' \"$value\"",
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
            root.path(),
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
}
