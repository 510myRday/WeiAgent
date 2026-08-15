use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex as StdMutex},
    time::Instant,
};

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, Deserializer},
};
use uuid::Uuid;

use crate::{
    domain::{TerminalPermissionMode, TerminalToolConfig, ToolResult},
    fsutil,
    tools::{
        terminal::SandboxPermissions,
        workspace::{self, WorkspaceResolutionError, expand_home_path},
    },
};

pub const MULTI_EDIT_TOOL_NAME: &str = "multi_edit_file";
/// Stable always-allow grant key (not the human-readable approval summary).
pub const MULTI_EDIT_GRANT_COMMAND: &str = "multi_edit_file";

const MAX_OPERATIONS: usize = 32;
const MAX_EDITS_PER_FILE: usize = 64;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_OLD_STRING_BYTES: usize = 200_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiEditFileArgs {
    /// Legacy single-file form: `path` plus top-level `edits`. Not advertised
    /// in the schema, but accepted so model calls shaped after common edit
    /// tools still work instead of failing deserialization.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_json_string")]
    pub edits: Vec<MultiEditFileEdit>,
    /// Legacy multi-file form: `files[]`, each with its own path and edits.
    #[serde(default, deserialize_with = "deserialize_vec_or_json_string")]
    pub files: Vec<MultiEditFileSpec>,
    #[serde(default, deserialize_with = "deserialize_vec_or_json_string")]
    pub operations: Vec<MultiEditFileOperation>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub sandbox_permissions: SandboxPermissions,
    #[serde(default)]
    pub justification: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiEditFileSpec {
    pub path: String,
    #[serde(default, deserialize_with = "deserialize_vec_or_json_string")]
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
        #[serde(deserialize_with = "deserialize_vec_or_json_string")]
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

/// Tool-call adapters occasionally stringify array-valued arguments before
/// handing them to the application. Accept the normal JSON array as well as
/// that representation, while keeping the internal model strongly typed.
fn deserialize_vec_or_json_string<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let value = match value {
        serde_json::Value::String(raw) => {
            let mut parsed = serde_json::from_str::<serde_json::Value>(&raw).map_err(|error| {
                de::Error::custom(format!("expected a JSON array string: {error}"))
            })?;
            for _ in 0..2 {
                let serde_json::Value::String(nested) = parsed else {
                    break;
                };
                parsed = serde_json::from_str(&nested).map_err(|error| {
                    de::Error::custom(format!("expected a JSON array string: {error}"))
                })?;
            }
            parsed
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => value,
        serde_json::Value::Null => return Ok(Vec::new()),
        other => {
            return Err(de::Error::custom(format!(
                "expected an array or JSON array string, got {other}"
            )));
        }
    };

    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<T>, _>>()
            .map_err(de::Error::custom),
        serde_json::Value::Object(item) => serde_json::from_value(serde_json::Value::Object(item))
            .map(|item| vec![item])
            .map_err(de::Error::custom),
        _ => unreachable!("validated array-like JSON value"),
    }
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

/// Failure that still carries the warnings collected so far, most importantly
/// rollback failures. Collapsing this to a plain message would silently drop
/// the only signal that the disk may be in a partially rolled-back state.
#[derive(Debug)]
struct FileEditFailure {
    message: String,
    warnings: Vec<String>,
}

fn render_edit_failure(failure: FileEditFailure) -> String {
    if failure.warnings.is_empty() {
        return failure.message;
    }
    let mut message = failure.message;
    message.push_str("\nwarnings:");
    for warning in &failure.warnings {
        message.push_str("\n- ");
        message.push_str(warning);
    }
    message
}

/// Per-path async locks shared by every multi_edit_file call in this process.
/// Two concurrent calls (e.g. from different conversations) that touch the
/// same file would otherwise both stage from the same on-disk version and
/// silently lose one side's changes, or the later writer would fail with an
/// old_string mismatch. Keys come from `fsutil::path_identity_key`, so case
/// variants of one Windows path share a lock.
static FILE_LOCKS: LazyLock<StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Locks are acquired in sorted key order so overlapping calls cannot
/// deadlock against each other.
async fn acquire_file_locks(paths: &[PathBuf]) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
    let mut keys: Vec<String> = paths
        .iter()
        .map(|path| fsutil::path_identity_key(path))
        .collect();
    keys.sort();
    keys.dedup();

    let mutexes: Vec<Arc<tokio::sync::Mutex<()>>> = {
        let mut table = FILE_LOCKS.lock().expect("file lock table poisoned");
        keys.iter()
            .map(|key| {
                table
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                    .clone()
            })
            .collect()
    };

    let mut guards = Vec::with_capacity(mutexes.len());
    for mutex in mutexes {
        guards.push(mutex.lock_owned().await);
    }
    guards
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

pub fn parse_multi_edit_file_args(
    arguments: serde_json::Value,
) -> Result<MultiEditFileArgs, String> {
    let arguments = normalize_multi_edit_arguments(arguments);
    match serde_json::from_value(arguments.clone()) {
        Ok(args) => Ok(args),
        Err(_) => {
            if let Some(index) = missing_operation_type(&arguments) {
                return Err(format!(
                    "multi_edit_file operations[{index}] is missing required field `type`. Put `type` inside each operation object, for example {{\"type\":\"edit\",\"path\":\"src/file.rs\",\"edits\":[{{\"old_string\":\"old\",\"new_string\":\"new\"}}]}}."
                ));
            }
            Err("Invalid multi_edit_file arguments. Use operations as an array of objects; each operation needs type=edit, create, delete, or move, plus its operation-specific fields.".to_string())
        }
    }
}

fn normalize_multi_edit_arguments(mut arguments: serde_json::Value) -> serde_json::Value {
    let Some(object) = arguments.as_object_mut() else {
        return arguments;
    };
    let Some(operations) = object
        .get_mut("operations")
        .and_then(|value| value.as_array_mut())
    else {
        return arguments;
    };
    for operation in operations {
        let serde_json::Value::Object(operation) = operation else {
            continue;
        };
        if operation.contains_key("type") {
            continue;
        }
        let inferred_type = if operation.contains_key("path") && operation.contains_key("edits") {
            Some("edit")
        } else if operation.contains_key("path") && operation.contains_key("content") {
            Some("create")
        } else if operation.contains_key("from") && operation.contains_key("to") {
            Some("move")
        } else {
            None
        };
        if let Some(inferred_type) = inferred_type {
            operation.insert(
                "type".to_string(),
                serde_json::Value::String(inferred_type.to_string()),
            );
        }
    }
    arguments
}

fn missing_operation_type(arguments: &serde_json::Value) -> Option<usize> {
    arguments
        .get("operations")
        .and_then(serde_json::Value::as_array)
        .and_then(|operations| {
            operations
                .iter()
                .enumerate()
                .find_map(|(index, operation)| {
                    operation
                        .as_object()
                        .filter(|operation| !operation.contains_key("type"))
                        .map(|_| index + 1)
                })
        })
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
            code: None,
            details: None,
        };
    }

    let operations = match normalize_multi_edit_operations(&request.args) {
        Ok(ops) => ops,
        Err(message) => return ToolResult::Error { message },
    };

    let full_access = config.permission_mode == TerminalPermissionMode::FullAccess;
    let escalation_requested =
        request.args.sandbox_permissions == SandboxPermissions::RequireEscalated;
    let allow_outside_workspace = full_access || escalation_requested || request.approved;
    let workspace = match workspace::resolve_workspace_async(
        request.args.cwd.clone(),
        config.allowed_cwd_roots.clone(),
        default_workspace.to_path_buf(),
        allow_outside_workspace,
        workspace::WorkspaceResolveOptions {
            expand_home: true,
            blocked_message:
                "Filesystem roots and the entire user home directory cannot be workspace roots",
        },
    )
    .await
    {
        Ok(workspace) => workspace,
        Err(WorkspaceResolutionError::Outside) => {
            return ToolResult::PendingApproval {
                command: build_approval_summary(&operations),
                approval_id: Uuid::new_v4().to_string(),
            };
        }
        Err(WorkspaceResolutionError::Invalid(message))
        | Err(WorkspaceResolutionError::Blocked(message)) => {
            return ToolResult::Blocked {
                reason: message,
                code: None,
                details: None,
            };
        }
    };

    let affected = match collect_affected_paths(&workspace.cwd, &operations) {
        Ok(paths) => paths,
        Err(message) => return ToolResult::Error { message },
    };

    if let Err(PathPolicyError::Outside) =
        enforce_path_policy(&affected, &workspace.roots, allow_outside_workspace)
    {
        return ToolResult::PendingApproval {
            command: build_approval_summary(&operations),
            approval_id: Uuid::new_v4().to_string(),
        };
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
    // Hold the per-path locks for the whole stage+commit window, and move the
    // synchronous filesystem work off the async runtime so a large batch does
    // not stall unrelated tasks on the shared worker threads.
    let _file_locks = acquire_file_locks(&affected).await;
    let cwd_for_apply = workspace.cwd.clone();
    let operations_for_apply = operations.clone();
    let outcome = match tokio::task::spawn_blocking(move || {
        apply_multi_edit_file(&cwd_for_apply, &operations_for_apply)
    })
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(failure)) => {
            return ToolResult::Error {
                message: render_edit_failure(failure),
            };
        }
        Err(join_error) => {
            return ToolResult::Error {
                message: format!("multi_edit_file worker panicked: {join_error}"),
            };
        }
    };

    let payload = serde_json::json!({
        "status": "completed",
        "tool": MULTI_EDIT_TOOL_NAME,
        "cwd": fsutil::display_path(&workspace.cwd),
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
        structured_content: None,
        is_error: false,
    }
}

