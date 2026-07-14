use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::Deserialize;
use tokio::process::Command;
use uuid::Uuid;

use crate::{
    domain::{ProcessInfo, ProcessStatus, TerminalPermissionMode, TerminalToolConfig, ToolResult},
    tools::{
        process_manager::{ProcessManager, ProcessMetadata},
        runtime::{SandboxExecutable, SandboxRuntimeManager},
    },
};

pub const MAX_TERMINAL_TIMEOUT_MS: u64 = 120_000;
const MAX_LLM_TOOL_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPermissions {
    #[default]
    UseDefault,
    RequireEscalated,
}

pub struct ExecuteTerminalRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub background: Option<bool>,
    pub sandbox_permissions: SandboxPermissions,
    pub approved: bool,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub tool_call_id: String,
}

pub async fn execute_terminal(
    req: &ExecuteTerminalRequest,
    config: &TerminalToolConfig,
    process_manager: &Arc<ProcessManager>,
    sandbox_runtime: &Arc<SandboxRuntimeManager>,
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

    let escalation_requested = req.sandbox_permissions == SandboxPermissions::RequireEscalated;
    let full_access_mode = config.permission_mode == TerminalPermissionMode::FullAccess;
    let allow_outside_workspace = full_access_mode || escalation_requested;
    let workspace = match resolve_workspace(req.cwd.as_deref(), config, allow_outside_workspace) {
        Ok(workspace) => workspace,
        Err(WorkspaceResolutionError::Invalid(message)) => {
            return ToolResult::Error { message };
        }
        Err(WorkspaceResolutionError::Blocked(reason)) => {
            return ToolResult::Blocked { reason };
        }
    };

    let context_granted =
        escalation_requested && config.has_context_grant(&req.command, req.cwd.as_deref());
    if !full_access_mode && escalation_requested && !context_granted && !req.approved {
        return ToolResult::PendingApproval {
            command: req.command.clone(),
            approval_id: Uuid::new_v4().to_string(),
        };
    }

    let use_full_access = full_access_mode || escalation_requested;
    let sandbox_executable = if use_full_access {
        None
    } else {
        match sandbox_runtime.executable().await {
            Some(executable) => Some(executable),
            None => {
                return ToolResult::Blocked {
                    reason: "The managed OS sandbox is unavailable. Install or refresh the sandbox runtime in Terminal settings, or explicitly choose full-access mode."
                        .to_string(),
                };
            }
        }
    };
    let launch = match build_execution_command(
        &req.command,
        &workspace,
        use_full_access,
        sandbox_executable.as_ref(),
    ) {
        Ok(command) => command,
        Err(message) => return ToolResult::Error { message },
    };
    let process_id = process_manager.new_process_id();
    let metadata = ProcessMetadata {
        agent_id: req.agent_id,
        conversation_id: req.conversation_id,
        tool_call_id: req.tool_call_id.clone(),
        command: req.command.clone(),
        cwd: Some(workspace.cwd.to_string_lossy().to_string()),
        detached: req.background.unwrap_or(false),
    };
    if let Err(error) = process_manager
        .spawn(process_id.clone(), launch.command, metadata)
        .await
    {
        let hint = if launch.runtime_label == "os_sandbox" {
            " Install or refresh the managed sandbox runtime in WeiAgent Terminal settings."
        } else {
            ""
        };
        return ToolResult::Error {
            message: format!("Failed to start terminal command: {error}.{hint}"),
        };
    }
    let mut process_lease = ProcessLease::new(process_manager.clone(), process_id.clone());

    if req.background.unwrap_or(false) {
        process_lease.disarm();
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

    let timeout_ms = req
        .timeout_ms
        .unwrap_or(config.default_timeout_ms)
        .clamp(1, MAX_TERMINAL_TIMEOUT_MS);
    let result = match process_manager
        .wait_for_completion(&process_id, Duration::from_millis(timeout_ms))
        .await
    {
        Some(info) if info.status == ProcessStatus::Running => {
            process_manager.promote_to_background(&process_id);
            ToolResult::Running {
                process_id,
                message: format!(
                    "Command exceeded {timeout_ms}ms and continues in background with output capture ({})",
                    launch.runtime_label
                ),
                stdout: truncate_for_llm(info.stdout),
                stderr: truncate_for_llm(info.stderr),
                duration_ms: info.duration_ms,
                runtime: Some(launch.runtime_label),
            }
        }
        Some(info) => {
            let result = process_info_to_result_with_runtime(info, Some(launch.runtime_label));
            process_manager.remove(&process_id);
            result
        }
        None => ToolResult::Error {
            message: "Process disappeared before completion".to_string(),
        },
    };
    process_lease.disarm();
    result
}

struct ProcessLease {
    process_manager: Arc<ProcessManager>,
    process_id: String,
    armed: bool,
}

impl ProcessLease {
    fn new(process_manager: Arc<ProcessManager>, process_id: String) -> Self {
        Self {
            process_manager,
            process_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProcessLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let process_manager = self.process_manager.clone();
        let process_id = self.process_id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = process_manager.stop(&process_id).await;
            });
        }
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
            stdout: truncate_for_llm(info.stdout),
            stderr: truncate_for_llm(info.stderr),
            exit_code: info.exit_code.unwrap_or(-1),
            duration_ms: info.duration_ms,
            runtime,
        },
        ProcessStatus::Running => ToolResult::Running {
            process_id: info.process_id,
            message: "Process is still running".to_string(),
            stdout: truncate_for_llm(info.stdout),
            stderr: truncate_for_llm(info.stderr),
            duration_ms: info.duration_ms,
            runtime,
        },
        ProcessStatus::Stopped => ToolResult::Rejected {
            reason: "Process was stopped".to_string(),
        },
    }
}

