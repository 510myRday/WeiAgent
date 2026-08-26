use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard, broadcast, mpsc, watch};
use uuid::Uuid;

use crate::{
    app::agent_executor::{AgentRunEvent, AgentRunPhase},
    domain::{AgentRun, AgentRunStatus},
    error::{AppError, AppResult},
};

const RUN_EVENT_BUFFER: usize = 512;
const MAX_REPLAY_EVENTS: usize = 16_384;
const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;
const MAX_RETAINED_FINISHED_RUNS: usize = 64;
const FINISHED_RUN_RETENTION_MINUTES: i64 = 10;
const CANCEL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct SequencedRunEvent {
    pub sequence: u64,
    pub event: AgentRunEvent,
}

struct RunState {
    info: AgentRun,
    events: VecDeque<SequencedRunEvent>,
    event_bytes: usize,
}

struct RunRecord {
    state: Mutex<RunState>,
    events: broadcast::Sender<SequencedRunEvent>,
    cancel: watch::Sender<bool>,
    steer: mpsc::UnboundedSender<SteerMessage>,
    finished: Notify,
}

pub struct RunRegistration {
    pub info: AgentRun,
    pub cancel: watch::Receiver<bool>,
    steer: Option<mpsc::UnboundedReceiver<SteerMessage>>,
    _guard: OwnedMutexGuard<()>,
}

#[derive(Debug, Clone)]
pub struct SteerMessage {
    pub content: String,
}

pub struct RunSubscription {
    pub replay: Vec<SequencedRunEvent>,
    pub receiver: broadcast::Receiver<SequencedRunEvent>,
    pub terminal: bool,
}

pub struct RunManager {
    conversation_locks: DashMap<Uuid, Arc<AsyncMutex<()>>>,
    runs: DashMap<Uuid, Arc<RunRecord>>,
    active_by_conversation: DashMap<Uuid, Uuid>,
}

impl RunManager {
    pub fn new() -> Self {
        Self {
            conversation_locks: DashMap::new(),
            runs: DashMap::new(),
            active_by_conversation: DashMap::new(),
        }
    }

    pub fn try_start(&self, conversation_id: Uuid) -> AppResult<OwnedMutexGuard<()>> {
        self.cleanup();
        let lock = self
            .conversation_locks
            .entry(conversation_id)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        lock.try_lock_owned().map_err(|_| {
            AppError::Conflict("a run is already active for this conversation".to_string())
        })
    }

    pub fn create(&self, conversation_id: Uuid) -> AppResult<RunRegistration> {
        let guard = self.try_start(conversation_id)?;
        let id = Uuid::new_v4();
        let info = AgentRun {
            id,
            conversation_id,
            user_message_id: None,
            status: AgentRunStatus::Running,
            phase: Some("waiting_for_model".to_string()),
            latest_sequence: 0,
            error: None,
            created_at: Utc::now(),
            finished_at: None,
        };
        let (event_sender, _) = broadcast::channel(RUN_EVENT_BUFFER);
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        let (steer_sender, steer_receiver) = mpsc::unbounded_channel();
        self.runs.insert(
            id,
            Arc::new(RunRecord {
                state: Mutex::new(RunState {
                    info: info.clone(),
                    events: VecDeque::new(),
                    event_bytes: 0,
                }),
                events: event_sender,
                cancel: cancel_sender,
                steer: steer_sender,
                finished: Notify::new(),
            }),
        );
        self.active_by_conversation.insert(conversation_id, id);
        Ok(RunRegistration {
            info,
            cancel: cancel_receiver,
            steer: Some(steer_receiver),
            _guard: guard,
        })
    }

    pub fn take_steer_receiver(
        &self,
        registration: &mut RunRegistration,
    ) -> mpsc::UnboundedReceiver<SteerMessage> {
        registration
            .steer
            .take()
            .expect("run steer receiver can only be taken once")
    }

    pub fn steer(&self, run_id: Uuid, message: SteerMessage) -> AppResult<()> {
        let record = self
            .runs
            .get(&run_id)
            .map(|entry| entry.clone())
            .ok_or(AppError::NotFound)?;
        let info = record.state.lock().unwrap().info.clone();
        if info.status.is_terminal() {
            return Err(AppError::Conflict(
                "the run has already finished".to_string(),
            ));
        }
        record
            .steer
            .send(message)
            .map_err(|_| AppError::Conflict("the run is no longer accepting guidance".to_string()))
    }