fn has_multi_edit_grant(
    config: &TerminalToolConfig,
    workspace: &workspace::ResolvedWorkspace,
    requested_cwd: Option<&str>,
) -> bool {
    let now = chrono::Utc::now();
    let mut candidates = vec![
        workspace.root().display().to_string(),
        workspace.cwd.display().to_string(),
    ];
    if let Some(cwd) = requested_cwd
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
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

fn enforce_path_policy(
    paths: &[PathBuf],
    roots: &[PathBuf],
    allow_outside: bool,
) -> Result<(), PathPolicyError> {
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
            return Err(PathPolicyError::Outside);
        }
    }
    Ok(())
}

#[derive(Debug)]
enum PathPolicyError {
    Outside,
}

fn normalize_multi_edit_operations(
    args: &MultiEditFileArgs,
) -> Result<Vec<MultiEditFileOperation>, String> {
    let mut operations = Vec::new();

    if let Some(path) = args
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        operations.push(MultiEditFileOperation::Edit {
            path: path.to_string(),
            edits: args.edits.clone(),
        });
    } else if !args.edits.is_empty() {
        return Err("path is required when using top-level edits".to_string());
    }

    for file in &args.files {
        operations.push(MultiEditFileOperation::Edit {
            path: file.path.clone(),
            edits: file.edits.clone(),
        });
    }

    operations.extend(args.operations.clone());

    if operations.is_empty() {
        return Err(
            "multi_edit_file requires operations (or legacy path+edits / files)".to_string(),
        );
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
    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(fsutil::path_identity_key(path)));
    Ok(paths)
}

