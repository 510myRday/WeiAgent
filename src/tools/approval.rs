use dashmap::DashMap;
use tokio::sync::oneshot;

use crate::domain::{ApprovalDecision, PendingApproval};

struct PendingApprovalEntry {
    approval: PendingApproval,
    decision_tx: std::sync::Mutex<Option<oneshot::Sender<ApprovalDecision>>>,
}

pub struct ApprovalManager {
    pending: DashMap<String, PendingApprovalEntry>,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: DashMap::new(),
        }
    }

    pub fn register(&self, entry: PendingApproval) -> oneshot::Receiver<ApprovalDecision> {
        let (decision_tx, decision_rx) = oneshot::channel();
        self.pending.insert(
            entry.approval_id.clone(),
            PendingApprovalEntry {
                approval: entry,
                decision_tx: std::sync::Mutex::new(Some(decision_tx)),
            },
        );
        decision_rx
    }

    pub fn resolve(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Option<PendingApproval> {
        let (_, entry) = self.pending.remove(approval_id)?;
        if let Ok(mut tx) = entry.decision_tx.lock() {
            if let Some(tx) = tx.take() {
                let _ = tx.send(decision);
            }
        }
        Some(entry.approval)
    }

    /// 清理超过 10 分钟的过期审批
    pub fn cleanup_expired(&self) {
        let max_age = std::time::Duration::from_secs(600);
        self.pending
            .retain(|_, v| v.approval.created_at.elapsed() < max_age);
    }
}
