use crate::error::{AppError, AppResult};

pub async fn terminate_process_tree(pid: u32) -> AppResult<()> {
    #[cfg(windows)]
    {
        let status = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .await?;
        if status.success() {
            return Ok(());
        }
        return Err(AppError::Process(format!(
            "taskkill failed for MCP process tree rooted at {pid} with status {status}"
        )));
    }

    #[cfg(unix)]
    {
        let status = tokio::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .await?;
        if status.success() {
            return Ok(());
        }
        return Err(AppError::Process(format!(
            "failed to terminate MCP process {pid} with status {status}"
        )));
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        Ok(())
    }
}