fn resolve_operation_path(cwd: &Path, path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    let candidate = PathBuf::from(expand_home_path(path));
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
) -> Result<FileEditOutcome, FileEditFailure> {
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
        )
        .map_err(|message| FileEditFailure {
            message,
            warnings: Vec::new(),
        })?;
    }

    let mut changes = Vec::new();
    let mut committed: Vec<StagedFileOperation> = Vec::with_capacity(staged.len());
    for operation in staged {
        match commit_operation(&operation) {
            Ok(change) => {
                changes.push(change);
                committed.push(operation);
            }
            Err(error) => {
                let rollback_warnings = rollback_committed(&committed);
                // Only claim a clean rollback when every undo step succeeded;
                // otherwise say exactly how many steps failed and surface the
                // per-step details through the warnings.
                let message = if committed.is_empty() {
                    error
                } else if rollback_warnings.is_empty() {
                    format!("{error}; rolled back {} earlier change(s)", committed.len())
                } else {
                    format!(
                        "{error}; attempted rollback of {} earlier change(s) but {} rollback step(s) failed, files may be partially modified",
                        committed.len(),
                        rollback_warnings.len()
                    )
                };
                warnings.extend(rollback_warnings);
                return Err(FileEditFailure { message, warnings });
            }
        }
    }

    if changes.is_empty() {
        return Err(FileEditFailure {
            message: "multi_edit_file produced no changes".to_string(),
            warnings,
        });
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
            warnings.extend(collect_edit_warnings(&target, &original, &updated));
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
                Err(error) if is_path_not_found(&error) => None,
                Err(error) => {
                    return Err(format!(
                        "operation {index} failed to inspect {path}: {error}"
                    ));
                }
            };
            warnings.extend(collect_edit_warnings(
                &target,
                previous.as_deref().unwrap_or_default(),
                content,
            ));
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
        MultiEditFileOperation::Move {
            from,
            to,
            overwrite,
        } => {
            let from_path = resolve_operation_path(cwd, from)?;
            let to_path = resolve_operation_path(cwd, to)?;
            if fsutil::paths_equal(&from_path, &to_path) {
                return Err(format!(
                    "operation {index} move source and target are the same: {from}"
                ));
            }
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
                Err(error) if is_path_not_found(&error) => None,
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
                path: fsutil::display_path(path),
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
                path: fsutil::display_path(path),
                overwritten: Some(*overwritten),
            })
        }
        StagedFileOperation::Delete { path, .. } => {
            std::fs::remove_file(path)
                .map_err(|error| format!("failed to delete {}: {error}", path.display()))?;
            Ok(FileEditChange::Delete {
                path: fsutil::display_path(path),
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
                path: fsutil::display_path(from),
                move_path: Some(fsutil::display_path(to)),
                replacements: 0,
            })
        }
    }
}

