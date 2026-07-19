use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostShellKind {
    #[cfg(windows)]
    PowerShell7,
    #[cfg(windows)]
    WindowsPowerShell,
    #[cfg(windows)]
    Cmd,
    #[cfg(not(windows))]
    PosixSh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShell {
    kind: HostShellKind,
    executable: PathBuf,
}

static SELECTED_HOST_SHELL: OnceLock<HostShell> = OnceLock::new();

pub fn selected_host_shell() -> &'static HostShell {
    SELECTED_HOST_SHELL.get_or_init(detect_host_shell)
}

impl HostShell {
    pub fn kind(&self) -> HostShellKind {
        self.kind
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn display_name(&self) -> &'static str {
        match self.kind {
            #[cfg(windows)]
            HostShellKind::PowerShell7 => "PowerShell 7 (pwsh.exe)",
            #[cfg(windows)]
            HostShellKind::WindowsPowerShell => "Windows PowerShell (powershell.exe)",
            #[cfg(windows)]
            HostShellKind::Cmd => "Command Prompt (cmd.exe)",
            #[cfg(not(windows))]
            HostShellKind::PosixSh => "POSIX shell (sh)",
        }
    }

    pub fn syntax_name(&self) -> &'static str {
        match self.kind {
            #[cfg(windows)]
            HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => "PowerShell",
            #[cfg(windows)]
            HostShellKind::Cmd => "cmd.exe",
            #[cfg(not(windows))]
            HostShellKind::PosixSh => "POSIX sh",
        }
    }

    pub fn environment_label(&self) -> String {
        format!(
            "{}; command strings use {} syntax",
            self.display_name(),
            self.syntax_name()
        )
    }

    pub fn tool_guidance(&self) -> &'static str {
        match self.kind {
            #[cfg(windows)]
            HostShellKind::PowerShell7 => {
                "Host OS: Windows. Commands use PowerShell 7 syntax. Do not use cmd.exe-only or POSIX shell syntax unless invoking that shell explicitly."
            }
            #[cfg(windows)]
            HostShellKind::WindowsPowerShell => {
                "Host OS: Windows. Commands use Windows PowerShell 5.1-compatible syntax. Do not use PowerShell 7-only, cmd.exe-only, or POSIX shell syntax unless invoking that shell explicitly."
            }
            #[cfg(windows)]
            HostShellKind::Cmd => {
                "Host OS: Windows. Commands use cmd.exe syntax. Do not use bare PowerShell cmdlets or POSIX shell syntax; invoke PowerShell explicitly if it is installed and needed."
            }
            #[cfg(not(windows))]
            HostShellKind::PosixSh => {
                "Host OS: Unix-like. Commands use POSIX sh syntax and Unix paths."
            }
        }
    }

    pub fn command_examples(&self) -> &'static str {
        match self.kind {
            #[cfg(windows)]
            HostShellKind::PowerShell7 | HostShellKind::WindowsPowerShell => {
                "Examples: `Get-ChildItem -Force`, `Get-Content README.md`, `rg --files | Select-Object -First 200`, `cargo test`."
            }
            #[cfg(windows)]
            HostShellKind::Cmd => {
                "Examples: `dir /a`, `type README.md`, `rg --files`, `cargo test`."
            }
            #[cfg(not(windows))]
            HostShellKind::PosixSh => {
                "Examples: `ls -la`, `sed -n '1,80p' README.md`, `rg --files | head -n 200`, `cargo test`."
            }
        }
    }
}

fn detect_host_shell() -> HostShell {
    #[cfg(windows)]
    {
        detect_windows_shell_from(
            env::var_os("PATH").as_deref(),
            env::var_os("COMSPEC").as_deref(),
            env::var_os("ProgramFiles").as_deref(),
            env::var_os("SystemRoot").as_deref(),
        )
    }

    #[cfg(not(windows))]
    {
        let executable = detect_posix_sh_path(env::var_os("PATH").as_deref(), Path::new("/bin/sh"));
        HostShell {
            kind: HostShellKind::PosixSh,
            executable,
        }
    }
}

#[cfg(any(not(windows), test))]
fn detect_posix_sh_path(path: Option<&std::ffi::OsStr>, standard_shell: &Path) -> PathBuf {
    if standard_shell.is_file() {
        return standard_shell.to_path_buf();
    }
    find_executable(path, "sh").unwrap_or_else(|| PathBuf::from("sh"))
}

#[cfg(windows)]
fn detect_windows_shell_from(
    path: Option<&std::ffi::OsStr>,
    comspec: Option<&std::ffi::OsStr>,
    program_files: Option<&std::ffi::OsStr>,
    system_root: Option<&std::ffi::OsStr>,
) -> HostShell {
    let pwsh = find_executable(path, "pwsh.exe")
        .or_else(|| existing_relative_executable(program_files, &["PowerShell", "7", "pwsh.exe"]));
    if let Some(executable) = pwsh {
        return HostShell {
            kind: HostShellKind::PowerShell7,
            executable,
        };
    }
    let windows_powershell = find_executable(path, "powershell.exe").or_else(|| {
        existing_relative_executable(
            system_root,
            &["System32", "WindowsPowerShell", "v1.0", "powershell.exe"],
        )
    });
    if let Some(executable) = windows_powershell {
        return HostShell {
            kind: HostShellKind::WindowsPowerShell,
            executable,
        };
    }
    let executable = comspec
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|candidate| candidate.is_file())
        .or_else(|| find_executable(path, "cmd.exe"))
        .or_else(|| existing_relative_executable(system_root, &["System32", "cmd.exe"]))
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    HostShell {
        kind: HostShellKind::Cmd,
        executable,
    }
}

