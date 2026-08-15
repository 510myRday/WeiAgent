use std::path::{Path, PathBuf};

use crate::fsutil;

#[derive(Debug, Clone)]
pub struct ResolvedWorkspace {
    pub default_root: PathBuf,
    pub cwd: PathBuf,
    pub roots: Vec<PathBuf>,
    pub matching_root: Option<PathBuf>,
}

impl ResolvedWorkspace {
    pub fn root(&self) -> &Path {
        self.matching_root.as_ref().unwrap_or(&self.default_root)
    }
}

#[derive(Debug)]
pub enum WorkspaceResolutionError {
    Invalid(String),
    Outside,
    Blocked(String),
}

#[derive(Debug, Clone, Copy)]
pub struct WorkspaceResolveOptions {
    pub expand_home: bool,
    pub blocked_message: &'static str,
}

impl Default for WorkspaceResolveOptions {
    fn default() -> Self {
        Self {
            expand_home: false,
            blocked_message: "Filesystem roots and the entire user home directory cannot be workspace roots",
        }
    }
}

pub fn home_dir() -> Option<PathBuf> {
    crate::fsutil::home_dir()
}

pub fn expand_home_path(raw: &str) -> String {
    if raw == "~"
        && let Some(home) = home_dir()
    {
        return home.to_string_lossy().into_owned();
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    raw.to_string()
}

pub fn resolve_workspace(
    cwd: Option<&str>,
    allowed_cwd_roots: &[String],
    default_workspace: &Path,
    allow_outside: bool,
    options: WorkspaceResolveOptions,
) -> Result<ResolvedWorkspace, WorkspaceResolutionError> {
    let default_root = std::fs::canonicalize(default_workspace).map_err(|error| {
        WorkspaceResolutionError::Invalid(format!(
            "Default terminal workspace '{}' does not exist or cannot be accessed: {error}",
            default_workspace.display()
        ))
    })?;
    let mut roots = vec![default_root.clone()];
    for root in allowed_cwd_roots {
        let Ok(root) = std::fs::canonicalize(root) else {
            continue;
        };
        if !roots
            .iter()
            .any(|existing| fsutil::paths_equal(existing, &root))
        {
            roots.push(root);
        }
    }

    let home = home_dir().and_then(|path| std::fs::canonicalize(path).ok());
    if roots.iter().any(|root| {
        root.parent().is_none()
            || home
                .as_ref()
                .is_some_and(|home| fsutil::paths_equal(home, root))
    }) {
        return Err(WorkspaceResolutionError::Blocked(
            options.blocked_message.to_string(),
        ));
    }

    let requested = match cwd.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(if options.expand_home {
                expand_home_path(path)
            } else {
                path.to_string()
            });
            if path.is_absolute() {
                path
            } else {
                default_root.join(path)
            }
        }
        None => default_root.clone(),
    };
    let canonical_cwd = std::fs::canonicalize(&requested).map_err(|error| {
        WorkspaceResolutionError::Invalid(format!(
            "Working directory '{}' does not exist or cannot be accessed: {error}. Retry without cwd to use the default workspace '{}'.",
            requested.display(),
            default_root.display()
        ))
    })?;
    let matching_root = roots
        .iter()
        .find(|root| fsutil::path_is_within(&canonical_cwd, root))
        .cloned();
    if matching_root.is_none() && !allow_outside {
        return Err(WorkspaceResolutionError::Outside);
    }
    Ok(ResolvedWorkspace {
        default_root,
        cwd: canonical_cwd,
        roots,
        matching_root,
    })
}

pub async fn resolve_workspace_async(
    cwd: Option<String>,
    allowed_cwd_roots: Vec<String>,
    default_workspace: PathBuf,
    allow_outside: bool,
    options: WorkspaceResolveOptions,
) -> Result<ResolvedWorkspace, WorkspaceResolutionError> {
    tokio::task::spawn_blocking(move || {
        resolve_workspace(
            cwd.as_deref(),
            &allowed_cwd_roots,
            &default_workspace,
            allow_outside,
            options,
        )
    })
    .await
    .unwrap_or_else(|error| {
        Err(WorkspaceResolutionError::Invalid(format!(
            "workspace resolution worker failed: {error}"
        )))
    })
}
