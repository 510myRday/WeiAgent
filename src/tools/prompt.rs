use std::path::Path;

use crate::domain::{AgentToolsConfig, TerminalPermissionMode};

/// Stable runtime facts + terminal policy for the system prompt prefix.
/// Intentionally excludes timestamps and other per-message values so prompt
/// caching remains effective across turns in the same configuration.
pub fn build_environment_prefix(tools_config: &AgentToolsConfig, data_dir: &Path) -> String {
    let mut sections = Vec::new();
    sections.push(build_environment_section(tools_config, data_dir));
    if tools_config.terminal.enabled {
        sections.push(build_terminal_policy_section(tools_config));
    }
    sections.join("\n\n")
}

fn build_environment_section(tools_config: &AgentToolsConfig, data_dir: &Path) -> String {
    let os = host_os_label();
    let arch = std::env::consts::ARCH;
    let shell = host_shell_label();
    let data_dir = resolve_data_dir_for_prompt(data_dir);
    let roots = resolve_workspace_roots_for_prompt(&tools_config.terminal.allowed_cwd_roots);
    let default_workspace = roots
        .first()
        .cloned()
        .unwrap_or_else(|| "server process working directory".to_string());
    let roots_text = if roots.is_empty() {
        default_workspace.clone()
    } else {
        roots.join(", ")
    };
    let terminal_enabled = if tools_config.terminal.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let permission = match tools_config.terminal.permission_mode {
        TerminalPermissionMode::RequestApproval => "request_approval",
        TerminalPermissionMode::FullAccess => "full_access",
    };
    let timeout = tools_config.terminal.default_timeout_ms;
    let app_version = env!("CARGO_PKG_VERSION");

    format!(
        "## Environment\n\
- App: WeiAgent {app_version}\n\
- OS: {os} ({arch})\n\
- Shell: {shell}\n\
- App data directory: {data_dir}\n\
- Default workspace: {default_workspace}\n\
- Allowed workspace roots: {roots_text}\n\
- Terminal tool: {terminal_enabled}\n\
- Terminal permission mode: {permission}\n\
- Default foreground timeout_ms: {timeout} (max 120000)\n\
- Tool stdout/stderr returned to the model: last 64KB only\n\
\n\
These environment facts are fixed for this agent configuration. Do not invent a different OS, shell, workspace, or app data directory. The app data directory is where WeiAgent stores settings, agents, conversations, and runtime files for this process. Prefer relative paths under the default workspace unless the user explicitly asks for another existing path."
    )
}

fn build_terminal_policy_section(tools_config: &AgentToolsConfig) -> String {
    match tools_config.terminal.permission_mode {
        TerminalPermissionMode::RequestApproval => {
            "## Terminal Policy\n\
- Default execution uses the OS sandbox (`sandbox_permissions=\"use_default\"`).\n\
- Inside configured workspace roots: file reads/writes for normal local work are allowed by the sandbox profile.\n\
- Network access and writes outside the workspace roots are blocked in the default sandbox.\n\
- Use `sandbox_permissions=\"require_escalated\"` only when the user explicitly needs internet access, writes outside the workspace, or other full host access. Always include a short `justification`.\n\
- Escalated commands require user approval unless this agent already has a matching always-allow grant.\n\
- If a sandboxed command is denied or fails due to permissions, report the real error. Retry with escalation only when the user intent requires it; never invent success.\n\
- Commands are non-interactive: stdin is closed. Do not rely on prompts, PTY sessions, or interactive pagers.\n\
- Prefer foreground `execute_terminal` for short commands. Use `background=true` only for long-lived services/watchers/dev servers; do not background ordinary one-shot work.\n\
- Long-running workflow: start with `background=true` (or let a foreground timeout promote the process) -> poll with `get_terminal_process` until ready/finished -> do the work -> call `stop_terminal_process` when the process is no longer needed.\n\
- Call `get_terminal_process` / `stop_terminal_process` only with a returned `process_id`. Do not spam empty polls; if still running, wait and retry later."
                .to_string()
        }
        TerminalPermissionMode::FullAccess => {
            "## Terminal Policy\n\
- Terminal commands run with full host access and no approval prompts.\n\
- Prefer staying inside the configured workspace roots unless the user explicitly asks otherwise.\n\
- Commands are non-interactive: stdin is closed. Do not rely on prompts, PTY sessions, or interactive pagers.\n\
- Prefer foreground `execute_terminal` for short commands. Use `background=true` only for long-lived services/watchers/dev servers; do not background ordinary one-shot work.\n\
- Long-running workflow: start with `background=true` (or let a foreground timeout promote the process) -> poll with `get_terminal_process` until ready/finished -> do the work -> call `stop_terminal_process` when the process is no longer needed.\n\
- Call `get_terminal_process` / `stop_terminal_process` only with a returned `process_id`. Do not spam empty polls; if still running, wait and retry later.\n\
- Be careful with destructive operations (delete, overwrite, recursive remove). Prefer targeted, reversible steps."
                .to_string()
        }
    }
}