    pub fn set_user_message(&self, run_id: Uuid, message_id: Uuid) {
        if let Some(record) = self.runs.get(&run_id) {
            record.state.lock().unwrap().info.user_message_id = Some(message_id);
        }
    }

    pub fn record_event(&self, run_id: Uuid, event: AgentRunEvent) {
        let Some(record) = self.runs.get(&run_id).map(|entry| entry.clone()) else {
            return;
        };
        let sequenced = {
            let mut state = record.state.lock().unwrap();
            state.info.latest_sequence += 1;
            if let AgentRunEvent::Phase { phase, .. } = &event {
                state.info.phase = Some(phase_name(*phase).to_string());
            }
            let sequenced = SequencedRunEvent {
                sequence: state.info.latest_sequence,
                event,
            };
            state.event_bytes = state
                .event_bytes
                .saturating_add(run_event_size(&sequenced.event));
            state.events.push_back(sequenced.clone());
            trim_replay_events(&mut state);
            sequenced
        };
        let _ = record.events.send(sequenced);
    }

    pub fn finish(&self, run_id: Uuid, status: AgentRunStatus, error: Option<String>) {
        let Some(record) = self.runs.get(&run_id).map(|entry| entry.clone()) else {
            return;
        };
        let conversation_id = {
            let mut state = record.state.lock().unwrap();
            state.info.status = status;
            state.info.phase = None;
            state.info.error = error;
            state.info.finished_at = Some(Utc::now());
            state.info.conversation_id
        };
        self.active_by_conversation
            .remove_if(&conversation_id, |_, active_id| *active_id == run_id);
        record.finished.notify_waiters();
        self.cleanup();
    }

    pub fn get(&self, run_id: Uuid) -> AppResult<AgentRun> {
        self.runs
            .get(&run_id)
            .map(|record| record.state.lock().unwrap().info.clone())
            .ok_or(AppError::NotFound)
    }

    pub fn active_for_conversation(&self, conversation_id: Uuid) -> Option<AgentRun> {
        let run_id = *self.active_by_conversation.get(&conversation_id)?;
        self.get(run_id).ok()
    }

    pub fn active_for_conversations(&self, conversation_ids: &[Uuid]) -> Vec<AgentRun> {
        conversation_ids
            .iter()
            .filter_map(|conversation_id| self.active_for_conversation(*conversation_id))
            .collect()
    }

    pub fn subscribe(&self, run_id: Uuid, after: u64) -> AppResult<RunSubscription> {
        let record = self
            .runs
            .get(&run_id)
            .map(|entry| entry.clone())
            .ok_or(AppError::NotFound)?;
        let receiver = record.events.subscribe();
        let state = record.state.lock().unwrap();
        Ok(RunSubscription {
            replay: state
                .events
                .iter()
                .filter(|event| event.sequence > after)
                .cloned()
                .collect(),
            receiver,
            terminal: state.info.status.is_terminal(),
        })
    }

    pub fn cancel(&self, run_id: Uuid) -> AppResult<AgentRun> {
        let record = self
            .runs
            .get(&run_id)
            .map(|entry| entry.clone())
            .ok_or(AppError::NotFound)?;
        let info = record.state.lock().unwrap().info.clone();
        if !info.status.is_terminal() {
            let _ = record.cancel.send(true);
        }
        Ok(info)
    }

