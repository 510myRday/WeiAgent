use std::{sync::Arc, time::Duration};

use dashmap::DashMap;
use tokio::sync::oneshot;

use crate::domain::{ApprovalDecision, PendingApproval};

struct PendingApprovalEntry {
    approval: PendingApproval,
    decision_tx: std::sync::Mutex<Option<oneshot::Sender<ApprovalDecision>>>,
}

struct ResolvedApprovalEntry {
    agent_id: uuid::Uuid,
    decision: ApprovalDecision,
}

pub enum ApprovalResolution {
    Resolved(Box<PendingApproval>),
    AlreadyResolved(ApprovalDecision),
    AgentMismatch,
    NotFound,
}

pub struct ApprovalManager {
    pending: DashMap<String, PendingApprovalEntry>,
    resolved: DashMap<String, ResolvedApprovalEntry>,
    ttl: Duration,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(600))
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            pending: DashMap::new(),
            resolved: DashMap::new(),
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
    ) -> ApprovalResolution {
        if let Some(entry) = self.pending.get(approval_id)
            && entry.approval.agent_id != agent_id
        {
            return ApprovalResolution::AgentMismatch;
        }
        if let Some((_, entry)) = self.pending.remove(approval_id) {
            self.resolved.insert(
                approval_id.to_string(),
                ResolvedApprovalEntry {
                    agent_id,
                    decision: decision.clone(),
                },
            );
            send_decision(&entry, decision);
            return ApprovalResolution::Resolved(Box::new(entry.approval));
        }
        match self.resolved.get(approval_id) {
            Some(entry) if entry.agent_id == agent_id => {
                ApprovalResolution::AlreadyResolved(entry.decision.clone())
            }
            Some(_) => ApprovalResolution::AgentMismatch,
            None => ApprovalResolution::NotFound,
        }
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
        self.resolved.remove(approval_id);
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
            runtime: "full_access".to_string(),
            permission: "full_access".to_string(),
            justification: Some("test approval".to_string()),
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
            runtime: "full_access".to_string(),
            permission: "full_access".to_string(),
            justification: Some("owned approval".to_string()),
            background: false,
        });
        assert!(matches!(
            manager.resolve(
                "approval-owned",
                uuid::Uuid::new_v4(),
                ApprovalDecision::Approve
            ),
            ApprovalResolution::AgentMismatch
        ));
        assert!(matches!(
            manager.resolve("approval-owned", agent_id, ApprovalDecision::Approve),
            ApprovalResolution::Resolved(_)
        ));
        assert_eq!(receiver.await.unwrap(), ApprovalDecision::Approve);
    }

    #[tokio::test]
    async fn duplicate_approval_requests_are_idempotent() {
        let manager = Arc::new(ApprovalManager::new());
        let agent_id = uuid::Uuid::new_v4();
        let receiver = manager.register(PendingApproval {
            approval_id: "approval-duplicate".to_string(),
            agent_id,
            conversation_id: uuid::Uuid::new_v4(),
            tool_call_id: "call-duplicate".to_string(),
            command: "echo duplicate".to_string(),
            cwd: None,
            runtime: "full_access".to_string(),
            permission: "full_access".to_string(),
            justification: Some("duplicate test".to_string()),
            background: false,
        });
        assert!(matches!(
            manager.resolve(
                "approval-duplicate",
                agent_id,
                ApprovalDecision::AlwaysApprove
            ),
            ApprovalResolution::Resolved(_)
        ));
        assert_eq!(receiver.await.unwrap(), ApprovalDecision::AlwaysApprove);
        assert!(matches!(
            manager.resolve(
                "approval-duplicate",
                agent_id,
                ApprovalDecision::AlwaysApprove
            ),
            ApprovalResolution::AlreadyResolved(ApprovalDecision::AlwaysApprove)
        ));
    }
}