fn truncate_for_llm(value: String) -> String {
    if value.len() <= MAX_LLM_TOOL_OUTPUT_BYTES {
        return value;
    }
    let start = value.len() - MAX_LLM_TOOL_OUTPUT_BYTES;
    let tail = String::from_utf8_lossy(&value.as_bytes()[start..]);
    format!("[output truncated; showing last {MAX_LLM_TOOL_OUTPUT_BYTES} bytes]\n{tail}")
}

struct ResolvedWorkspace {
    root: PathBuf,
    cwd: PathBuf,
}

#[derive(Debug)]
enum WorkspaceResolutionError {
    Invalid(String),
    Blocked(String),
}

struct PreparedLaunch {
    command: Command,
    runtime_label: String,
}

fn resolve_workspace(
    cwd: Option<&str>,
    config: &TerminalToolConfig,
    allow_outside_workspace: bool,
) -> Result<ResolvedWorkspace, WorkspaceResolutionError> {
    let roots = if config.allowed_cwd_roots.is_empty() {
        vec![std::env::current_dir().map_err(|error| {
            WorkspaceResolutionError::Invalid(format!(
                "Failed to resolve server working directory: {error}"
            ))
        })?]
    } else {
        config.allowed_cwd_roots.iter().map(PathBuf::from).collect()
    };
    let roots: Vec<PathBuf> = roots
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    if roots.is_empty() {
        return Err(WorkspaceResolutionError::Invalid(
            "No valid terminal workspace root is configured".to_string(),
        ));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .and_then(|path| std::fs::canonicalize(path).ok());
    if roots.iter().any(|root| {
        root.parent().is_none()
            || home
                .as_ref()
                .is_some_and(|home| crate::fsutil::paths_equal(home, root))
    })
    {
        return Err(WorkspaceResolutionError::Blocked(
            "Filesystem roots and the entire user home directory cannot be terminal workspace roots"
                .to_string(),
        ));
    }

    let requested = match cwd.map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => roots[0].join(path),
        None => roots[0].clone(),
    };
    let canonical_cwd = std::fs::canonicalize(&requested).map_err(|error| {
        WorkspaceResolutionError::Invalid(format!(
            "Working directory '{}' does not exist or cannot be accessed: {error}. Retry without cwd to use the default workspace '{}'; do not substitute a guessed path such as /tmp.",
            requested.display(),
            roots[0].display()
        ))
    })?;
    let matching_root = roots
        .iter()
        .find(|root| crate::fsutil::path_is_within(&canonical_cwd, root))
        .cloned();
    if matching_root.is_none() && !allow_outside_workspace {
        return Err(WorkspaceResolutionError::Blocked(format!(
            "Working directory '{}' is outside the configured OS sandbox roots. Retry with sandbox_permissions='require_escalated' if the user asked to work there.",
            canonical_cwd.display()
        )));
    }
    Ok(ResolvedWorkspace {
        root: matching_root.unwrap_or_else(|| roots[0].clone()),
        cwd: canonical_cwd,
    })
}

