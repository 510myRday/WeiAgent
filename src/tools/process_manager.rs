use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::{Mutex, Notify},
};
use uuid::Uuid;

use crate::{
    domain::{ProcessInfo, ProcessStatus},
    error::{AppError, AppResult},
};

const MAX_OUTPUT_BYTES: usize = 1_000_000;
const FINISHED_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_RUNNING_PROCESSES: usize = 32;
const MAX_RUNNING_PROCESSES_PER_AGENT: usize = 8;
const MAX_PROCESS_LIFETIME: Duration = Duration::from_secs(60 * 60);
const MAX_RETAINED_PROCESSES: usize = 256;

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub tool_call_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub docker_container_name: Option<String>,
    pub detached: bool,
}

struct ProcessState {
    status: ProcessStatus,
    exit_code: Option<i32>,
    finished_at: Option<DateTime<Utc>>,
    duration_ms: Option<u64>,
}

struct ProcessRecord {
    process_id: String,
    pid: u32,
    metadata: ProcessMetadata,
    stdout: Mutex<VecDeque<u8>>,
    stderr: Mutex<VecDeque<u8>>,
    state: Mutex<ProcessState>,
    started_at: DateTime<Utc>,
    started_instant: Instant,
    stop_requested: AtomicBool,
    changed: Notify,
}

