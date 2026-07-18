use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    domain::{TerminalPermissionMode, TerminalToolConfig, ToolResult},
    fsutil,
};

pub const MULTI_EDIT_TOOL_NAME: &str = "multi_edit_file";
/// Stable always-allow grant key (not the human-readable approval summary).
pub const MULTI_EDIT_GRANT_COMMAND: &str = "multi_edit_file";

const MAX_OPERATIONS: usize = 32;
const MAX_EDITS_PER_FILE: usize = 64;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_OLD_STRING_BYTES: usize = 200_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiEditFileArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub edits: Vec<MultiEditFileEdit>,
    #[serde(default)]
    pub files: Vec<MultiEditFileSpec>,
    #[serde(default)]
    pub operations: Vec<MultiEditFileOperation>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiEditFileSpec {
    pub path: String,
    pub edits: Vec<MultiEditFileEdit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiEditFileEdit {
    #[serde(alias = "oldString")]
    pub old_string: String,
    #[serde(alias = "newString")]
    pub new_string: String,
    #[serde(default, alias = "replaceAll")]
    pub replace_all: bool,
    #[serde(default, alias = "startLine")]
    pub start_line: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MultiEditFileOperation {
    #[serde(alias = "update")]
    Edit {
        path: String,
        edits: Vec<MultiEditFileEdit>,
    },
    Create {
        path: String,
        content: String,
        #[serde(default)]
        overwrite: bool,
    },
    Delete {
        path: String,
    },
    #[serde(alias = "rename")]
    Move {
        #[serde(alias = "fromPath", alias = "from_path")]
        from: String,
        #[serde(alias = "toPath", alias = "to_path")]
        to: String,
        #[serde(default)]
        overwrite: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEditChange {
    Add {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        overwritten: Option<bool>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        move_path: Option<String>,
        replacements: usize,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct MultiEditSummary {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub moved: Vec<String>,
}

#[derive(Debug)]
struct FileEditOutcome {
    summary: MultiEditSummary,
    changes: Vec<FileEditChange>,
    warnings: Vec<String>,
}

#[derive(Debug)]
enum StagedFileOperation {
    Update {
        path: PathBuf,
        old_content: String,
        new_content: String,
        replacements: usize,
    },
    Create {
        path: PathBuf,
        content: String,
        overwritten: bool,
        previous_content: Option<String>,
    },
    Delete {
        path: PathBuf,
        content: String,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        content: String,
        overwritten_content: Option<String>,
    },
}

#[derive(Debug)]
pub struct MultiEditRequest {
    pub args: MultiEditFileArgs,
    pub approved: bool,
}

pub async fn execute_multi_edit(
    request: &MultiEditRequest,
    config: &TerminalToolConfig,
    default_workspace: &Path,
) -> ToolResult {
    if !config.enabled {
        return ToolResult::Blocked {
            reason: "File edit tool is disabled because the terminal tool group is disabled"
                .to_string(),
        };
    }

    let operations = match normalize_multi_edit_operations(&request.args) {
        Ok(ops) => ops,
        Err(message) => return ToolResult::Error { message },
    };
    if operations.is_empty() {
        return ToolResult::Error {
            message: "multi_edit_file requires path+edits, files, or operations".to_string(),
        };
    }

    let full_access = config.permission_mode == TerminalPermissionMode::FullAccess;
    let workspace = match resolve_edit_workspace(
        request.args.cwd.as_deref(),
        config,
        default_workspace,
        full_access,
    ) {
        Ok(workspace) => workspace,
        Err(message) => return ToolResult::Blocked { reason: message },
    };

    let affected = match collect_affected_paths(&workspace.cwd, &operations) {
        Ok(paths) => paths,
        Err(message) => return ToolResult::Error { message },
    };

    if let Err(reason) = enforce_path_policy(&affected, &workspace.roots, full_access) {
        return ToolResult::Blocked { reason };
    }

    let context_granted = has_multi_edit_grant(config, &workspace, request.args.cwd.as_deref());
    if !full_access && !context_granted && !request.approved {
        let summary = build_approval_summary(&operations);
        return ToolResult::PendingApproval {
            command: summary,
            approval_id: Uuid::new_v4().to_string(),
        };
    }

    let started = Instant::now();
    let outcome = match apply_multi_edit_file(&workspace.cwd, &operations) {
        Ok(outcome) => outcome,
        Err(message) => return ToolResult::Error { message },
    };

    let payload = serde_json::json!({
        "status": "completed",
        "tool": MULTI_EDIT_TOOL_NAME,
        "cwd": workspace.cwd.display().to_string(),
        "summary": outcome.summary,
        "changes": outcome.changes,
        "warnings": outcome.warnings,
        "permission_mode": match config.permission_mode {
            TerminalPermissionMode::RequestApproval => "request_approval",
            TerminalPermissionMode::FullAccess => "full_access",
        },
    });

    ToolResult::Completed {
        stdout: serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
        stderr: String::new(),
        exit_code: 0,
        duration_ms: started.elapsed().as_millis() as u64,
        runtime: Some("builtin_multi_edit".to_string()),
        output_cursor: 0,
    }
}

fn has_multi_edit_grant(
    config: &TerminalToolConfig,
    workspace: &ResolvedEditWorkspace,
    requested_cwd: Option<&str>,
) -> bool {
    let now = chrono::Utc::now();
    let mut candidates = vec![
        workspace.root.display().to_string(),
        workspace.cwd.display().to_string(),
    ];
    if let Some(cwd) = requested_cwd.map(str::trim).filter(|value| !value.is_empty()) {
        candidates.push(cwd.to_string());
    }
    config.approval_grants.iter().any(|grant| {
        if grant.command != MULTI_EDIT_GRANT_COMMAND {
            return false;
        }
        if grant.permission_mode != TerminalPermissionMode::RequestApproval {
            return false;
        }
        if !grant.expires_at.is_some_and(|expires_at| expires_at > now) {
            return false;
        }
        match grant.cwd.as_deref() {
            None => true,
            Some(granted_cwd) => candidates.iter().any(|candidate| {
                fsutil::paths_equal(Path::new(candidate), Path::new(granted_cwd))
                    || candidate == granted_cwd
            }),
        }
    })
}

struct ResolvedEditWorkspace {
    root: PathBuf,
    cwd: PathBuf,
    roots: Vec<PathBuf>,
}

fn resolve_edit_workspace(
    cwd: Option<&str>,
    config: &TerminalToolConfig,
    default_workspace: &Path,
    allow_outside: bool,
) -> Result<ResolvedEditWorkspace, String> {
    let roots = if config.allowed_cwd_roots.is_empty() {
        vec![default_workspace.to_path_buf()]
    } else {
        config.allowed_cwd_roots.iter().map(PathBuf::from).collect()
    };
    let roots = roots
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err("No valid workspace root is configured".to_string());
    }

    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .and_then(|path| std::fs::canonicalize(path).ok());
    if roots.iter().any(|root| {
        root.parent().is_none()
            || home
                .as_ref()
                .is_some_and(|home| fsutil::paths_equal(home, root))
    }) {
        return Err(
            "Filesystem roots and the entire user home directory cannot be workspace roots"
                .to_string(),
        );
    }

    let requested = match cwd.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                roots[0].join(path)
            }
        }
        None => roots[0].clone(),
    };
    let canonical_cwd = std::fs::canonicalize(&requested).map_err(|error| {
        format!(
            "Working directory '{}' does not exist or cannot be accessed: {error}",
            requested.display()
        )
    })?;
    let matching_root = roots
        .iter()
        .find(|root| fsutil::path_is_within(&canonical_cwd, root))
        .cloned();
    if matching_root.is_none() && !allow_outside {
        return Err(format!(
            "Working directory '{}' is outside the configured workspace roots",
            canonical_cwd.display()
        ));
    }
    Ok(ResolvedEditWorkspace {
        root: matching_root.unwrap_or_else(|| roots[0].clone()),
        cwd: canonical_cwd,
        roots,
    })
}

fn enforce_path_policy(
    paths: &[PathBuf],
    roots: &[PathBuf],
    allow_outside: bool,
) -> Result<(), String> {
    if allow_outside {
        return Ok(());
    }
    for path in paths {
        let candidate = if path.exists() {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())
        } else if let Some(parent) = path.parent() {
            let canonical_parent =
                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            canonical_parent.join(path.file_name().unwrap_or_default())
        } else {
            path.clone()
        };
        if !roots
            .iter()
            .any(|root| fsutil::path_is_within(&candidate, root))
        {
            return Err(format!(
                "Path '{}' is outside the configured workspace roots",
                path.display()
            ));
        }
    }
    Ok(())
}

fn normalize_multi_edit_operations(
    args: &MultiEditFileArgs,
) -> Result<Vec<MultiEditFileOperation>, String> {
    let mut operations = Vec::new();
    if !args.operations.is_empty() {
        operations.extend(args.operations.clone());
    }
    if !args.files.is_empty() {
        for file in &args.files {
            operations.push(MultiEditFileOperation::Edit {
                path: file.path.clone(),
                edits: file.edits.clone(),
            });
        }
    }
    if let Some(path) = args
        .path
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if args.edits.is_empty() {
            return Err("path was provided without edits".to_string());
        }
        operations.push(MultiEditFileOperation::Edit {
            path: path.to_string(),
            edits: args.edits.clone(),
        });
    }

    if operations.len() > MAX_OPERATIONS {
        return Err(format!(
            "multi_edit_file supports at most {MAX_OPERATIONS} operations per call"
        ));
    }

    for (index, operation) in operations.iter().enumerate() {
        validate_operation(index + 1, operation)?;
    }
    Ok(operations)
}