/// Best-effort rollback of operations that already reached disk before a
/// later stage failed. Iterates in reverse order; every failed undo step is
/// reported individually and never aborts the remaining undo attempts.
fn rollback_committed(committed: &[StagedFileOperation]) -> Vec<String> {
    let mut warnings = Vec::new();
    for operation in committed.iter().rev() {
        match operation {
            StagedFileOperation::Update {
                path, old_content, ..
            } => {
                if let Err(error) = write_text_file(path, old_content) {
                    warnings.push(error);
                }
            }
            StagedFileOperation::Create {
                path,
                overwritten,
                previous_content,
                ..
            } => {
                let result = if *overwritten {
                    if let Some(previous) = previous_content {
                        write_text_file(path, previous)
                    } else {
                        Ok(())
                    }
                } else {
                    remove_file_tolerant(path)
                };
                if let Err(error) = result {
                    warnings.push(error);
                }
            }
            StagedFileOperation::Delete { path, content } => {
                if let Err(error) = write_text_file(path, content) {
                    warnings.push(error);
                }
            }
            StagedFileOperation::Move {
                from,
                to,
                content,
                overwritten_content,
            } => {
                if let Err(error) = write_text_file(from, content) {
                    warnings.push(error);
                }
                let restore_to = if let Some(previous) = overwritten_content {
                    write_text_file(to, previous)
                } else {
                    remove_file_tolerant(to)
                };
                if let Err(error) = restore_to {
                    warnings.push(error);
                }
            }
        }
    }
    warnings
}

/// Removing a file that is already gone is a successful undo, not a failure.
fn remove_file_tolerant(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "rollback failed to remove {}: {error}",
            path.display()
        )),
    }
}

