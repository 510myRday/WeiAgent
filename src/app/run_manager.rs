use std::sync::{Arc, Mutex};

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard, broadcast, watch};
use uuid::Uuid;

use crate::{
    app::agent_executor::{AgentRunEvent, AgentRunPhase},
    domain::{AgentRun, AgentRunStatus},
    error::{AppError, AppResult},
};

const RUN_EVENT_BUFFER: usize = 512;

#[derive(Debug, Clone)]
pub struct SequencedRunEvent {
    pub sequence: u64,
    pub event: AgentRunEvent,
}

struct RunState {
    info: AgentRun,
    events: Vec<SequencedRunEvent>,
}

struct RunRecord {
    state: Mutex<RunState>,
    events: broadcast::Sender<SequencedRunEvent>,
    cancel: watch::Sender<bool>,
    finished: Notify,
}

pub struct RunRegistration {
    pub info: AgentRun,
    pub cancel: watch::Receiver<bool>,
    _guard: OwnedMutexGuard<()>,
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
        self.runs.insert(
            id,
            Arc::new(RunRecord {
                state: Mutex::new(RunState {
                    info: info.clone(),
                    events: Vec::new(),
                }),
                events: event_sender,
                cancel: cancel_sender,
                finished: Notify::new(),
            }),
        );
        self.active_by_conversation.insert(conversation_id, id);
        Ok(RunRegistration {
            info,
            cancel: cancel_receiver,
            _guard: guard,
        })
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
            state.events.push(sequenced.clone());
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
}

fn phase_name(phase: AgentRunPhase) -> &'static str {
    match phase {
        AgentRunPhase::WaitingForModel => "waiting_for_model",
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
