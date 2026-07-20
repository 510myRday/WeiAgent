use chrono::Local;
use std::path::Path;

pub fn today_date() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Stable runtime facts for the system prompt prefix.
/// Intentionally excludes timestamps and other per-message values so prompt
/// caching remains effective across turns in the same configuration.
pub fn build_environment_prefix(data_dir: &Path) -> String {
    build_environment_section(data_dir)
}

fn build_environment_section(data_dir: &Path) -> String {
    let os = host_os_label();
    let arch = std::env::consts::ARCH;
    let shell = crate::tools::shell::selected_host_shell().environment_label();
    let default_workspace = resolve_data_dir_for_prompt(data_dir);
    let app_version = env!("CARGO_PKG_VERSION");

    format!(
        "## Environment\n\
- App: WeiAgent {app_version}\n\
- OS: {os} ({arch})\n\
- Shell: {shell}\n\
- Default workspace: {default_workspace}"
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
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        value
    }
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
        let first = build_environment_prefix(data_dir);
        let second = build_environment_prefix(data_dir);
        assert_eq!(first, second);
        assert!(first.contains("## Environment"));
        assert!(first.contains("WeiAgent"));
        assert!(first.contains("Default workspace:"));
        assert!(first.contains("Shell:"));
        assert!(!first.contains("App data directory:"));
        assert!(!first.contains("Allowed workspace roots:"));
        assert!(!first.contains("Terminal tool:"));
        assert!(!first.contains("Terminal permission mode:"));
        assert!(!first.contains("## Terminal Policy"));
        assert!(!first.to_lowercase().contains("2026-"));
        assert!(!first.contains("expires_at"));
        assert!(!first.contains("created_at"));
    }

    #[test]
    fn environment_prefix_includes_only_default_workspace_path() {
        let data_dir = Path::new("data");
        let prefix = build_environment_prefix(data_dir);
        let expected = resolve_data_dir_for_prompt(data_dir);
        assert!(
            prefix.contains(&format!("Default workspace: {expected}")),
            "prefix missing default workspace line: {prefix}"
        );
        assert!(prefix.contains("Shell:"));
        assert!(!prefix.contains("App data directory:"));
        assert!(!prefix.contains("Allowed workspace roots:"));
    }
}
