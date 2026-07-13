use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub struct RunManager {
    conversations: DashMap<Uuid, Arc<Mutex<()>>>,
}

impl RunManager {
    pub fn new() -> Self {
        Self {
            conversations: DashMap::new(),
        }
    }

    pub fn try_start(&self, conversation_id: Uuid) -> AppResult<OwnedMutexGuard<()>> {
        let lock = self
            .conversations
            .entry(conversation_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        lock.try_lock_owned().map_err(|_| {
            AppError::Conflict("a run is already active for this conversation".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_concurrent_runs_for_the_same_conversation() {
        let manager = RunManager::new();
        let conversation_id = Uuid::new_v4();
        let first = manager.try_start(conversation_id).unwrap();
        assert!(manager.try_start(conversation_id).is_err());
        drop(first);
        assert!(manager.try_start(conversation_id).is_ok());
    }
}
