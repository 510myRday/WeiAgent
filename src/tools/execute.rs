use std::{path::Path, time::Instant};

use tokio::process::Command;
use uuid::Uuid;

use crate::{
    domain::{CommandDecision, TerminalToolConfig, ToolResult},
    tools::{process_manager::ProcessManager, terminal::CommandChecker},
};

pub struct ExecuteTerminalRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub background: Option<bool>,
    pub approved: bool,
}

const MAX_CAPTURE_BYTES: usize = 1_000_000;

pub async fn execute_terminal(
    req: &ExecuteTerminalRequest,
    config: &TerminalToolConfig,
    process_manager: &ProcessManager,
) -> ToolResult {
    if !config.enabled {
        return ToolResult::Blocked {
            reason: "Terminal tool is disabled".to_string(),
        };
    }

    if req.command.trim().is_empty() {
        return ToolResult::Blocked {
            reason: "Command cannot be empty".to_string(),
        };
    }

    if req.command.len() > 8_000 {
        return ToolResult::Blocked {
            reason: "Command is too long".to_string(),
        };
    }

    // cwd 安全检查
    if let Err(reason) = validate_cwd(req.cwd.as_deref(), config) {
        return ToolResult::Blocked { reason };
    }

    fn truncate_output(bytes: &[u8]) -> String {
        if bytes.len() <= MAX_CAPTURE_BYTES {
            return String::from_utf8_lossy(bytes).to_string();
        }
        let start = bytes.len().saturating_sub(MAX_CAPTURE_BYTES);
        format!(
            "[output truncated to last {} bytes]\n{}",
            MAX_CAPTURE_BYTES,
            String::from_utf8_lossy(&bytes[start..])
        )
    }

    // 命令安全检查
    let checker = CommandChecker::new(config);
    match checker.check(&req.command) {
        CommandDecision::Blocked(reason) => return ToolResult::Blocked { reason },
        CommandDecision::NeedsApproval if !req.approved => {
            return ToolResult::PendingApproval {
                command: req.command.clone(),
                approval_id: Uuid::new_v4().to_string(),
            };
        }
        CommandDecision::NeedsApproval => {}
        CommandDecision::Allowed => {}
    }

    // 执行命令
    let timeout_ms = req.timeout_ms.unwrap_or(config.default_timeout_ms);
    let force_background = req.background.unwrap_or(false);
    let started = Instant::now();

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &req.command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &req.command]);
        c
    };

    if let Some(ref cwd) = req.cwd {
        cmd.current_dir(cwd);
    }

    if force_background {
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
    } else {
        cmd.kill_on_drop(true);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
    }

    match cmd.spawn() {
        Ok(child) => {
            if force_background {
                let pid = Uuid::new_v4().to_string();
                process_manager.register(pid.clone(), child);
                return ToolResult::Running {
                    process_id: pid,
                    message: "Command started in background without output capture".to_string(),
                };
            }

            // 等待完成或超时
            let output = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                child.wait_with_output(),
            )
            .await;

            match output {
                Ok(Ok(output)) => ToolResult::Completed {
                    stdout: truncate_output(&output.stdout),
                    stderr: truncate_output(&output.stderr),
                    exit_code: output.status.code().unwrap_or(-1),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
                Ok(Err(e)) => ToolResult::Error {
                    message: format!("Failed to wait for process: {}", e),
                },
                Err(_) => ToolResult::Error {
                    message: format!(
                        "Command timed out after {}ms and was terminated",
                        timeout_ms
                    ),
                },
            }
        }
        Err(e) => ToolResult::Error {
            message: format!("Failed to spawn process: {}", e),
        },
    }
}

fn validate_cwd(cwd: Option<&str>, config: &TerminalToolConfig) -> Result<(), String> {
    if config.allowed_cwd_roots.is_empty() {
        return Ok(());
    }

    let cwd_path = match cwd {
        Some(cwd) => Path::new(cwd).to_path_buf(),
        None => std::env::current_dir()
            .map_err(|e| format!("Failed to resolve current directory: {e}"))?,
    };
    let cwd_canonical = std::fs::canonicalize(&cwd_path)
        .map_err(|e| format!("Working directory '{}' is invalid: {e}", cwd_path.display()))?;

    for root in &config.allowed_cwd_roots {
        let root_canonical = match std::fs::canonicalize(root) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if cwd_canonical.starts_with(root_canonical) {
            return Ok(());
        }
    }

    Err(format!(
        "Working directory '{}' is not in allowed roots",
        cwd_canonical.display()
    ))
}
