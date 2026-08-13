use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy)]
pub enum TerminateMode {
    /// Force-kill the process tree (Windows) or the process itself (Unix).
    Force,
    /// Send SIGTERM to the process group, then SIGKILL. Windows still force-kills.
    GroupGraceful,
}

pub async fn terminate_process_tree(pid: u32) -> AppResult<()> {
    terminate_process(pid, TerminateMode::Force).await
}

pub async fn terminate_process(pid: u32, mode: TerminateMode) -> AppResult<()> {
    #[cfg(windows)]
    {
        let _ = mode;
        let status = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .await?;
        if status.success() {
            return Ok(());
        }
        return Err(AppError::Process(format!(
            "taskkill failed for process tree rooted at {pid} with status {status}"
        )));
    }

    #[cfg(unix)]
    {
        match mode {
            TerminateMode::Force => {
                let status = tokio::process::Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()
                    .await?;
                if status.success() {
                    return Ok(());
                }
                return Err(AppError::Process(format!(
                    "failed to terminate process {pid} with status {status}"
                )));
            }
            TerminateMode::GroupGraceful => {
                let group = format!("-{pid}");
                let term = tokio::process::Command::new("kill")
                    .args(["-TERM", &group])
                    .status()
                    .await?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let kill = tokio::process::Command::new("kill")
                    .args(["-KILL", &group])
                    .status()
                    .await?;
                if term.success() || kill.success() {
                    return Ok(());
                }
                return Err(AppError::Process(format!(
                    "failed to terminate Unix process group rooted at {pid}"
                )));
            }
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = (pid, mode);
        Ok(())
    }
}