/// Returns true if the IO error indicates the path (or a component of it)
/// does not exist. On Unix, traversing a path like `file.txt/child.txt`
/// where `file.txt` is a regular file yields ENOTDIR (raw OS error 20)
/// rather than ENOENT; Windows reports NotFound for the same situation.
fn is_path_not_found(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::NotFound {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(20) {
        return true;
    }
    false
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
    let mut applied_new_strings: Vec<String> = Vec::new();
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
        // Refuse edits whose old_string targets text that an earlier edit in
        // this same operation just produced: those matches did not exist in
        // the file the model read, so applying them silently rewrites
        // generated content in ways the model did not intend.
        for previous_new_string in &applied_new_strings {
            if previous_new_string.contains(&old) {
                return Err(format!(
                    "operation {operation_index} edit {} old_string is a substring of a previous new_string; split or reorder edits to avoid re-editing generated content",
                    edit_index + 1
                ));
            }
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
            applied_new_strings.push(new);
            continue;
        }
        if matches.len() > 1 {
            let Some(start_line) = edit.start_line else {
                return Err(format!(
                    "operation {operation_index} edit {} old_string matches {} times in {path}; set replace_all=true or provide start_line",
                    edit_index + 1,
                    matches.len()
                ));
            };
            let chosen =
                choose_closest_match(&current, &old, &matches, start_line).ok_or_else(|| {
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
        applied_new_strings.push(new);
    }
    Ok((current, total))
}

/// Pick the ambiguous match closest to the model-provided line hint. The hint
/// is scored against both the expected first line and the expected last line
/// of the old_string block, so hints that point anywhere near a multi-line
/// block (or drift after earlier edits shifted content) still resolve to the
/// intended occurrence. A hint of 0 is treated as the top of the file.
fn choose_closest_match(
    content: &str,
    old_string: &str,
    matches: &[usize],
    start_line: usize,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let expected = start_line.max(1);
    let old_line_count = old_string.split('\n').count().max(1);
    matches.iter().copied().min_by_key(|byte_index| {
        let actual_line = line_number_at_byte_index(content, *byte_index);
        let distance_to_start = actual_line.abs_diff(expected);
        let distance_to_end = actual_line.abs_diff(expected.saturating_add(old_line_count - 1));
        distance_to_start.min(distance_to_end)
    })
}

fn line_number_at_byte_index(content: &str, byte_index: usize) -> usize {
    content[..byte_index.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
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

/// Only CRLF pairs are folded for matching; a lone `\r` is real file content
/// and folding it would rewrite untouched regions on write-back.
fn normalize_to_lf(value: &str) -> String {
    value.replace("\r\n", "\n")
}

#[derive(Clone, Copy)]
enum LineEnding {
    Lf,
    Crlf,
}

/// Majority vote instead of `contains("\r\n")`: one stray CRLF in an
/// otherwise-LF file must not flip the whole file to CRLF on write-back.
fn detect_line_endings(value: &str) -> LineEnding {
    let crlf_count = value.match_indices("\r\n").count();
    let lf_count = value.bytes().filter(|byte| *byte == b'\n').count();
    if crlf_count > lf_count.saturating_sub(crlf_count) {
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

/// Post-edit hazard checks for code files. Warnings never block the write;
/// they flag likely mistakes (escaped `${` in TS/JS templates, delimiters
/// that became unbalanced) so the model can inspect before continuing.
fn collect_edit_warnings(path: &Path, old_content: &str, new_content: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return warnings;
    };
    let extension = extension.to_ascii_lowercase();
    let is_js_like = matches!(extension.as_str(), "js" | "jsx" | "ts" | "tsx");
    let is_checked_code = is_js_like || extension == "rs";

    if is_js_like && new_content.contains("\\${") {
        warnings.push(format!(
            "{} contains `\\${{`; TS/JS template expressions usually require `${{...}}` without the backslash.",
            path.to_string_lossy()
        ));
    }

    if is_checked_code {
        let old_issues = bracket_balance_issues(old_content, is_js_like);
        let new_issues = bracket_balance_issues(new_content, is_js_like);
        // Only warn when this edit introduced or changed the issues; a file
        // that was already unbalanced before the edit stays silent.
        if !new_issues.is_empty() && old_issues != new_issues {
            warnings.push(format!(
                "{} may have unbalanced delimiters after edit: {}",
                path.to_string_lossy(),
                new_issues.join("; ")
            ));
        }
    }

    warnings
}

/// Bracket balance scanner that skips comments and string literals so quoted
/// delimiters do not produce false positives. Reports at most five issues
/// with line and column positions.
fn bracket_balance_issues(content: &str, js_like: bool) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ScanState {
        Code,
        LineComment,
        BlockComment,
        DoubleString,
        SingleString,
        TemplateString,
    }

    let mut state = ScanState::Code;
    let mut escaped = false;
    let mut stack = Vec::<(char, usize, usize)>::new();
    let mut issues = Vec::<String>::new();
    let mut chars = content.chars().peekable();
    let mut line = 1usize;
    let mut column = 0usize;

    while let Some(ch) = chars.next() {
        column += 1;
        let next = chars.peek().copied();

        match state {
            ScanState::Code => match ch {
                '/' if next == Some('/') => {
                    chars.next();
                    column += 1;
                    state = ScanState::LineComment;
                }
                '/' if next == Some('*') => {
                    chars.next();
                    column += 1;
                    state = ScanState::BlockComment;
                }
                '"' => {
                    escaped = false;
                    state = ScanState::DoubleString;
                }
                '\'' if js_like => {
                    escaped = false;
                    state = ScanState::SingleString;
                }
                '`' if js_like => {
                    escaped = false;
                    state = ScanState::TemplateString;
                }
                '(' | '[' | '{' => stack.push((ch, line, column)),
                ')' | ']' | '}' => {
                    let expected = matching_open_delimiter(ch);
                    match stack.pop() {
                        Some((open, _, _)) if open == expected => {}
                        Some((open, open_line, open_column)) => issues.push(format!(
                            "line {line}, column {column}: found `{ch}` while `{open}` opened at line {open_line}, column {open_column}"
                        )),
                        None => issues.push(format!(
                            "line {line}, column {column}: unmatched closing `{ch}`"
                        )),
                    }
                }
                _ => {}
            },
            ScanState::LineComment => {
                if ch == '\n' {
                    state = ScanState::Code;
                }
            }
            ScanState::BlockComment => {
                if ch == '*' && next == Some('/') {
                    chars.next();
                    column += 1;
                    state = ScanState::Code;
                }
            }
            ScanState::DoubleString => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    state = ScanState::Code;
                }
            }
            ScanState::SingleString => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' {
                    state = ScanState::Code;
                }
            }
            ScanState::TemplateString => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '`' {
                    state = ScanState::Code;
                }
            }
        }

        if ch == '\n' {
            line += 1;
            column = 0;
            if state == ScanState::LineComment {
                state = ScanState::Code;
            }
        }
    }

    issues.extend(
        stack
            .into_iter()
            .rev()
            .map(|(open, open_line, open_column)| {
                format!("line {open_line}, column {open_column}: unclosed `{open}`")
            }),
    );
    issues.truncate(5);
    issues
}