fn validate_operation(index: usize, operation: &MultiEditFileOperation) -> Result<(), String> {
    match operation {
        MultiEditFileOperation::Edit { path, edits } => {
            validate_path_string(index, path)?;
            if edits.is_empty() {
                return Err(format!("operation {index} has empty edits"));
            }
            if edits.len() > MAX_EDITS_PER_FILE {
                return Err(format!(
                    "operation {index} has more than {MAX_EDITS_PER_FILE} edits"
                ));
            }
            for (edit_index, edit) in edits.iter().enumerate() {
                if edit.old_string.is_empty() {
                    return Err(format!(
                        "operation {index} edit {} has empty old_string",
                        edit_index + 1
                    ));
                }
                if edit.old_string == edit.new_string {
                    return Err(format!(
                        "operation {index} edit {} old_string and new_string are identical",
                        edit_index + 1
                    ));
                }
                if edit.old_string.len() > MAX_OLD_STRING_BYTES
                    || edit.new_string.len() > MAX_CONTENT_BYTES
                {
                    return Err(format!(
                        "operation {index} edit {} exceeds size limits",
                        edit_index + 1
                    ));
                }
            }
        }
        MultiEditFileOperation::Create { path, content, .. } => {
            validate_path_string(index, path)?;
            if content.len() > MAX_CONTENT_BYTES {
                return Err(format!("operation {index} create content is too large"));
            }
        }
        MultiEditFileOperation::Delete { path } => {
            validate_path_string(index, path)?;
        }
        MultiEditFileOperation::Move { from, to, .. } => {
            validate_path_string(index, from)?;
            validate_path_string(index, to)?;
        }
    }
    Ok(())
}

