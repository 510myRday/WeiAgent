use std::collections::VecDeque;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::process::Child;
use tokio::sync::Mutex;

#[allow(dead_code)]
const MAX_OUTPUT_BYTES: usize = 1_000_000;

#[allow(dead_code)]
pub struct ProcessHandle {
    pub child: Arc<Mutex<Child>>,
    pub output: Arc<Mutex<VecDeque<u8>>>,
    pub started_at: std::time::Instant,
}

pub struct ProcessManager {
    processes: DashMap<String, ProcessHandle>,
}

#[allow(dead_code)]
impl ProcessManager {
    pub fn new() -> Self {
        Self {
            processes: DashMap::new(),
        }
    }

    pub fn register(&self, id: String, child: Child) {
        self.processes.insert(
            id,
            ProcessHandle {
                child: Arc::new(Mutex::new(child)),
                output: Arc::new(Mutex::new(VecDeque::new())),
                started_at: std::time::Instant::now(),
            },
        );
    }

    pub async fn append_output(&self, id: &str, data: &[u8]) {
        if let Some(handle) = self.processes.get(id) {
            let mut buf = handle.output.lock().await;
            for &b in data {
                if buf.len() >= MAX_OUTPUT_BYTES {
                    buf.pop_front();
                }
                buf.push_back(b);
            }
        }
    }

    pub async fn get_output(&self, id: &str) -> Option<String> {
        let handle = self.processes.get(id)?;
        let buf = handle.output.lock().await;
        Some(String::from_utf8_lossy(&buf.iter().copied().collect::<Vec<_>>()).to_string())
    }

    pub async fn stop(&self, id: &str) -> bool {
        // Kill the child process while holding the DashMap ref,
        // then drop the ref before removing from the map.
        {
            let handle = match self.processes.get(id) {
                Some(h) => h,
                None => return false,
            };
            let mut child = handle.child.lock().await;
            let _ = child.kill().await;
            // child (MutexGuard) dropped here
            // handle (DashMap Ref) dropped here
        }
        self.processes.remove(id);
        true
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.processes.contains_key(id)
    }

    pub fn elapsed_ms(&self, id: &str) -> Option<u64> {
        self.processes
            .get(id)
            .map(|h| h.started_at.elapsed().as_millis() as u64)
    }
}