pub struct ProcessManager {
    processes: DashMap<String, Arc<ProcessRecord>>,
    spawn_lock: Mutex<()>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            processes: DashMap::new(),
            spawn_lock: Mutex::new(()),
        }
    }

    pub fn new_process_id(&self) -> String {
        Uuid::new_v4().to_string()
    }

    pub async fn spawn(
        &self,
        process_id: String,
        mut command: Command,
        metadata: ProcessMetadata,
    ) -> AppResult<String> {
        let _spawn_guard = self.spawn_lock.lock().await;
        self.cleanup_finished().await;
        let running_total = self.running_count(None).await;
        if running_total >= MAX_RUNNING_PROCESSES {
            return Err(AppError::Process(format!(
                "global running process limit ({MAX_RUNNING_PROCESSES}) reached"
            )));
        }
        let running_for_agent = self.running_count(Some(metadata.agent_id)).await;
        if running_for_agent >= MAX_RUNNING_PROCESSES_PER_AGENT {
            return Err(AppError::Process(format!(
                "agent running process limit ({MAX_RUNNING_PROCESSES_PER_AGENT}) reached"
            )));
        }
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(false);

        let mut child = command.spawn().map_err(AppError::Storage)?;
        let pid = child
            .id()
            .ok_or_else(|| AppError::Llm("spawned process has no process id".to_string()))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let record = Arc::new(ProcessRecord {
            process_id: process_id.clone(),
            pid,
            metadata,
            stdout: Mutex::new(VecDeque::new()),
            stderr: Mutex::new(VecDeque::new()),
            state: Mutex::new(ProcessState {
                status: ProcessStatus::Running,
                exit_code: None,
                finished_at: None,
                duration_ms: None,
            }),
            started_at: Utc::now(),
            started_instant: Instant::now(),
            stop_requested: AtomicBool::new(false),
            changed: Notify::new(),
        });
        self.processes.insert(process_id.clone(), record.clone());

        let watchdog_record = record.clone();
        tokio::spawn(async move {
            tokio::time::sleep(MAX_PROCESS_LIFETIME).await;
            if watchdog_record.state.lock().await.status != ProcessStatus::Running {
                return;
            }
            watchdog_record.stop_requested.store(true, Ordering::SeqCst);
            let _ = terminate_record(&watchdog_record, true).await;
        });

        let stdout_task =
            stdout.map(|stdout| tokio::spawn(read_output(stdout, record.clone(), false)));
        let stderr_task =
            stderr.map(|stderr| tokio::spawn(read_output(stderr, record.clone(), true)));
        tokio::spawn(async move {
            let outcome = child.wait().await;
            if let Some(task) = stdout_task {
                let _ = task.await;
            }
            if let Some(task) = stderr_task {
                let _ = task.await;
            }
            let mut state = record.state.lock().await;
            state.finished_at = Some(Utc::now());
            state.duration_ms = Some(record.started_instant.elapsed().as_millis() as u64);
            match outcome {
                Ok(status) => {
                    state.exit_code = status.code();
                    state.status = if record.stop_requested.load(Ordering::SeqCst) {
                        ProcessStatus::Stopped
                    } else if status.success() {
                        ProcessStatus::Completed
                    } else {
                        ProcessStatus::Failed
                    };
                }
                Err(error) => {
                    state.status = ProcessStatus::Failed;
                    drop(state);
                    append_limited(
                        &record.stderr,
                        format!("\nFailed to wait for process: {error}").as_bytes(),
                    )
                    .await;
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
        let records: Vec<_> = self
            .processes
            .iter()
            .filter(|entry| agent_id.is_none_or(|id| entry.metadata.agent_id == id))
            .map(|entry| entry.value().clone())
            .collect();
        let mut result = Vec::with_capacity(records.len());
        for record in records {
            result.push(snapshot(&record).await);
        }
        result.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        result
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
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Some(current);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Some(snapshot(&record).await);
            }
        }
    }

    pub async fn stop(&self, id: &str) -> AppResult<Option<ProcessInfo>> {
        let Some(record) = self.record(id) else {
            return Ok(None);
        };
        let current = snapshot(&record).await;
        if current.status != ProcessStatus::Running {
            return Ok(Some(current));
        }
        record.stop_requested.store(true, Ordering::SeqCst);

        let output = match terminate_record(&record, false).await {
            Ok(output) => output,
            Err(error) => {
                record.stop_requested.store(false, Ordering::SeqCst);
                return Err(AppError::Storage(error));
            }
        };
        if !output.status.success() {
            record.stop_requested.store(false, Ordering::SeqCst);
            return Err(AppError::Process(format!(
                "failed to terminate process: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        if let Some(info) = self.wait_for_completion(id, Duration::from_secs(3)).await
            && info.status != ProcessStatus::Running
        {
            return Ok(Some(info));
        }

        #[cfg(unix)]
        {
            let output = Command::new("kill")
                .args(["-KILL", &format!("-{}", record.pid)])
                .output()
                .await?;
            if !output.status.success() {
                return Err(AppError::Process(format!(
                    "failed to force-kill process: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
        }

        Ok(self
            .wait_for_completion(id, Duration::from_secs(2))
            .await
            .or(Some(current)))
    }

    pub async fn stop_for_conversation(&self, conversation_id: Uuid) {
        let ids = self
            .processes
            .iter()
            .filter(|entry| {
                entry.metadata.conversation_id == conversation_id && !entry.metadata.detached
            })
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop(&id).await;
        }
    }

    async fn running_count(&self, agent_id: Option<Uuid>) -> usize {
        let records: Vec<_> = self
            .processes
            .iter()
            .filter(|entry| agent_id.is_none_or(|id| entry.metadata.agent_id == id))
            .map(|entry| entry.value().clone())
            .collect();
        let mut count = 0;
        for record in records {
            if record.state.lock().await.status == ProcessStatus::Running {
                count += 1;
            }
        }
        count
    }

    fn record(&self, id: &str) -> Option<Arc<ProcessRecord>> {
        self.processes.get(id).map(|entry| entry.value().clone())
    }

    async fn cleanup_finished(&self) {
        let records: Vec<_> = self
            .processes
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
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

    pub async fn cleanup_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
        loop {
            interval.tick().await;
            self.cleanup_finished().await;
        }
    }
}

async fn read_output<R>(mut reader: R, record: Arc<ProcessRecord>, is_stderr: bool)
where
    R: AsyncRead + Unpin,
{
    let mut chunk = vec![0_u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => {
                if is_stderr {
                    append_limited(&record.stderr, &chunk[..read]).await;
                } else {
                    append_limited(&record.stdout, &chunk[..read]).await;
                }
                record.changed.notify_waiters();
            }
            Err(error) => {
                append_limited(
                    &record.stderr,
                    format!("\nFailed to read process output: {error}").as_bytes(),
                )
                .await;
                break;
            }
        }
    }
}

async fn append_limited(buffer: &Mutex<VecDeque<u8>>, data: &[u8]) {
    let mut buffer = buffer.lock().await;
    buffer.extend(data.iter().copied());
    let excess = buffer.len().saturating_sub(MAX_OUTPUT_BYTES);
    if excess > 0 {
        buffer.drain(..excess);
    }
}

async fn terminate_record(
    record: &ProcessRecord,
    force: bool,
) -> std::io::Result<std::process::Output> {
    if let Some(container_name) = &record.metadata.docker_container_name {
        let mut command = Command::new("docker");
        if force {
            command.args(["kill", container_name]);
        } else {
            command.args(["stop", "--time", "1", container_name]);
        }
        return command.output().await;
    }
    if cfg!(windows) {
        return Command::new("taskkill")
            .args(["/PID", &record.pid.to_string(), "/T", "/F"])
            .output()
            .await;
    }
    Command::new("kill")
        .args([
            if force { "-KILL" } else { "-TERM" },
            &format!("-{}", record.pid),
        ])
        .output()
        .await
}

async fn snapshot(record: &ProcessRecord) -> ProcessInfo {
    let state = record.state.lock().await;
    let stdout = record.stdout.lock().await;
    let stderr = record.stderr.lock().await;
    ProcessInfo {
        process_id: record.process_id.clone(),
        agent_id: record.metadata.agent_id,
        conversation_id: record.metadata.conversation_id,
        tool_call_id: record.metadata.tool_call_id.clone(),
        command: record.metadata.command.clone(),
        cwd: record.metadata.cwd.clone(),
        status: state.status.clone(),
        stdout: String::from_utf8_lossy(&stdout.iter().copied().collect::<Vec<_>>()).to_string(),
        stderr: String::from_utf8_lossy(&stderr.iter().copied().collect::<Vec<_>>()).to_string(),
        exit_code: state.exit_code,
        started_at: record.started_at,
        finished_at: state.finished_at,
        duration_ms: state
            .duration_ms
            .unwrap_or_else(|| record.started_instant.elapsed().as_millis() as u64),
    }
}