fn validate_path_string(index: usize, path: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Err(format!("operation {index} has an empty path"));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(format!("operation {index} path is too long"));
    }
    if path.contains('\0') {
        return Err(format!("operation {index} path contains a NUL byte"));
    }
    Ok(())
}

fn collect_affected_paths(
    cwd: &Path,
    operations: &[MultiEditFileOperation],
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for operation in operations {
        match operation {
            MultiEditFileOperation::Edit { path, .. }
            | MultiEditFileOperation::Create { path, .. }
            | MultiEditFileOperation::Delete { path } => {
                paths.push(resolve_operation_path(cwd, path)?);
            }
            MultiEditFileOperation::Move { from, to, .. } => {
                paths.push(resolve_operation_path(cwd, from)?);
                paths.push(resolve_operation_path(cwd, to)?);
            }
        }
    }
    Ok(paths)
}

fn resolve_operation_path(cwd: &Path, path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    let candidate = PathBuf::from(path);
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };
    Ok(normalize_logical_path(&joined))
}

fn normalize_logical_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn build_approval_summary(operations: &[MultiEditFileOperation]) -> String {
    let mut parts = Vec::new();
    for operation in operations.iter().take(8) {
        parts.push(match operation {
            MultiEditFileOperation::Edit { path, edits } => {
                format!("edit {path} ({} replacements)", edits.len())
            }
            MultiEditFileOperation::Create { path, .. } => format!("create {path}"),
            MultiEditFileOperation::Delete { path } => format!("delete {path}"),
            MultiEditFileOperation::Move { from, to, .. } => format!("move {from} -> {to}"),
        });
    }
    if operations.len() > 8 {
        parts.push(format!("… +{} more", operations.len() - 8));
    }
    format!("{MULTI_EDIT_TOOL_NAME}: {}", parts.join("; "))
}