fn matching_open_delimiter(close: char) -> char {
    match close {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TerminalToolConfig;

    #[test]
    fn accepts_stringified_array_arguments_from_tool_adapters() {
        let args: MultiEditFileArgs = serde_json::from_value(serde_json::json!({
            "operations": "[{\"type\":\"edit\",\"path\":\"a.txt\",\"edits\":[{\"old_string\":\"old\",\"new_string\":\"new\"}]}]"
        }))
        .expect("stringified operations should deserialize");

        assert_eq!(args.operations.len(), 1);
        assert!(matches!(
            args.operations[0],
            MultiEditFileOperation::Edit { .. }
        ));
    }

    #[test]
    fn infers_unambiguous_operation_type_when_model_omits_it() {
        let args = parse_multi_edit_file_args(serde_json::json!({
            "operations": [{
                "path": "a.txt",
                "edits": [{"old_string": "old", "new_string": "new"}]
            }]
        }))
        .expect("edit type should be inferred from path and edits");

        assert!(matches!(
            args.operations[0],
            MultiEditFileOperation::Edit { .. }
        ));
    }

    #[test]
    fn explains_where_missing_operation_type_belongs() {
        let error = parse_multi_edit_file_args(serde_json::json!({
            "operations": [{"path": "a.txt"}]
        }))
        .expect_err("an ambiguous operation should require type");

        assert!(error.contains("operations[1]"));
        assert!(error.contains("type"));
        assert!(error.contains("\"type\":\"edit\""));
    }

    #[test]
    fn accepts_camel_case_edit_aliases() {
        let args: MultiEditFileArgs = serde_json::from_value(serde_json::json!({
            "operations": [{
                "type": "edit",
                "path": "a.txt",
                "edits": [{"oldString": "old", "newString": "new", "replaceAll": true}]
            }]
        }))
        .expect("camelCase edit aliases should deserialize");

        let MultiEditFileOperation::Edit { edits, .. } = &args.operations[0] else {
            panic!("expected edit operation");
        };
        assert!(edits[0].replace_all);
    }

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
    async fn empty_roots_use_dedicated_directory_as_default_workspace() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::FullAccess,
            ..TerminalToolConfig::default()
        };

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Create {
                        path: "default-workspace.txt".to_string(),
                        content: "data directory".to_string(),
                        overwrite: false,
                    }],
                    cwd: None,
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
                },
                approved: true,
            },
            &config,
            workspace_dir.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::Completed { .. }));
        assert!(workspace_dir.path().join("default-workspace.txt").exists());
    }

    #[tokio::test]
    async fn configured_roots_do_not_replace_the_dedicated_default_workspace() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let configured_root = tempfile::tempdir().unwrap();
        let config = TerminalToolConfig {
            permission_mode: TerminalPermissionMode::RequestApproval,
            allowed_cwd_roots: vec![configured_root.path().display().to_string()],
            ..TerminalToolConfig::default()
        };

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Create {
                        path: "still-default-workspace.txt".to_string(),
                        content: "data directory".to_string(),
                        overwrite: false,
                    }],
                    cwd: None,
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
                },
                approved: true,
            },
            &config,
            workspace_dir.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::Completed { .. }));
        assert!(
            workspace_dir
                .path()
                .join("still-default-workspace.txt")
                .exists()
        );
        assert!(
            !configured_root
                .path()
                .join("still-default-workspace.txt")
                .exists()
        );

        let configured_result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Create {
                        path: "configured-root.txt".to_string(),
                        content: "configured directory".to_string(),
                        overwrite: false,
                    }],
                    cwd: Some(configured_root.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
                },
                approved: true,
            },
            &config,
            workspace_dir.path(),
        )
        .await;

        assert!(matches!(configured_result, ToolResult::Completed { .. }));
        assert!(configured_root.path().join("configured-root.txt").exists());
    }

    #[tokio::test]
    async fn multi_edit_applies_single_file_and_create() {
        let (dir, config) = temp_workspace();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {\n    println!(\"old\");\n}\n").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
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
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
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
                    operations: vec![MultiEditFileOperation::Edit {
                        path: "a.txt".to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(dir.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
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
                    operations: vec![MultiEditFileOperation::Edit {
                        path: "a.txt".to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(dir.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
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

    #[tokio::test]
    async fn outside_workspace_path_requests_approval_with_default_permissions() {
        let (workspace, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        std::fs::write(&target, "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Edit {
                        path: target.display().to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(workspace.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
                },
                approved: false,
            },
            &config,
            workspace.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::PendingApproval { .. }));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[tokio::test]
    async fn outside_workspace_cwd_requests_approval_with_default_permissions() {
        let (workspace, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        std::fs::write(&target, "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Edit {
                        path: "outside.txt".to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(outside.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
                },
                approved: false,
            },
            &config,
            workspace.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::PendingApproval { .. }));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[tokio::test]
    async fn outside_workspace_cwd_writes_after_internal_approval() {
        let (workspace, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        std::fs::write(&target, "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Edit {
                        path: "outside.txt".to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(outside.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::UseDefault,
                    justification: None,
                    ..Default::default()
                },
                approved: true,
            },
            &config,
            workspace.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::Completed { .. }));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "world");
    }

    #[tokio::test]
    async fn outside_workspace_path_requests_approval_when_escalated() {
        let (workspace, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        std::fs::write(&target, "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Edit {
                        path: target.display().to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(workspace.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::RequireEscalated,
                    justification: Some("edit project root outside data workspace".to_string()),
                    ..Default::default()
                },
                approved: false,
            },
            &config,
            workspace.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::PendingApproval { .. }));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[tokio::test]
    async fn outside_workspace_path_writes_after_approved_escalation() {
        let (workspace, mut config) = temp_workspace();
        config.permission_mode = TerminalPermissionMode::RequestApproval;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        std::fs::write(&target, "hello").unwrap();

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: MultiEditFileArgs {
                    operations: vec![MultiEditFileOperation::Edit {
                        path: target.display().to_string(),
                        edits: vec![MultiEditFileEdit {
                            old_string: "hello".to_string(),
                            new_string: "world".to_string(),
                            replace_all: false,
                            start_line: None,
                        }],
                    }],
                    cwd: Some(workspace.path().display().to_string()),
                    sandbox_permissions: SandboxPermissions::RequireEscalated,
                    justification: Some("edit project root outside data workspace".to_string()),
                    ..Default::default()
                },
                approved: true,
            },
            &config,
            workspace.path(),
        )
        .await;

        assert!(matches!(result, ToolResult::Completed { .. }));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "world");
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
        assert!(err.message.contains("matches"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "x\nx\n");
    }

    #[test]
    fn legacy_path_and_files_forms_normalize_to_operations() {
        let args: MultiEditFileArgs = serde_json::from_value(serde_json::json!({
            "path": "a.txt",
            "edits": [{"old_string": "x", "new_string": "y"}],
            "files": [{"path": "b.txt", "edits": [{"old_string": "1", "new_string": "2"}]}],
            "operations": [{"type": "delete", "path": "c.txt"}]
        }))
        .expect("legacy argument forms should deserialize");

        let operations = normalize_multi_edit_operations(&args).expect("normalize");
        assert_eq!(operations.len(), 3);
        assert!(
            matches!(&operations[0], MultiEditFileOperation::Edit { path, .. } if path == "a.txt")
        );
        assert!(
            matches!(&operations[1], MultiEditFileOperation::Edit { path, .. } if path == "b.txt")
        );
        assert!(
            matches!(&operations[2], MultiEditFileOperation::Delete { path } if path == "c.txt")
        );
    }

    #[test]
    fn top_level_edits_without_path_are_rejected() {
        let args: MultiEditFileArgs = serde_json::from_value(serde_json::json!({
            "edits": [{"old_string": "x", "new_string": "y"}]
        }))
        .expect("top-level edits should deserialize");
        let err = normalize_multi_edit_operations(&args).unwrap_err();
        assert!(err.contains("path is required"));
    }

    #[test]
    fn rejects_old_string_from_previous_new_string() {
        let err = apply_edits_to_lf(
            "alpha\n",
            &[
                MultiEditFileEdit {
                    old_string: "alpha".to_string(),
                    new_string: "beta gamma".to_string(),
                    replace_all: false,
                    start_line: None,
                },
                MultiEditFileEdit {
                    old_string: "gamma".to_string(),
                    new_string: "delta".to_string(),
                    replace_all: false,
                    start_line: None,
                },
            ],
            "a.txt",
            1,
        )
        .unwrap_err();
        assert!(err.contains("substring of a previous new_string"));
    }

    #[test]
    fn line_endings_follow_majority_style() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let edit_two = || {
            vec![MultiEditFileEdit {
                old_string: "two".to_string(),
                new_string: "TWO".to_string(),
                replace_all: false,
                start_line: None,
            }]
        };

        // One stray CRLF in an LF-majority file must not flip it to CRLF.
        std::fs::write(&path, "one\ntwo\r\nthree\nfour\n").unwrap();
        apply_multi_edit_file(
            dir.path(),
            &[MultiEditFileOperation::Edit {
                path: "a.txt".to_string(),
                edits: edit_two(),
            }],
        )
        .expect("edit should apply");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "one\nTWO\nthree\nfour\n"
        );

        // A CRLF-majority file keeps CRLF endings.
        std::fs::write(&path, "one\r\ntwo\r\nthree\n").unwrap();
        apply_multi_edit_file(
            dir.path(),
            &[MultiEditFileOperation::Edit {
                path: "a.txt".to_string(),
                edits: edit_two(),
            }],
        )
        .expect("edit should apply");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "one\r\nTWO\r\nthree\r\n"
        );
    }

    #[test]
    fn start_line_zero_is_tolerated_and_block_end_hint_resolves() {
        // Two identical two-line blocks starting at lines 1 and 9.
        let content = "alpha\nbeta\n1\n2\n3\n4\n5\n6\nalpha\nbeta\n";
        let edits = |start_line: Option<usize>| {
            vec![MultiEditFileEdit {
                old_string: "alpha\nbeta".to_string(),
                new_string: "ALPHA\nBETA".to_string(),
                replace_all: false,
                start_line,
            }]
        };

        // A hint on the block's last line still selects that block.
        let (updated, replacements) =
            apply_edits_to_lf(content, &edits(Some(10)), "a.txt", 1).unwrap();
        assert_eq!(replacements, 1);
        assert!(updated.starts_with("alpha\nbeta\n"));
        assert!(updated.ends_with("ALPHA\nBETA\n"));

        // A zero hint counts as the top of the file instead of erroring.
        let (updated, _) = apply_edits_to_lf(content, &edits(Some(0)), "a.txt", 1).unwrap();
        assert!(updated.starts_with("ALPHA\nBETA\n"));
    }

    #[test]
    fn warns_on_escaped_template_dollar_and_scanner_ignores_strings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.ts"), "const x = `v`;\n").unwrap();
        let outcome = apply_multi_edit_file(
            dir.path(),
            &[MultiEditFileOperation::Edit {
                path: "a.ts".to_string(),
                edits: vec![MultiEditFileEdit {
                    old_string: "`v`".to_string(),
                    new_string: "`v \\${name}`".to_string(),
                    replace_all: false,
                    start_line: None,
                }],
            }],
        )
        .expect("edit should apply");
        assert!(outcome.warnings.iter().any(|w| w.contains("\\${")));

        // A parenthesis inside a string literal must not trigger the scanner.
        std::fs::write(dir.path().join("b.rs"), "fn main() { let _s = \"x\"; }\n").unwrap();
        let outcome = apply_multi_edit_file(
            dir.path(),
            &[MultiEditFileOperation::Edit {
                path: "b.rs".to_string(),
                edits: vec![MultiEditFileEdit {
                    old_string: "\"x\"".to_string(),
                    new_string: "\"(\"".to_string(),
                    replace_all: false,
                    start_line: None,
                }],
            }],
        )
        .expect("edit should apply");
        assert!(
            outcome.warnings.is_empty(),
            "unexpected warnings: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn warns_on_unbalanced_brackets_from_edit_and_create() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        let outcome = apply_multi_edit_file(
            dir.path(),
            &[
                MultiEditFileOperation::Edit {
                    path: "a.rs".to_string(),
                    edits: vec![MultiEditFileEdit {
                        old_string: "fn main() {}".to_string(),
                        new_string: "fn main() {".to_string(),
                        replace_all: false,
                        start_line: None,
                    }],
                },
                MultiEditFileOperation::Create {
                    path: "b.rs".to_string(),
                    content: "fn broken( {}\n".to_string(),
                    overwrite: false,
                },
            ],
        )
        .expect("operations should apply");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("a.rs") && w.contains("unclosed"))
        );
        assert!(outcome.warnings.iter().any(|w| w.contains("b.rs")));
    }

    #[test]
    fn expand_home_path_resolves_tilde_prefix() {
        let home = workspace::home_dir().expect("home directory available in test environment");
        let expanded = expand_home_path("~/wei-agent-test.txt");
        assert!(Path::new(&expanded).starts_with(&home));
        assert_eq!(expand_home_path("plain.txt"), "plain.txt");
    }

    #[tokio::test]
    async fn file_locks_serialize_writes_to_the_same_path() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let target = PathBuf::from("wei-agent-lock-test/shared.txt");
        let gate = Arc::new(tokio::sync::Barrier::new(2));
        let in_critical = Arc::new(AtomicBool::new(false));
        let saw_overlap = Arc::new(AtomicBool::new(false));

        let run = || {
            let target = target.clone();
            let gate = gate.clone();
            let in_critical = in_critical.clone();
            let saw_overlap = saw_overlap.clone();
            async move {
                gate.wait().await;
                let _guards = acquire_file_locks(std::slice::from_ref(&target)).await;
                if in_critical.swap(true, Ordering::SeqCst) {
                    saw_overlap.store(true, Ordering::SeqCst);
                }
                // Hold the lock long enough that the peer would observe
                // overlap if serialization were broken.
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                in_critical.store(false, Ordering::SeqCst);
            }
        };

        let first = tokio::spawn(run());
        let second = tokio::spawn(run());
        first.await.expect("first task");
        second.await.expect("second task");
        assert!(!saw_overlap.load(Ordering::SeqCst));
    }
}