    pub async fn cancel_and_wait(&self, run_id: Uuid) -> AppResult<AgentRun> {
        self.cancel(run_id)?;
        tokio::time::timeout(CANCEL_WAIT_TIMEOUT, async {
            loop {
                let record = self
                    .runs
                    .get(&run_id)
                    .map(|entry| entry.clone())
                    .ok_or(AppError::NotFound)?;
                let notified = record.finished.notified();
                let info = record.state.lock().unwrap().info.clone();
                if info.status.is_terminal() {
                    return Ok(info);
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| AppError::Process("timed out waiting for run cancellation".to_string()))?
    }

    pub async fn wait_for_terminal(&self, run_id: Uuid) -> AppResult<AgentRun> {
        let record = self
            .runs
            .get(&run_id)
            .map(|entry| entry.clone())
            .ok_or(AppError::NotFound)?;
        loop {
            let notified = record.finished.notified();
            let info = record.state.lock().unwrap().info.clone();
            if info.status.is_terminal() {
                return Ok(info);
            }
            notified.await;
        }
    }

    pub async fn cancel_conversation_and_wait(&self, conversation_id: Uuid) -> AppResult<()> {
        let Some(run_id) = self
            .active_by_conversation
            .get(&conversation_id)
            .map(|entry| *entry)
        else {
            return Ok(());
        };
        self.cancel_and_wait(run_id).await?;
        Ok(())
    }

    fn cleanup(&self) {
        let cutoff = Utc::now() - chrono::Duration::minutes(FINISHED_RUN_RETENTION_MINUTES);
        let mut finished = self
            .runs
            .iter()
            .filter_map(|entry| {
                let state = entry.state.lock().ok()?;
                state
                    .info
                    .finished_at
                    .map(|finished_at| (*entry.key(), finished_at))
            })
            .collect::<Vec<_>>();

        for (run_id, finished_at) in &finished {
            if *finished_at < cutoff {
                self.runs.remove(run_id);
            }
        }
        finished.retain(|(run_id, _)| self.runs.contains_key(run_id));
        if finished.len() > MAX_RETAINED_FINISHED_RUNS {
            finished.sort_by_key(|(_, finished_at)| *finished_at);
            let remove_count = finished.len() - MAX_RETAINED_FINISHED_RUNS;
            for (run_id, _) in finished.into_iter().take(remove_count) {
                self.runs.remove(&run_id);
            }
        }

        self.conversation_locks.retain(|conversation_id, lock| {
            self.active_by_conversation.contains_key(conversation_id) || Arc::strong_count(lock) > 1
        });
    }
}

fn trim_replay_events(state: &mut RunState) {
    while state.events.len() > MAX_REPLAY_EVENTS || state.event_bytes > MAX_REPLAY_BYTES {
        let Some(event) = state.events.pop_front() else {
            break;
        };
        state.event_bytes = state
            .event_bytes
            .saturating_sub(run_event_size(&event.event));
    }
}

fn run_event_size(event: &AgentRunEvent) -> usize {
    match event {
        AgentRunEvent::User(message) | AgentRunEvent::Assistant(message) => {
            message.content.len() + message.tool_calls.as_ref().map_or(0, estimate_json_bytes)
        }
        AgentRunEvent::Usage(usage) => serde_json::to_vec(usage).map_or(0, |value| value.len()),
        AgentRunEvent::Delta(delta)
        | AgentRunEvent::ReasoningDelta(delta)
        | AgentRunEvent::Error(delta) => delta.len(),
        AgentRunEvent::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => call_id.len() + name.len() + estimate_json_bytes(arguments),
        AgentRunEvent::ToolCallStarted { call_id, name, .. } => call_id.len() + name.len(),
        AgentRunEvent::ToolCallArgsDelta { call_id, arguments } => call_id.len() + arguments.len(),
        AgentRunEvent::ToolResult { call_id, result } => call_id.len() + tool_result_size(result),
        AgentRunEvent::ApprovalRequired {
            call_id,
            command,
            approval_id,
            cwd,
            justification,
            ..
        } => {
            call_id.len()
                + command.len()
                + approval_id.len()
                + cwd.as_deref().map_or(0, str::len)
                + justification.as_deref().map_or(0, str::len)
        }
        AgentRunEvent::Phase { call_id, .. } => call_id.as_deref().map_or(0, str::len),
        AgentRunEvent::Retry { reason, .. } => reason.len(),
        AgentRunEvent::Done | AgentRunEvent::Cancelled => 0,
    }
}

/// Approximate serialized size without allocating; the replay budget only
/// needs a rough byte count, not the exact JSON length.
fn estimate_json_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    match value {
        Value::Null => 4,
        Value::Bool(_) => 5,
        Value::Number(_) => 12,
        Value::String(text) => text.len() + 2,
        Value::Array(values) => 2 + values.iter().map(estimate_json_bytes).sum::<usize>(),
        Value::Object(map) => {
            2 + map
                .iter()
                .map(|(key, value)| key.len() + 4 + estimate_json_bytes(value))
                .sum::<usize>()
        }
    }
}

fn tool_result_size(result: &crate::domain::ToolResult) -> usize {
    use crate::domain::ToolResult;
    const FIXED_OVERHEAD: usize = 64;
    FIXED_OVERHEAD
        + match result {
            ToolResult::Completed {
                stdout,
                stderr,
                runtime,
                ..
            } => stdout.len() + stderr.len() + runtime.as_deref().map_or(0, str::len),
            ToolResult::Running {
                process_id,
                message,
                stdout,
                stderr,
                runtime,
                ..
            } => {
                process_id.len()
                    + message.len()
                    + stdout.len()
                    + stderr.len()
                    + runtime.as_deref().map_or(0, str::len)
            }
            ToolResult::Blocked { reason, .. }
            | ToolResult::Rejected { reason }
            | ToolResult::Interrupted { reason } => reason.len(),
            ToolResult::PendingApproval {
                command,
                approval_id,
            } => command.len() + approval_id.len(),
            ToolResult::Error { message } => message.len(),
        }
}

fn phase_name(phase: AgentRunPhase) -> &'static str {
    match phase {
        AgentRunPhase::WaitingForModel => "waiting_for_model",
        AgentRunPhase::CompactingContext => "compacting_context",
        AgentRunPhase::ExecutingTool => "executing_tool",
        AgentRunPhase::WaitingForApproval => "waiting_for_approval",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_concurrent_runs_for_the_same_conversation() {
        let manager = RunManager::new();
        let conversation_id = Uuid::new_v4();
        let first = manager.create(conversation_id).unwrap();
        assert!(manager.create(conversation_id).is_err());
        manager.finish(first.info.id, AgentRunStatus::Completed, None);
        drop(first);
        assert!(manager.create(conversation_id).is_ok());
    }

    #[test]
    fn replays_events_after_requested_sequence() {
        let manager = RunManager::new();
        let registration = manager.create(Uuid::new_v4()).unwrap();
        manager.record_event(
            registration.info.id,
            AgentRunEvent::Delta("one".to_string()),
        );
        manager.record_event(
            registration.info.id,
            AgentRunEvent::Delta("two".to_string()),
        );
        let subscription = manager.subscribe(registration.info.id, 1).unwrap();
        assert_eq!(subscription.replay.len(), 1);
        assert_eq!(subscription.replay[0].sequence, 2);
        assert!(matches!(
            &subscription.replay[0].event,
            AgentRunEvent::Delta(delta) if delta == "two"
        ));
    }

    #[test]
    fn bounds_replay_history() {
        let manager = RunManager::new();
        let registration = manager.create(Uuid::new_v4()).unwrap();
        for iteration in 0..(MAX_REPLAY_EVENTS + 10) {
            manager.record_event(
                registration.info.id,
                AgentRunEvent::Phase {
                    phase: AgentRunPhase::WaitingForModel,
                    iteration,
                    call_id: None,
                },
            );
        }
        let subscription = manager.subscribe(registration.info.id, 0).unwrap();
        assert_eq!(subscription.replay.len(), MAX_REPLAY_EVENTS);
        assert!(subscription.replay[0].sequence > 1);
    }

    #[test]
    fn evicts_old_finished_runs_and_idle_conversation_locks() {
        let manager = RunManager::new();
        let first_conversation_id = Uuid::new_v4();
        for index in 0..(MAX_RETAINED_FINISHED_RUNS + 2) {
            let conversation_id = if index == 0 {
                first_conversation_id
            } else {
                Uuid::new_v4()
            };
            let registration = manager.create(conversation_id).unwrap();
            manager.finish(registration.info.id, AgentRunStatus::Completed, None);
            drop(registration);
        }
        manager.cleanup();
        assert!(manager.runs.len() <= MAX_RETAINED_FINISHED_RUNS);
        assert!(
            !manager
                .conversation_locks
                .contains_key(&first_conversation_id)
        );
    }

    #[tokio::test]
    async fn cancellation_notifies_the_background_run() {
        let manager = RunManager::new();
        let mut registration = manager.create(Uuid::new_v4()).unwrap();
        manager.cancel(registration.info.id).unwrap();
        registration.cancel.changed().await.unwrap();
        assert!(*registration.cancel.borrow());
    }
}