fn apply_multi_edit_file(
    cwd: &Path,
    operations: &[MultiEditFileOperation],
) -> Result<FileEditOutcome, String> {
    let mut touched = BTreeSet::new();
    let mut staged = Vec::new();
    let mut warnings = Vec::new();

    for (index, operation) in operations.iter().enumerate() {
        stage_operation(
            cwd,
            operation,
            index + 1,
            &mut touched,
            &mut staged,
            &mut warnings,
        )?;
    }

    let mut changes = Vec::new();
    let mut committed = Vec::new();
    for operation in staged {
        match commit_operation(&operation) {
            Ok(change) => {
                changes.push(change);
                committed.push(operation);
            }
            Err(error) => {
                let rollback = rollback_committed(&committed);
                warnings.extend(rollback);
                return Err(format!(
                    "{error}; rolled back {} earlier change(s)",
                    committed.len()
                ));
            }
        }
    }

    if changes.is_empty() {
        return Err("multi_edit_file produced no changes".to_string());
    }

    Ok(FileEditOutcome {
        summary: summarize_changes(&changes),
        changes,
        warnings,
    })
}

fn stage_operation(
    cwd: &Path,
    operation: &MultiEditFileOperation,
    index: usize,
    touched: &mut BTreeSet<String>,
    staged: &mut Vec<StagedFileOperation>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    match operation {
        MultiEditFileOperation::Edit { path, edits } => {
            let target = resolve_operation_path(cwd, path)?;
            ensure_unique(touched, &target)?;
            if target.is_dir() {
                return Err(format!("operation {index} target is a directory: {path}"));
            }
            let original = std::fs::read_to_string(&target)
                .map_err(|error| format!("operation {index} failed to read {path}: {error}"))?;
            if original.len() > MAX_CONTENT_BYTES {
                return Err(format!("operation {index} file is too large: {path}"));
            }
            let endings = detect_line_endings(&original);
            let original_lf = normalize_to_lf(&original);
            let (updated_lf, replacements) = apply_edits_to_lf(&original_lf, edits, path, index)?;
            if updated_lf == original_lf {
                return Err(format!(
                    "operation {index} edit for {path} produced no changes"
                ));
            }
            let updated = restore_line_endings(&updated_lf, endings);
            if let Some(warning) = delimiter_warning(path, &original, &updated) {
                warnings.push(warning);
            }
            staged.push(StagedFileOperation::Update {
                path: target,
                old_content: original,
                new_content: updated,
                replacements,
            });
        }
        MultiEditFileOperation::Create {
            path,
            content,
            overwrite,
        } => {
            let target = resolve_operation_path(cwd, path)?;
            ensure_unique(touched, &target)?;
            if target.is_dir() {
                return Err(format!(
                    "operation {index} create target is a directory: {path}"
                ));
            }
            let previous = match std::fs::read_to_string(&target) {
                Ok(existing) => {
                    if !overwrite {
                        return Err(format!(
                            "operation {index} create target already exists: {path}"
                        ));
                    }
                    Some(existing)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(format!(
                        "operation {index} failed to inspect {path}: {error}"
                    ));
                }
            };
            staged.push(StagedFileOperation::Create {
                path: target,
                content: content.clone(),
                overwritten: previous.is_some(),
                previous_content: previous,
            });
        }
        MultiEditFileOperation::Delete { path } => {
            let target = resolve_operation_path(cwd, path)?;
            ensure_unique(touched, &target)?;
            if target.is_dir() {
                return Err(format!(
                    "operation {index} refuses to delete directory: {path}"
                ));
            }
            let content = std::fs::read_to_string(&target).map_err(|error| {
                format!("operation {index} failed to read {path} for delete: {error}")
            })?;
            staged.push(StagedFileOperation::Delete {
                path: target,
                content,
            });
        }
        MultiEditFileOperation::Move { from, to, overwrite } => {
            let from_path = resolve_operation_path(cwd, from)?;
            let to_path = resolve_operation_path(cwd, to)?;
            ensure_unique(touched, &from_path)?;
            ensure_unique(touched, &to_path)?;
            if from_path.is_dir() || to_path.is_dir() {
                return Err(format!(
                    "operation {index} move only supports files: {from} -> {to}"
                ));
            }
            let content = std::fs::read_to_string(&from_path).map_err(|error| {
                format!("operation {index} failed to read move source {from}: {error}")
            })?;
            let overwritten = match std::fs::read_to_string(&to_path) {
                Ok(existing) => {
                    if !overwrite {
                        return Err(format!(
                            "operation {index} move target already exists: {to}"
                        ));
                    }
                    Some(existing)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(format!(
                        "operation {index} failed to inspect move target {to}: {error}"
                    ));
                }
            };
            staged.push(StagedFileOperation::Move {
                from: from_path,
                to: to_path,
                content,
                overwritten_content: overwritten,
            });
        }
    }
    Ok(())
}

