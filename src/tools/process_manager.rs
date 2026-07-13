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

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub tool_call_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub docker_container_name: Option<String>,
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
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            processes: DashMap::new(),
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

        let result = if let Some(container_name) = &record.metadata.docker_container_name {
            Command::new("docker")
                .args(["stop", "--time", "1", container_name])
                .output()
                .await
        } else if cfg!(windows) {
            Command::new("taskkill")
                .args(["/PID", &record.pid.to_string(), "/T", "/F"])
                .output()
                .await
        } else {
            Command::new("kill")
                .args(["-TERM", &format!("-{}", record.pid)])
                .output()
                .await
        };

        if let Err(error) = result {
            record.stop_requested.store(false, Ordering::SeqCst);
            return Err(AppError::Storage(error));
        }

        Ok(self
            .wait_for_completion(id, Duration::from_secs(5))
            .await
            .or(Some(current)))
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
    for byte in data {
        if buffer.len() >= MAX_OUTPUT_BYTES {
            buffer.pop_front();
        }
        buffer.push_back(*byte);
    }
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
