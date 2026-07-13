use std::{sync::Arc, time::Duration};

use dashmap::DashMap;
use tokio::sync::oneshot;

use crate::domain::{ApprovalDecision, PendingApproval};

struct PendingApprovalEntry {
    approval: PendingApproval,
    decision_tx: std::sync::Mutex<Option<oneshot::Sender<ApprovalDecision>>>,
}

pub struct ApprovalManager {
    pending: DashMap<String, PendingApprovalEntry>,
    ttl: Duration,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(600))
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            pending: DashMap::new(),
            ttl,
        }
    }

    pub fn register(
        self: &Arc<Self>,
        entry: PendingApproval,
    ) -> oneshot::Receiver<ApprovalDecision> {
        let approval_id = entry.approval_id.clone();
        let (decision_tx, decision_rx) = oneshot::channel();
        self.pending.insert(
            approval_id.clone(),
            PendingApprovalEntry {
                approval: entry,
                decision_tx: std::sync::Mutex::new(Some(decision_tx)),
            },
        );

        let manager = Arc::downgrade(self);
        let ttl = self.ttl;
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            if let Some(manager) = manager.upgrade() {
                manager.expire(&approval_id);
            }
        });
        decision_rx
    }

    pub fn resolve(
        &self,
        approval_id: &str,
        agent_id: uuid::Uuid,
        decision: ApprovalDecision,
    ) -> Option<PendingApproval> {
        if self
            .pending
            .get(approval_id)
            .is_none_or(|entry| entry.approval.agent_id != agent_id)
        {
            return None;
        }
        let (_, entry) = self.pending.remove(approval_id)?;
        send_decision(&entry, decision);
        Some(entry.approval)
    }

    pub fn cancel(&self, approval_id: &str) {
        self.pending.remove(approval_id);
    }
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub fn resolve_for_test_id(&self) -> Option<String> {
        self.pending.iter().next().map(|entry| entry.key().clone())
    }
    fn expire(&self, approval_id: &str) {
        if let Some((_, entry)) = self.pending.remove(approval_id) {
            send_decision(&entry, ApprovalDecision::Reject);
        }
    }
}

fn send_decision(entry: &PendingApprovalEntry, decision: ApprovalDecision) {
    if let Ok(mut sender) = entry.decision_tx.lock()
        && let Some(sender) = sender.take()
    {
        let _ = sender.send(decision);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approvals_expire_without_an_api_request() {
        let manager = Arc::new(ApprovalManager::with_ttl(Duration::from_millis(20)));
        let receiver = manager.register(PendingApproval {
            approval_id: "approval-1".to_string(),
            agent_id: uuid::Uuid::new_v4(),
            conversation_id: uuid::Uuid::new_v4(),
            tool_call_id: "call-1".to_string(),
            command: "echo test".to_string(),
            cwd: None,
            runtime: "docker".to_string(),
            network_enabled: false,
            write_roots: Vec::new(),
            background: false,
        });
        assert_eq!(manager.pending_count(), 1);
        assert_eq!(receiver.await.unwrap(), ApprovalDecision::Reject);
        assert_eq!(manager.pending_count(), 0);
    }

    #[tokio::test]
    async fn approvals_are_bound_to_the_owning_agent() {
        let manager = Arc::new(ApprovalManager::new());
        let agent_id = uuid::Uuid::new_v4();
        let receiver = manager.register(PendingApproval {
            approval_id: "approval-owned".to_string(),
            agent_id,
            conversation_id: uuid::Uuid::new_v4(),
            tool_call_id: "call-owned".to_string(),
            command: "echo owned".to_string(),
            cwd: None,
            runtime: "docker".to_string(),
            network_enabled: false,
            write_roots: Vec::new(),
            background: false,
        });
        assert!(
            manager
                .resolve(
                    "approval-owned",
                    uuid::Uuid::new_v4(),
                    ApprovalDecision::Approve
                )
                .is_none()
        );
        assert!(
            manager
                .resolve("approval-owned", agent_id, ApprovalDecision::Approve)
                .is_some()
        );
        assert_eq!(receiver.await.unwrap(), ApprovalDecision::Approve);
    }
}