fn ensure_unique(touched: &mut BTreeSet<String>, path: &Path) -> Result<(), String> {
    let key = fsutil::path_identity_key(path);
    if !touched.insert(key) {
        return Err(format!(
            "path appears in multiple operations in one multi_edit_file call: {}",
            path.display()
        ));
    }
    Ok(())
}

fn commit_operation(operation: &StagedFileOperation) -> Result<FileEditChange, String> {
    match operation {
        StagedFileOperation::Update {
            path,
            new_content,
            replacements,
            ..
        } => {
            write_text_file(path, new_content)?;
            Ok(FileEditChange::Update {
                path: path.display().to_string(),
                move_path: None,
                replacements: *replacements,
            })
        }
        StagedFileOperation::Create {
            path,
            content,
            overwritten,
            ..
        } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!("failed to create parent for {}: {error}", path.display())
                })?;
            }
            write_text_file(path, content)?;
            Ok(FileEditChange::Add {
                path: path.display().to_string(),
                overwritten: Some(*overwritten),
            })
        }
        StagedFileOperation::Delete { path, .. } => {
            std::fs::remove_file(path)
                .map_err(|error| format!("failed to delete {}: {error}", path.display()))?;
            Ok(FileEditChange::Delete {
                path: path.display().to_string(),
            })
        }
        StagedFileOperation::Move {
            from, to, content, ..
        } => {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!("failed to create parent for {}: {error}", to.display())
                })?;
            }
            write_text_file(to, content)?;
            std::fs::remove_file(from).map_err(|error| {
                format!("failed to remove move source {}: {error}", from.display())
            })?;
            Ok(FileEditChange::Update {
                path: from.display().to_string(),
                move_path: Some(to.display().to_string()),
                replacements: 0,
            })
        }
    }
}

fn rollback_committed(committed: &[StagedFileOperation]) -> Vec<String> {
    let mut warnings = Vec::new();
    for operation in committed.iter().rev() {
        let result = match operation {
            StagedFileOperation::Update {
                path, old_content, ..
            } => write_text_file(path, old_content),
            StagedFileOperation::Create {
                path,
                overwritten,
                previous_content,
                ..
            } => {
                if *overwritten {
                    if let Some(previous) = previous_content {
                        write_text_file(path, previous)
                    } else {
                        Ok(())
                    }
                } else {
                    std::fs::remove_file(path).map_err(|error| {
                        format!("rollback failed to remove {}: {error}", path.display())
                    })
                }
            }
            StagedFileOperation::Delete { path, content } => write_text_file(path, content),
            StagedFileOperation::Move {
                from,
                to,
                content,
                overwritten_content,
            } => {
                let restore_from = write_text_file(from, content);
                let restore_to = if let Some(previous) = overwritten_content {
                    write_text_file(to, previous)
                } else {
                    std::fs::remove_file(to).map_err(|error| {
                        format!("rollback failed to remove {}: {error}", to.display())
                    })
                };
                restore_from.and(restore_to)
            }
        };
        if let Err(error) = result {
            warnings.push(error);
        }
    }
    warnings
}

