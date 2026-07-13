use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::process::Command;
use uuid::Uuid;

use crate::{
    domain::{
        CommandDecision, ProcessInfo, ProcessStatus, SandboxMode, TerminalToolConfig, ToolResult,
    },
    tools::{
        process_manager::{ProcessManager, ProcessMetadata},
        terminal::CommandChecker,
    },
};

pub struct ExecuteTerminalRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub background: Option<bool>,
    pub approved: bool,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub tool_call_id: String,
}

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

    let checker = CommandChecker::new(config);
    match checker.check(&req.command) {
        CommandDecision::Blocked(reason) => return ToolResult::Blocked { reason },
        CommandDecision::NeedsApproval if !req.approved => {
            return ToolResult::PendingApproval {
                command: req.command.clone(),
                approval_id: Uuid::new_v4().to_string(),
            };
        }
        CommandDecision::NeedsApproval | CommandDecision::Allowed => {}
    }

    let workspace = match resolve_workspace(req.cwd.as_deref(), config) {
        Ok(workspace) => workspace,
        Err(reason) => return ToolResult::Blocked { reason },
    };
    let process_id = process_manager.new_process_id();
    let docker_available = if config.sandbox.mode == SandboxMode::Docker {
        docker_runtime_available().await
    } else {
        false
    };
    let launch = match build_sandbox_command(
        &process_id,
        &req.command,
        &workspace,
        config,
        docker_available,
    ) {
        Ok(command) => command,
        Err(message) => return ToolResult::Error { message },
    };

    let metadata = ProcessMetadata {
        agent_id: req.agent_id,
        conversation_id: req.conversation_id,
        tool_call_id: req.tool_call_id.clone(),
        command: req.command.clone(),
        cwd: Some(workspace.cwd.to_string_lossy().to_string()),
        docker_container_name: launch.docker_container_name,
    };
    if let Err(error) = process_manager
        .spawn(process_id.clone(), launch.command, metadata)
        .await
    {
        return ToolResult::Error {
            message: format!("Failed to start sandboxed command: {error}"),
        };
    }

    if req.background.unwrap_or(false) {
        return ToolResult::Running {
            process_id,
            message: format!(
                "Command started in background with output capture ({})",
                launch.runtime_label
            ),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            runtime: Some(launch.runtime_label),
        };
    }

    let timeout_ms = req.timeout_ms.unwrap_or(config.default_timeout_ms).max(1);
    match process_manager
        .wait_for_completion(&process_id, Duration::from_millis(timeout_ms))
        .await
    {
        Some(info) if info.status == ProcessStatus::Running => ToolResult::Running {
            process_id,
            message: format!(
                "Command exceeded {timeout_ms}ms and continues in background with output capture ({})",
                launch.runtime_label
            ),
            stdout: info.stdout,
            stderr: info.stderr,
            duration_ms: info.duration_ms,
            runtime: Some(launch.runtime_label),
        },
        Some(info) => process_info_to_result_with_runtime(info, Some(launch.runtime_label)),
        None => ToolResult::Error {
            message: "Process disappeared before completion".to_string(),
        },
    }
}

pub fn process_info_to_result(info: ProcessInfo) -> ToolResult {
    process_info_to_result_with_runtime(info, None)
}

pub fn process_info_to_result_with_runtime(
    info: ProcessInfo,
    runtime: Option<String>,
) -> ToolResult {
    match info.status {
        ProcessStatus::Completed | ProcessStatus::Failed => ToolResult::Completed {
            stdout: info.stdout,
            stderr: info.stderr,
            exit_code: info.exit_code.unwrap_or(-1),
            duration_ms: info.duration_ms,
            runtime,
        },
        ProcessStatus::Running => ToolResult::Running {
            process_id: info.process_id,
            message: "Process is still running".to_string(),
            stdout: info.stdout,
            stderr: info.stderr,
            duration_ms: info.duration_ms,
            runtime,
        },
        ProcessStatus::Stopped => ToolResult::Rejected {
            reason: "Process was stopped".to_string(),
        },
    }
}

struct ResolvedWorkspace {
    root: PathBuf,
    cwd: PathBuf,
    container_cwd: String,
}

struct PreparedLaunch {
    command: Command,
    docker_container_name: Option<String>,
    runtime_label: String,
}

fn resolve_workspace(
    cwd: Option<&str>,
    config: &TerminalToolConfig,
) -> Result<ResolvedWorkspace, String> {
    let roots = if config.allowed_cwd_roots.is_empty() {
        vec![
            std::env::current_dir()
                .map_err(|error| format!("Failed to resolve server working directory: {error}"))?,
        ]
    } else {
        config.allowed_cwd_roots.iter().map(PathBuf::from).collect()
    };
    let roots: Vec<PathBuf> = roots
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    if roots.is_empty() {
        return Err("No valid terminal workspace root is configured".to_string());
    }

    let requested = cwd.map(PathBuf::from).unwrap_or_else(|| roots[0].clone());
    let canonical_cwd = std::fs::canonicalize(&requested).map_err(|error| {
        format!(
            "Working directory '{}' is invalid: {error}",
            requested.display()
        )
    })?;
    let root = roots
        .into_iter()
        .find(|root| canonical_cwd.starts_with(root))
        .ok_or_else(|| {
            format!(
                "Working directory '{}' is outside the configured sandbox roots",
                canonical_cwd.display()
            )
        })?;
    let relative = canonical_cwd
        .strip_prefix(&root)
        .map_err(|_| "Failed to calculate sandbox working directory".to_string())?;
    let relative = path_to_container(relative);
    let container_cwd = if relative.is_empty() {
        "/workspace".to_string()
    } else {
        format!("/workspace/{relative}")
    };
    Ok(ResolvedWorkspace {
        root,
        cwd: canonical_cwd,
        container_cwd,
    })
}

