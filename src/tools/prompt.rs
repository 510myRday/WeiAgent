use chrono::Local;
use std::path::Path;

/// Default guidance for efficient project file discovery when terminal tools
/// are available. Keep generated and dependency-heavy directories out of the
/// model context unless the task explicitly targets them.
pub const TERMINAL_FILE_DISCOVERY_POLICY: &str = r#"## File Discovery and Reading Policy
When inspecting a project with terminal or file tools, start from the smallest relevant project or feature directory and inspect only files needed for the current task. By default, do not recursively list, search, or read generated, dependency, build, cache, log, or temporary directories. Exclude these directory names unless the user explicitly asks about them or the task directly requires them: `.git`, `.hg`, `.svn`, `.venv`, `venv`, `env`, `node_modules`, `target`, `dist`, `build`, `out`, `coverage`, `.cache`, `cache`, `tmp`, `temp`, `.pytest_cache`, `__pycache__`, `.mypy_cache`, `.ruff_cache`, `.tox`, `.idea`, `.vscode`, and `logs`. Prefer `rg` for text search and `rg --files` for discovery with explicit `-g '!DIRECTORY/**'` exclusions; never dump an entire repository or a large generated file into context. Read lockfiles, logs, binaries, minified bundles, and other generated files only when they are directly relevant, and use targeted matches or bounded output instead of bulk reads.

If an excluded directory is relevant, enter it deliberately and narrow the command to the required path or filename."#;

pub fn today_date() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Stable runtime facts for the system prompt prefix.
/// Intentionally excludes timestamps and other per-message values so prompt
/// caching remains effective across turns in the same configuration.
pub fn build_environment_prefix(
    data_dir: &Path,
    terminal_workspace_dir: &Path,
    attachments_dir: &Path,
) -> String {
    let os = host_os_label();
    let arch = std::env::consts::ARCH;
    let shell = crate::tools::shell::selected_host_shell().environment_label();
    let data_dir = resolve_path_for_prompt(data_dir);
    let default_workspace = resolve_path_for_prompt(terminal_workspace_dir);
    let attachments_dir = resolve_path_for_prompt(attachments_dir);
    let app_version = env!("CARGO_PKG_VERSION");

    format!(
        "## Environment\n\
- App: WeiAgent {app_version}\n\
- OS: {os} ({arch})\n\
- Shell: {shell}\n\
- Data directory: {data_dir} (WeiAgent application data root)\n\
- Default workspace: {default_workspace} (default cwd for terminal and file tools)
- Image attachments: {attachments_dir} (save generated PNG/JPEG/WebP/GIF files here before embedding them)"
    )
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

fn resolve_path_for_prompt(path: &Path) -> String {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    });
    crate::fsutil::display_path(&absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_date_format() {
        let date = today_date();
        assert_eq!(date.len(), 10);
        assert!(date.contains("-"));
        let parts: Vec<&str> = date.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 2);
        assert_eq!(parts[2].len(), 2);
    }

    #[test]
    fn environment_prefix_omits_timestamps_and_is_stable_for_same_config() {
        let data_dir = Path::new("data");
        let workspace_dir = data_dir.join("workspace");
        let attachments_dir = workspace_dir.join("attachments");
        let first = build_environment_prefix(data_dir, &workspace_dir, &attachments_dir);
        let second = build_environment_prefix(data_dir, &workspace_dir, &attachments_dir);
        assert_eq!(first, second);
        assert!(first.contains("## Environment"));
        assert!(first.contains("WeiAgent"));
        assert!(first.contains("Default workspace:"));
        assert!(first.contains("Shell:"));
        assert!(first.contains("Data directory:"));
        assert!(!first.contains("Allowed workspace roots:"));
        assert!(!first.contains("Terminal tool:"));
        assert!(!first.contains("Terminal permission mode:"));
        assert!(!first.contains("## Terminal Policy"));
        assert!(!first.to_lowercase().contains("2026-"));
        assert!(!first.contains("expires_at"));
        assert!(!first.contains("created_at"));
    }

    #[test]
    fn terminal_file_discovery_policy_excludes_generated_and_dependency_directories() {
        assert!(TERMINAL_FILE_DISCOVERY_POLICY.contains("node_modules"));
        assert!(TERMINAL_FILE_DISCOVERY_POLICY.contains("target"));
        assert!(TERMINAL_FILE_DISCOVERY_POLICY.contains(".venv"));
        assert!(TERMINAL_FILE_DISCOVERY_POLICY.contains("rg --files"));
        assert!(TERMINAL_FILE_DISCOVERY_POLICY.contains("unless the user explicitly asks"));
    }

    #[test]
    fn environment_prefix_distinguishes_data_and_terminal_workspace_paths() {
        let data_dir = Path::new("data");
        let workspace_dir = data_dir.join("workspace");
        let attachments_dir = workspace_dir.join("attachments");
        let prefix = build_environment_prefix(data_dir, &workspace_dir, &attachments_dir);
        let expected_data = resolve_path_for_prompt(data_dir);
        let expected_workspace = resolve_path_for_prompt(&workspace_dir);
        let expected_attachments = resolve_path_for_prompt(&attachments_dir);
        assert!(
            prefix.contains(&format!("Data directory: {expected_data}")),
            "prefix missing data directory line: {prefix}"
        );
        assert!(
            prefix.contains(&format!("Default workspace: {expected_workspace}")),
            "prefix missing default workspace line: {prefix}"
        );
        assert!(prefix.contains("Shell:"));
        assert!(prefix.contains(&format!("Image attachments: {expected_attachments}")));
        assert!(!prefix.contains("Allowed workspace roots:"));
    }
}