fn write_text_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create parent for {}: {error}", path.display()))?;
    }
    let temp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "edit".to_string()),
        Uuid::new_v4().simple()
    );
    let temp_path = match path.parent() {
        Some(parent) => parent.join(&temp_name),
        None => PathBuf::from(&temp_name),
    };
    let write_result = (|| {
        std::fs::write(&temp_path, content)?;
        // Windows rename does not replace an existing destination.
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&temp_path, path)?;
        Ok::<(), std::io::Error>(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result.map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn apply_edits_to_lf(
    content: &str,
    edits: &[MultiEditFileEdit],
    path: &str,
    operation_index: usize,
) -> Result<(String, usize), String> {
    let mut current = content.to_string();
    let mut total = 0usize;
    for (edit_index, edit) in edits.iter().enumerate() {
        let old = normalize_to_lf(&edit.old_string);
        let new = normalize_to_lf(&edit.new_string);
        if old.is_empty() {
            return Err(format!(
                "operation {operation_index} edit {} has empty old_string",
                edit_index + 1
            ));
        }
        if old == new {
            return Err(format!(
                "operation {operation_index} edit {} is a no-op",
                edit_index + 1
            ));
        }
        let matches: Vec<_> = current.match_indices(&old).map(|(idx, _)| idx).collect();
        if matches.is_empty() {
            return Err(format!(
                "operation {operation_index} edit {} old_string not found in {path}",
                edit_index + 1
            ));
        }
        if edit.replace_all {
            current = current.replace(&old, &new);
            total = total.saturating_add(matches.len());
            continue;
        }
        if matches.len() > 1 {
            let Some(start_line) = edit.start_line else {
                return Err(format!(
                    "operation {operation_index} edit {} old_string matches {} times in {path}; set replace_all=true or provide startLine",
                    edit_index + 1,
                    matches.len()
                ));
            };
            let chosen = choose_closest_match(&current, &matches, start_line).ok_or_else(|| {
                format!(
                    "operation {operation_index} edit {} could not resolve ambiguous match near line {start_line} in {path}",
                    edit_index + 1
                )
            })?;
            current.replace_range(chosen..chosen + old.len(), &new);
            total = total.saturating_add(1);
        } else {
            let idx = matches[0];
            current.replace_range(idx..idx + old.len(), &new);
            total = total.saturating_add(1);
        }
    }
    Ok((current, total))
}

fn choose_closest_match(content: &str, matches: &[usize], start_line: usize) -> Option<usize> {
    if start_line == 0 {
        return None;
    }
    matches.iter().copied().min_by_key(|idx| {
        let line = content[..*idx].bytes().filter(|byte| *byte == b'\n').count() + 1;
        line.abs_diff(start_line)
    })
}

fn summarize_changes(changes: &[FileEditChange]) -> MultiEditSummary {
    let mut summary = MultiEditSummary {
        added: Vec::new(),
        modified: Vec::new(),
        deleted: Vec::new(),
        moved: Vec::new(),
    };
    for change in changes {
        match change {
            FileEditChange::Add { path, .. } => summary.added.push(path.clone()),
            FileEditChange::Delete { path } => summary.deleted.push(path.clone()),
            FileEditChange::Update {
                path, move_path, ..
            } => {
                if let Some(moved_to) = move_path {
                    summary.moved.push(format!("{path} -> {moved_to}"));
                } else {
                    summary.modified.push(path.clone());
                }
            }
        }
    }
    summary
}

fn normalize_to_lf(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

#[derive(Clone, Copy)]
enum LineEnding {
    Lf,
    Crlf,
}

fn detect_line_endings(value: &str) -> LineEnding {
    if value.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

fn restore_line_endings(value: &str, endings: LineEnding) -> String {
    match endings {
        LineEnding::Lf => value.to_string(),
        LineEnding::Crlf => value.replace('\n', "\r\n"),
    }
}

fn delimiter_warning(path: &str, before: &str, after: &str) -> Option<String> {
    let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
    for (open, close) in pairs {
        let before_balance = balance(before, open, close);
        let after_balance = balance(after, open, close);
        if before_balance == 0 && after_balance != 0 {
            return Some(format!(
                "warning: edit for {path} changed balance of {open}{close} delimiters"
            ));
        }
    }
    None
}

fn balance(value: &str, open: char, close: char) -> i32 {
    let mut score = 0i32;
    for ch in value.chars() {
        if ch == open {
            score += 1;
        } else if ch == close {
            score -= 1;
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TerminalToolConfig;

    fn temp_workspace() -> (tempfile::TempDir, TerminalToolConfig) {
        let dir = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            enabled: true,
            permission_mode: TerminalPermissionMode::FullAccess,
            allowed_cwd_roots: vec![dir.path().display().to_string()],
            ..TerminalToolConfig::default()
        };
        (dir, config)
    }

    #[tokio::test]
    async fn empty_roots_use_data_directory_as_default_workspace() {
        let data_dir = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            ..TerminalToolConfig::default()
        };

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    path: None,
                    edits: Vec::new(),
                    files: Vec::new(),
                    operations: vec![MultiEditFileOperation::Create {
                        path: "default-workspace.txt".to_string(),
                        content: "data directory".to_string(),
                        overwrite: false,
                    }],
                    cwd: None,
                },
                approved: true,
            },
            &config,
            data_dir.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::Completed { .. }));
        assert!(data_dir.path().join("default-workspace.txt").exists());
    }

    #[tokio::test]
    async fn multi_edit_applies_single_file_and_create() {
        let (dir, config) = temp_workspace();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {\n    println!(\"old\");\n}\n").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    path: None,
                    edits: Vec::new(),
                    files: Vec::new(),
                    operations: vec![
                        MultiEditFileOperation::Edit {
                            path: "a.rs".to_string(),
                            edits: vec![MultiEditFileEdit {
                                old_string: "old".to_string(),
                                new_string: "new".to_string(),
                                replace_all: false,
                                start_line: None,
                            }],
                        },
                        MultiEditFileOperation::Create {
                            path: "b.rs".to_string(),
                            content: "pub fn b() {}\n".to_string(),
                            overwrite: false,
                        },
                    ],
                    cwd: Some(dir.path().display().to_string()),
                },
                approved: true,
            },
            &config,
            dir.path(),
        )
        .await;

        match result {
            ToolResult::Completed { exit_code, .. } => assert_eq!(exit_code, 0),
            other => panic!("unexpected result: {other:?}"),
        }
        assert!(std::fs::read_to_string(&file).unwrap().contains("new"));
        assert!(dir.path().join("b.rs").exists());
    }

    #[tokio::test]
    async fn request_approval_mode_requires_approval() {
        let (dir, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    path: Some("a.txt".to_string()),
                    edits: vec![MultiEditFileEdit {
                        old_string: "hello".to_string(),
                        new_string: "world".to_string(),
                        replace_all: false,
                        start_line: None,
                    }],
                    files: Vec::new(),
                    operations: Vec::new(),
                    cwd: Some(dir.path().display().to_string()),
                },
                approved: false,
            },
            &config,
            dir.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::PendingApproval { .. }));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn always_allow_grant_skips_approval() {
        let (dir, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        let root = std::fs::canonicalize(dir.path()).unwrap();
        config
            .approval_grants
            .push(crate::domain::TerminalApprovalGrant {
                command: MULTI_EDIT_GRANT_COMMAND.to_string(),
                cwd: Some(root.display().to_string()),
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(chrono::Utc::now()),
                expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(24)),
            });
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    path: Some("a.txt".to_string()),
                    edits: vec![MultiEditFileEdit {
                        old_string: "hello".to_string(),
                        new_string: "world".to_string(),
                        replace_all: false,
                        start_line: None,
                    }],
                    files: Vec::new(),
                    operations: Vec::new(),
                    cwd: Some(dir.path().display().to_string()),
                },
                approved: false,
            },
            &config,
            dir.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::Completed { .. }));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "world"
        );
    }

    #[test]
    fn ambiguous_edit_fails_without_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x\nx\n").unwrap();
        let err = apply_multi_edit_file(
            dir.path(),
            &[MultiEditFileOperation::Edit {
                path: "a.txt".to_string(),
                edits: vec![MultiEditFileEdit {
                    old_string: "x".to_string(),
                    new_string: "y".to_string(),
                    replace_all: false,
                    start_line: None,
                }],
            }],
        )
        .unwrap_err();
        assert!(err.contains("matches"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "x\nx\n");
    }
}