fn build_sandbox_command(
    process_id: &str,
    shell_command: &str,
    workspace: &ResolvedWorkspace,
    config: &TerminalToolConfig,
    docker_available: bool,
) -> Result<PreparedLaunch, String> {
    match config.sandbox.mode {
        SandboxMode::Docker if docker_available => {
            let (command, container_name) =
                build_docker_command(process_id, shell_command, workspace, config)?;
            Ok(PreparedLaunch {
                command,
                docker_container_name: container_name,
                runtime_label: "docker".to_string(),
            })
        }
        SandboxMode::Docker => Ok(PreparedLaunch {
            command: build_native_restricted_command(shell_command, &workspace.cwd),
            docker_container_name: None,
            runtime_label: "native_restricted (docker unavailable fallback)".to_string(),
        }),
        SandboxMode::NativeRestricted => Ok(PreparedLaunch {
            command: build_native_restricted_command(shell_command, &workspace.cwd),
            docker_container_name: None,
            runtime_label: "native_restricted".to_string(),
        }),
    }
}

async fn docker_runtime_available() -> bool {
    let mut command = Command::new("docker");
    command
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.status().await {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

fn build_docker_command(
    process_id: &str,
    shell_command: &str,
    workspace: &ResolvedWorkspace,
    config: &TerminalToolConfig,
) -> Result<(Command, Option<String>), String> {
    let image = config.sandbox.docker_image.trim();
    if image.is_empty() {
        return Err("Docker sandbox image cannot be empty".to_string());
    }
    if config.sandbox.cpu_limit <= 0.0 || config.sandbox.memory_limit_mb == 0 {
        return Err("Docker sandbox CPU and memory limits must be greater than zero".to_string());
    }

    let container_name = format!("weiagent-{}", process_id.replace('-', ""));
    let mount_mode = if config.sandbox.workspace_read_only {
        "ro"
    } else {
        "rw"
    };
    let mount = format!("{}:/workspace:{mount_mode}", workspace.root.display());
    let mut command = Command::new("docker");
    command.args([
        "run",
        "--rm",
        "--name",
        &container_name,
        "--network",
        if config.sandbox.network_enabled {
            "bridge"
        } else {
            "none"
        },
        "--memory",
        &format!("{}m", config.sandbox.memory_limit_mb),
        "--cpus",
        &config.sandbox.cpu_limit.to_string(),
        "--pids-limit",
        &config.sandbox.pids_limit.max(1).to_string(),
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=64m",
        "-v",
        &mount,
        "-w",
        &workspace.container_cwd,
        image,
        "sh",
        "-lc",
        shell_command,
    ]);
    configure_process_group(&mut command);
    Ok((command, Some(container_name)))
}

fn build_native_restricted_command(shell_command: &str, cwd: &Path) -> Command {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C", shell_command]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-lc", shell_command]);
        command
    };
    command.current_dir(cwd);
    sanitize_environment(&mut command);
    configure_process_group(&mut command);
    command
}

fn sanitize_environment(command: &mut Command) {
    let allowed = [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
        "LANG",
        "LC_ALL",
    ];
    let preserved: Vec<_> = allowed
        .iter()
        .filter_map(|key| std::env::var_os(key).map(|value| ((*key).to_string(), value)))
        .collect();
    command.env_clear();
    for (key, value) in preserved {
        command.env(key, value);
    }
    command.env("WEIAGENT_SANDBOX", "native_restricted");
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

fn path_to_container(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_sandbox_is_the_secure_default_and_has_hardening_flags() {
        let root = tempfile::tempdir().unwrap();
        let workspace = ResolvedWorkspace {
            root: root.path().to_path_buf(),
            cwd: root.path().to_path_buf(),
            container_cwd: "/workspace".to_string(),
        };
        let config = TerminalToolConfig::default();
        assert_eq!(config.sandbox.mode, SandboxMode::Docker);
        let (command, container_name) =
            build_docker_command("test-process", "echo sandbox", &workspace, &config).unwrap();
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(container_name.is_some());
        assert!(args.windows(2).any(|pair| pair == ["--network", "none"]));
        assert!(args.windows(2).any(|pair| pair == ["--cap-drop", "ALL"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--security-opt", "no-new-privileges"])
        );
        assert!(args.iter().any(|arg| arg == "--read-only"));
        assert!(args.windows(2).any(|pair| pair[0] == "--memory"));
        assert!(args.windows(2).any(|pair| pair[0] == "--cpus"));
        assert!(args.windows(2).any(|pair| pair[0] == "--pids-limit"));
    }

    #[test]
    fn workspace_resolution_rejects_paths_outside_allowed_roots() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            allowed_cwd_roots: vec![allowed.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let result = resolve_workspace(Some(&outside.path().to_string_lossy()), &config);
        assert!(result.is_err());
    }

    #[test]
    fn docker_mode_falls_back_to_native_when_docker_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let workspace = ResolvedWorkspace {
            root: root.path().to_path_buf(),
            cwd: root.path().to_path_buf(),
            container_cwd: "/workspace".to_string(),
        };
        let config = TerminalToolConfig::default();
        let launch =
            build_sandbox_command("fallback-process", "echo fallback", &workspace, &config, false)
                .unwrap();
        assert!(launch.docker_container_name.is_none());
        assert!(launch.runtime_label.contains("fallback"));
        let program = launch.command.as_std().get_program().to_string_lossy();
        assert!(program == "cmd" || program == "sh");
    }
}