fn build_execution_command(
    shell_command: &str,
    workspace: &ResolvedWorkspace,
    full_access: bool,
    sandbox_executable: Option<&SandboxExecutable>,
) -> Result<PreparedLaunch, String> {
    if full_access {
        return Ok(PreparedLaunch {
            command: build_host_command(shell_command, &workspace.cwd, "full_access"),
            runtime_label: "full_access".to_string(),
        });
    }
    let sandbox_executable =
        sandbox_executable.ok_or_else(|| "OS sandbox executable is unavailable".to_string())?;
    Ok(PreparedLaunch {
        command: build_os_sandbox_command(shell_command, workspace, sandbox_executable),
        runtime_label: "os_sandbox".to_string(),
    })
}

fn build_os_sandbox_command(
    shell_command: &str,
    workspace: &ResolvedWorkspace,
    executable: &SandboxExecutable,
) -> Command {
    let mut command = Command::new(&executable.program);
    if cfg!(windows) {
        command.args(["-c", "windows.sandbox=\"unelevated\""]);
    }
    command
        .arg("sandbox")
        .args(["--permissions-profile", ":workspace"])
        .arg("-C")
        .arg(os_command_path(&workspace.root));

    if cfg!(windows) {
        command
            .arg("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(windows_powershell_cmd_script(shell_command, Some(&workspace.cwd)));
    } else {
        command.arg("sh").args(["-lc"]).arg(format!(
            "cd -- {} && {}",
            quote_unix_path(&workspace.cwd),
            shell_command
        ));
    }
    sanitize_environment(&mut command, "os_sandbox");
    command.env("CODEX_HOME", &executable.codex_home);
    configure_process_group(&mut command);
    command
}

fn build_host_command(shell_command: &str, cwd: &Path, runtime: &str) -> Command {
    let mut command = if cfg!(windows) {
        // Match the sandbox path: PowerShell UTF-8 console setup + cmd.exe syntax.
        // `chcp 65001` alone is not enough for many Windows builtins/tools.
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(windows_powershell_cmd_script(shell_command, Some(cwd)));
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-lc", shell_command]);
        command
    };
    command.current_dir(cwd);
    sanitize_environment(&mut command, runtime);
    configure_process_group(&mut command);
    command
}

#[cfg(test)]
fn quote_windows_path(path: &Path) -> String {
    format!("\"{}\"", windows_command_path(path).replace('"', "\"\""))
}

fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn windows_utf8_command(command: &str) -> String {
    format!("chcp 65001>nul & {command}")
}

/// Shared Windows launch wrapper used by both full_access and os_sandbox.
///
/// Forces PowerShell's output encoding to UTF-8, optionally cds into `cwd`,
/// then runs the user command through cmd.exe so existing cmd syntax stays valid.
/// `chcp 65001` remains as a belt-and-suspenders measure for cmd builtins.
fn windows_powershell_cmd_script(shell_command: &str, cwd: Option<&Path>) -> String {
    let shell_command = windows_utf8_command(shell_command);
    let set_location = match cwd {
        Some(cwd) => format!(
            "Set-Location -LiteralPath {}; ",
            quote_powershell_literal(&windows_command_path(cwd))
        ),
        None => String::new(),
    };
    format!(
        "$ErrorActionPreference = 'Stop'; $OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); {set_location}$command = {}; & cmd.exe /D /S /C $command; exit $LASTEXITCODE",
        quote_powershell_literal(&shell_command)
    )
}


fn quote_unix_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn os_command_path(path: &Path) -> std::ffi::OsString {
    windows_command_path(path).into()
}

#[cfg(not(windows))]
fn os_command_path(path: &Path) -> std::ffi::OsString {
    path.as_os_str().to_os_string()
}

fn windows_command_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    value
        .strip_prefix(r"\\?\")
        .unwrap_or(value.as_ref())
        .to_string()
}

fn sanitize_environment(command: &mut Command, runtime: &str) {
    // Keep a minimal, cross-platform host environment. env_clear() removes
    // values tools commonly need (especially TMPDIR on macOS/Linux).
    let allowed = [
        // Paths and identity
        "PATH",
        "HOME",
        "USER",
        "USERNAME",
        "LOGNAME",
        "USERPROFILE",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LC_MESSAGES",
        // Temp dirs (Windows uses TEMP/TMP; Unix/macOS uses TMPDIR)
        "TEMP",
        "TMP",
        "TMPDIR",
        // Proxy / TLS
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "NODE_EXTRA_CA_CERTS",
        // Windows host process essentials
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "SystemDrive",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "OS",
        // Linux desktop conventions
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        // Common tool roots
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
        "GOROOT",
        "JAVA_HOME",
        "NODE_PATH",
        "NVM_DIR",
        "PYENV_ROOT",
        "VIRTUAL_ENV",
        "CONDA_PREFIX",
        "HOMEBREW_PREFIX",
        "HOMEBREW_CELLAR",
        "HOMEBREW_REPOSITORY",
    ];
    let preserved: Vec<_> = allowed
        .iter()
        .filter_map(|key| std::env::var_os(key).map(|value| ((*key).to_string(), value)))
        .collect();
    command.env_clear();
    for (key, value) in preserved {
        command.env(key, value);
    }
    // Ensure temp resolution works even when the host did not export TMPDIR.
    if std::env::var_os("TMPDIR").is_none() {
        let fallback = std::env::temp_dir();
        if !fallback.as_os_str().is_empty() {
            command.env("TMPDIR", fallback);
        }
    }
    command.env("WEIAGENT_RUNTIME", runtime);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_approval_is_the_default_permission_mode() {
        assert_eq!(
            TerminalToolConfig::default().permission_mode,
            TerminalPermissionMode::RequestApproval
        );
    }

    #[test]
    fn workspace_resolution_rejects_default_access_outside_roots() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            allowed_cwd_roots: vec![allowed.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let result = resolve_workspace(Some(&outside.path().to_string_lossy()), &config, false);
        assert!(matches!(result, Err(WorkspaceResolutionError::Blocked(_))));
    }

    #[test]
    fn elevated_access_accepts_existing_cwd_outside_roots() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            allowed_cwd_roots: vec![allowed.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let resolved =
            resolve_workspace(Some(&outside.path().to_string_lossy()), &config, true).unwrap();
        assert_eq!(resolved.cwd, std::fs::canonicalize(outside.path()).unwrap());
    }

    #[test]
    fn relative_dot_uses_the_default_workspace() {
        let root = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let resolved = resolve_workspace(Some("."), &config, false).unwrap();
        assert_eq!(resolved.cwd, std::fs::canonicalize(root.path()).unwrap());
    }

    #[test]
    fn os_sandbox_launch_uses_codex_workspace_profile() {
        let root = tempfile::tempdir().unwrap();
        let workspace = ResolvedWorkspace {
            root: root.path().to_path_buf(),
            cwd: root.path().to_path_buf(),
        };
        let executable = SandboxExecutable {
            program: PathBuf::from(if cfg!(windows) { "codex.exe" } else { "codex" }),
            codex_home: root.path().join("codex-home"),
        };
        let launch =
            build_execution_command("echo sandbox", &workspace, false, Some(&executable)).unwrap();
        let args = launch
            .command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(launch.runtime_label, "os_sandbox");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permissions-profile", ":workspace"])
        );
        assert!(
            args.iter()
                .any(|arg| arg == "echo sandbox" || arg.contains("echo sandbox"))
        );
    }

    #[test]
    fn windows_commands_switch_to_utf8_before_execution() {
        assert_eq!(
            windows_utf8_command("echo test"),
            "chcp 65001>nul & echo test"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_wrapper_is_shared_by_host_and_sandbox() {
        let cwd = PathBuf::from(r"D:\workspace");
        let script = windows_powershell_cmd_script("echo test", Some(&cwd));
        assert!(script.contains("[Text.UTF8Encoding]::new($false)"));
        assert!(script.contains("chcp 65001>nul & echo test"));
        assert!(script.contains("cmd.exe /D /S /C"));
        assert!(script.contains("Set-Location -LiteralPath"));
        assert!(script.contains(r"D:\workspace"));
    }

    #[cfg(windows)]
    #[test]
    fn full_access_windows_host_uses_powershell_utf8_wrapper() {
        if !cfg!(windows) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let launch = build_host_command("echo host", root.path(), "full_access");
        let program = launch.as_std().get_program().to_string_lossy().to_lowercase();
        assert!(
            program.contains("powershell"),
            "expected powershell host launcher, got {program}"
        );
        let args = launch
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let joined = args.join(" ");
        assert!(joined.contains("UTF8Encoding"));
        assert!(joined.contains("echo host") || joined.contains("chcp 65001"));
    }

    #[tokio::test]
    async fn escalated_execution_requires_approval() {
        let root = tempfile::tempdir().unwrap();
        let sandbox_runtime = Arc::new(SandboxRuntimeManager::new_for_tests(root.path()).unwrap());
        let config = TerminalToolConfig {
            enabled: true,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo approval-required".to_string(),
                cwd: None,
                timeout_ms: Some(100),
                background: Some(false),
                sandbox_permissions: SandboxPermissions::RequireEscalated,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "call-escalated".to_string(),
            },
            &config,
            &Arc::new(ProcessManager::new()),
            &sandbox_runtime,
        )
        .await;
        assert!(matches!(result, ToolResult::PendingApproval { .. }));
    }

    #[tokio::test]
    async fn full_access_runs_without_approval() {
        let root = tempfile::tempdir().unwrap();
        let process_manager = Arc::new(ProcessManager::new());
        let sandbox_runtime = Arc::new(SandboxRuntimeManager::new_for_tests(root.path()).unwrap());
        let config = TerminalToolConfig {
            enabled: true,
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let result = execute_terminal(
            &ExecuteTerminalRequest {
                command: "echo full-access".to_string(),
                cwd: None,
                timeout_ms: Some(2_000),
                background: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "call-full-access".to_string(),
            },
            &config,
            &process_manager,
            &sandbox_runtime,
        )
        .await;
        assert!(matches!(result, ToolResult::Completed { exit_code: 0, .. }));
        assert!(process_manager.list(None).await.is_empty());
    }

    #[tokio::test]
    async fn os_sandbox_writes_workspace_and_blocks_external_write_when_available() {
        let current = std::env::current_dir().unwrap();
        let parent = current.parent().unwrap();
        let root = tempfile::tempdir_in(&current).unwrap();
        let sandbox_runtime = Arc::new(SandboxRuntimeManager::new(root.path()).unwrap());
        if !sandbox_runtime.initialize().await.unwrap().available {
            return;
        }
        let outside = tempfile::tempdir_in(parent).unwrap();
        let inside_file = root.path().join("inside.txt");
        let outside_file = outside.path().join("outside.txt");
        let config = TerminalToolConfig {
            enabled: true,
            allowed_cwd_roots: vec![root.path().to_string_lossy().to_string()],
            ..TerminalToolConfig::default()
        };
        let inside_command = if cfg!(windows) {
            format!("echo inside> {}", quote_windows_path(&inside_file))
        } else {
            format!("printf inside > {}", quote_unix_path(&inside_file))
        };
        let inside = execute_terminal(
            &ExecuteTerminalRequest {
                command: inside_command,
                cwd: None,
                timeout_ms: Some(60_000),
                background: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "call-sandbox-inside".to_string(),
            },
            &config,
            &Arc::new(ProcessManager::new()),
            &sandbox_runtime,
        )
        .await;
        assert!(
            matches!(inside, ToolResult::Completed { exit_code: 0, .. }),
            "{inside:?}"
        );
        assert!(inside_file.exists());

        let outside_command = if cfg!(windows) {
            format!("echo outside> {}", quote_windows_path(&outside_file))
        } else {
            format!("printf outside > {}", quote_unix_path(&outside_file))
        };
        let outside_result = execute_terminal(
            &ExecuteTerminalRequest {
                command: outside_command,
                cwd: None,
                timeout_ms: Some(60_000),
                background: Some(false),
                sandbox_permissions: SandboxPermissions::UseDefault,
                approved: false,
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                tool_call_id: "call-sandbox-outside".to_string(),
            },
            &config,
            &Arc::new(ProcessManager::new()),
            &sandbox_runtime,
        )
        .await;
        assert!(
            matches!(
                outside_result,
                ToolResult::Completed {
                    exit_code,
                    ..
                } if exit_code != 0
            ),
            "{outside_result:?}"
        );
        assert!(!outside_file.exists());
    }
}