#[cfg(windows)]
fn existing_relative_executable(
    base: Option<&std::ffi::OsStr>,
    segments: &[&str],
) -> Option<PathBuf> {
    let mut candidate = PathBuf::from(base?);
    candidate.extend(segments);
    candidate.is_file().then_some(candidate)
}

fn find_executable(path: Option<&std::ffi::OsStr>, name: &str) -> Option<PathBuf> {
    env::split_paths(path?).find_map(|directory| {
        let candidate = directory.join(name);
        candidate.is_file().then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_shell_has_consistent_labels() {
        let shell = selected_host_shell();
        assert!(shell.environment_label().contains(shell.display_name()));
        assert!(shell.environment_label().contains(shell.syntax_name()));
        assert!(shell.tool_guidance().contains("Host OS:"));
        assert!(shell.command_examples().contains("Examples:"));
    }

    #[test]
    fn posix_shell_path_prefers_standard_then_path_then_command_name() {
        let root = tempfile::tempdir().unwrap();
        let standard = root.path().join("standard").join("sh");
        let path_dir = root.path().join("path");
        let path_shell = path_dir.join("sh");
        std::fs::create_dir_all(standard.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();
        std::fs::write(&standard, b"").unwrap();
        std::fs::write(&path_shell, b"").unwrap();
        let path = env::join_paths([&path_dir]).unwrap();

        assert_eq!(detect_posix_sh_path(Some(&path), &standard), standard);

        std::fs::remove_file(&standard).unwrap();
        assert_eq!(detect_posix_sh_path(Some(&path), &standard), path_shell);

        std::fs::remove_file(&path_shell).unwrap();
        assert_eq!(
            detect_posix_sh_path(Some(&path), &standard),
            PathBuf::from("sh")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_detection_prefers_pwsh_then_windows_powershell_then_cmd() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("cmd.exe"), b"").unwrap();
        std::fs::write(second.join("powershell.exe"), b"").unwrap();
        std::fs::write(first.join("pwsh.exe"), b"").unwrap();
        let path = env::join_paths([&first, &second]).unwrap();

        let shell = detect_windows_shell_from(Some(&path), None, None, None);
        assert_eq!(shell.kind(), HostShellKind::PowerShell7);
        assert_eq!(shell.executable(), first.join("pwsh.exe"));

        std::fs::remove_file(first.join("pwsh.exe")).unwrap();
        let shell = detect_windows_shell_from(Some(&path), None, None, None);
        assert_eq!(shell.kind(), HostShellKind::WindowsPowerShell);
        assert_eq!(shell.executable(), second.join("powershell.exe"));

        std::fs::remove_file(second.join("powershell.exe")).unwrap();
        let shell = detect_windows_shell_from(Some(&path), None, None, None);
        assert_eq!(shell.kind(), HostShellKind::Cmd);
        assert_eq!(shell.executable(), second.join("cmd.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_detection_uses_comspec_as_the_final_fallback() {
        let root = tempfile::tempdir().unwrap();
        let comspec = root.path().join("cmd.exe");
        std::fs::write(&comspec, b"").unwrap();
        let shell = detect_windows_shell_from(None, Some(comspec.as_os_str()), None, None);
        assert_eq!(shell.kind(), HostShellKind::Cmd);
        assert_eq!(shell.executable(), comspec);
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_detection_checks_standard_install_directories() {
        let root = tempfile::tempdir().unwrap();
        let program_files = root.path().join("Program Files");
        let system_root = root.path().join("Windows");
        let pwsh = program_files.join(r"PowerShell\7\pwsh.exe");
        let windows_powershell =
            system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        std::fs::create_dir_all(pwsh.parent().unwrap()).unwrap();
        std::fs::create_dir_all(windows_powershell.parent().unwrap()).unwrap();
        std::fs::write(&windows_powershell, b"").unwrap();

        let shell = detect_windows_shell_from(
            None,
            None,
            Some(program_files.as_os_str()),
            Some(system_root.as_os_str()),
        );
        assert_eq!(shell.kind(), HostShellKind::WindowsPowerShell);
        assert_eq!(shell.executable(), windows_powershell);

        std::fs::write(&pwsh, b"").unwrap();
        let shell = detect_windows_shell_from(
            None,
            None,
            Some(program_files.as_os_str()),
            Some(system_root.as_os_str()),
        );
        assert_eq!(shell.kind(), HostShellKind::PowerShell7);
        assert_eq!(shell.executable(), pwsh);
    }
}