pub fn host_os_label() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        std::env::consts::OS
    }
}

pub fn host_shell_label() -> &'static str {
    if cfg!(windows) {
        "cmd.exe (command strings use cmd.exe syntax; UTF-8 code page is applied automatically)"
    } else {
        "POSIX shell via sh -lc"
    }
}

pub fn host_shell_guidance() -> &'static str {
    if cfg!(windows) {
        "The host is Windows. The `command` string is interpreted by cmd.exe. Use Windows-compatible commands and paths (for example `dir`, `type file.txt`, `where python`, `cd /d D:\\path && dir`). Do not assume a POSIX shell, /tmp, bash heredocs, python3, ls, or pwd are available unless you have verified them."
    } else {
        "The host is Unix-like. The `command` string is interpreted by a POSIX shell (`sh -lc`). Prefer standard shell utilities and absolute or workspace-relative paths."
    }
}


fn resolve_data_dir_for_prompt(data_dir: &Path) -> String {
    let absolute = std::fs::canonicalize(data_dir).unwrap_or_else(|_| {
        if data_dir.is_absolute() {
            data_dir.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(data_dir))
                .unwrap_or_else(|_| data_dir.to_path_buf())
        }
    });
    let value = absolute.display().to_string();
    // Drop Windows extended-length prefixes so the model sees normal paths.
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        value
    }
}

fn resolve_workspace_roots_for_prompt(configured: &[String]) -> Vec<String> {
    if configured.is_empty() {
        return std::env::current_dir()
            .ok()
            .map(|path| {
                std::fs::canonicalize(&path)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .into_iter()
            .collect();
    }

    configured
        .iter()
        .map(|root| {
            std::fs::canonicalize(root)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| root.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TerminalToolConfig;

    #[test]
    fn environment_prefix_omits_timestamps_and_is_stable_for_same_config() {
        let config = AgentToolsConfig {
            terminal: TerminalToolConfig {
                enabled: true,
                permission_mode: TerminalPermissionMode::RequestApproval,
                default_timeout_ms: 30_000,
                allowed_cwd_roots: Vec::new(),
                approval_grants: Vec::new(),
            },
            ..AgentToolsConfig::default()
        };
        let data_dir = Path::new("data");
        let first = build_environment_prefix(&config, data_dir);
        let second = build_environment_prefix(&config, data_dir);
        assert_eq!(first, second);
        assert!(first.contains("## Environment"));
        assert!(first.contains("## Terminal Policy"));
        assert!(first.contains("WeiAgent"));
        assert!(first.contains("App data directory:"));
        assert!(first.contains("Long-running workflow"));
        assert!(first.contains("stop_terminal_process"));
        assert!(first.contains("Do not spam empty polls"));
        assert!(!first.to_lowercase().contains("2026-"));
        assert!(!first.contains("expires_at"));
        assert!(!first.contains("created_at"));
    }

    #[test]
    fn full_access_policy_is_used_when_configured() {
        let config = AgentToolsConfig {
            terminal: TerminalToolConfig {
                enabled: true,
                permission_mode: TerminalPermissionMode::FullAccess,
                ..TerminalToolConfig::default()
            },
            ..AgentToolsConfig::default()
        };
        let prefix = build_environment_prefix(&config, Path::new("data"));
        assert!(prefix.contains("full host access"));
        assert!(prefix.contains("Long-running workflow"));
        assert!(prefix.contains("stop_terminal_process"));
        assert!(!prefix.contains("require_escalated"));
    }

    #[test]
    fn disabled_terminal_skips_policy_section() {
        let config = AgentToolsConfig {
            terminal: TerminalToolConfig {
                enabled: false,
                ..TerminalToolConfig::default()
            },
            ..AgentToolsConfig::default()
        };
        let prefix = build_environment_prefix(&config, Path::new("data"));
        assert!(prefix.contains("Terminal tool: disabled"));
        assert!(!prefix.contains("## Terminal Policy"));
    }

    #[test]
    fn environment_prefix_includes_resolved_data_directory() {
        let config = AgentToolsConfig::default();
        let data_dir = Path::new("data");
        let prefix = build_environment_prefix(&config, data_dir);
        let expected = resolve_data_dir_for_prompt(data_dir);
        assert!(
            prefix.contains(&format!("App data directory: {expected}")),
            "prefix missing data dir line: {prefix}"
        );
    }
}
