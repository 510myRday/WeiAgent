use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

use chrono::Utc;
use futures_util::{StreamExt, stream};
use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs, sync::RwLock};
use uuid::Uuid;

use crate::{
    domain::{
        SkillChangeEvent, SkillChangeFeed, SkillConflict, SkillContent, SkillDiagnostic,
        SkillGroupState, SkillImportTask, SkillManifest, SkillOverrides, SkillSettings, ToolResult,
    },
    error::{AppError, AppResult},
    fsutil,
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::Store,
};

use super::{
    parser::{COLLAPSED_FILE_KIND, discover_display_files, is_collapsed_directory, parse_skill},
    path_policy::relative_file,
};

use crate::storage::{BUILTIN_SKILL_GROUP_ID, BUILTIN_WEIAGENT_CLI_SKILL_NAME};

const BUILTIN_WEIAGENT_CLI_SKILL: &str = include_str!("builtin/weiagentcli/SKILL.md");
const BUILTIN_WEIAGENT_CLI_FILES: &[(&str, &str)] = &[
    (
        "references/agents.md",
        include_str!("builtin/weiagentcli/references/agents.md"),
    ),
    (
        "references/providers-models.md",
        include_str!("builtin/weiagentcli/references/providers-models.md"),
    ),
    (
        "references/skills.md",
        include_str!("builtin/weiagentcli/references/skills.md"),
    ),
    (
        "references/mcp.md",
        include_str!("builtin/weiagentcli/references/mcp.md"),
    ),
    (
        "references/proxies.md",
        include_str!("builtin/weiagentcli/references/proxies.md"),
    ),
    (
        "references/cli-reference.md",
        include_str!("builtin/weiagentcli/references/cli-reference.md"),
    ),
    (
        "references/scheduling.md",
        include_str!("builtin/weiagentcli/references/scheduling.md"),
    ),
];
const LEGACY_BUILTIN_SKILL_NAME: &str = "client";

#[cfg(windows)]
fn display_path(path: &Path) -> String {
    fsutil::display_path(path)
}

#[cfg(not(windows))]
fn display_path(path: &Path) -> String {
    fsutil::display_path(path)
}

pub const SKILL_VIEW_TOOL_NAME: &str = "skill_view";

const MAX_REMOTE_SKILL_BYTES: u64 = 1024 * 1024 * 1024;
const REMOTE_DOWNLOAD_CONCURRENCY: usize = 8;
const REMOTE_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(8);
const REMOTE_REQUEST_RETRIES: usize = 3;
const REMOTE_RETRY_BACKOFF: Duration = Duration::from_millis(300);
const DEFAULT_SKILL_VIEW_MAX_CHARS: usize = 12_000;
const MAX_SKILL_VIEW_MAX_CHARS: usize = 12_000;
const SKILL_RESOURCE_DIRECTORIES: [&str; 5] =
    ["references", "scripts", "examples", "templates", "assets"];
const SKILL_RELATIONS_FILE: &str = "skill-relations.json";

#[derive(Default)]
struct Catalog {
    manifests: Vec<SkillManifest>,
    diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshotEntry {
    path: String,
    kind: u8,
    size: u64,
    modified_ns: u128,
}

fn changed_snapshot_paths(
    before: &[FileSnapshotEntry],
    after: &[FileSnapshotEntry],
) -> Vec<String> {
    let before_by_path = before
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let after_by_path = after
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<HashMap<_, _>>();
    before_by_path
        .keys()
        .chain(after_by_path.keys())
        .filter_map(|path| {
            let before_entry = before_by_path.get(path);
            let after_entry = after_by_path.get(path);
            (before_entry != after_entry).then(|| (*path).to_string())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

fn skill_catalog_changed(before: &[SkillManifest], after: &[SkillManifest]) -> bool {
    let signature = |manifest: &SkillManifest| {
        (
            manifest.name.clone(),
            manifest.description.clone(),
            manifest.version.clone(),
            manifest.group.clone(),
            manifest.tags.clone(),
            (
                manifest.root_dir.clone(),
                manifest.skill_md.clone(),
                manifest.content_hash.clone(),
                manifest.created_at.clone(),
                manifest.updated_at.clone(),
                manifest.scripts.clone(),
                manifest.references.clone(),
                manifest.examples.clone(),
            ),
        )
    };
    before.iter().map(signature).collect::<Vec<_>>()
        != after.iter().map(signature).collect::<Vec<_>>()
}

fn changed_skill_names(
    root: &Path,
    before: &[SkillManifest],
    after: &[SkillManifest],
    changed_paths: &[String],
) -> Vec<String> {
    let mut names = HashSet::new();
    for manifest in before.iter().chain(after.iter()) {
        let skill_root = PathBuf::from(&manifest.root_dir);
        let relative_root = skill_root
            .strip_prefix(root)
            .unwrap_or(&skill_root)
            .to_string_lossy()
            .replace('\\', "/");
        if changed_paths
            .iter()
            .any(|path| path == &relative_root || path.starts_with(&format!("{relative_root}/")))
        {
            names.insert(manifest.name.clone());
        }
    }
    for manifest in before {
        if !after
            .iter()
            .any(|item| item.name == manifest.name && item.root_dir == manifest.root_dir)
        {
            names.insert(manifest.name.clone());
        }
    }
    for manifest in after {
        if !before
            .iter()
            .any(|item| item.name == manifest.name && item.root_dir == manifest.root_dir)
        {
            names.insert(manifest.name.clone());
        }
    }
    let mut result = names.into_iter().collect::<Vec<_>>();
    result.sort();
    result
}

pub struct SkillManager {
    store: Arc<Store>,
    root: PathBuf,
    catalog: RwLock<Catalog>,
    import_tasks: RwLock<HashMap<Uuid, SkillImportTask>>,
    file_snapshot: RwLock<Vec<FileSnapshotEntry>>,
    change_events: RwLock<VecDeque<SkillChangeEvent>>,
    change_version: AtomicU64,
}

impl SkillManager {
    pub async fn new(store: Arc<Store>) -> AppResult<Self> {
        let root = store.data_dir.join("skills");
        fs::create_dir_all(&root).await?;
        let root = fs::canonicalize(root).await?;
        store.ensure_builtin_skill_group().await?;
        Self::migrate_legacy_builtin_skill(&root).await?;
        Self::materialize_builtin_skill(&root).await?;
        store
            .update_skill_source(BUILTIN_WEIAGENT_CLI_SKILL_NAME, None)
            .await?;
        store
            .update_skill_global_override(BUILTIN_WEIAGENT_CLI_SKILL_NAME, None)
            .await?;
        let manager = Self {
            store: store.clone(),
            root,
            catalog: RwLock::new(Catalog::default()),
            import_tasks: RwLock::new(HashMap::new()),
            file_snapshot: RwLock::new(Vec::new()),
            change_events: RwLock::new(VecDeque::new()),
            change_version: AtomicU64::new(0),
        };
        manager.refresh().await?;
        store
            .set_skill_group(
                BUILTIN_WEIAGENT_CLI_SKILL_NAME,
                Some(BUILTIN_SKILL_GROUP_ID),
            )
            .await?;
        *manager.file_snapshot.write().await = manager.capture_file_snapshot().await?;
        Ok(manager)
    }

    async fn materialize_builtin_skill(root: &Path) -> AppResult<()> {
        let builtin_root = root.join(BUILTIN_WEIAGENT_CLI_SKILL_NAME);
        if fs::try_exists(&builtin_root).await? {
            fs::remove_dir_all(&builtin_root).await?;
        }
        fs::create_dir_all(&builtin_root).await?;
        crate::fsutil::write_atomic(&builtin_root.join("SKILL.md"), BUILTIN_WEIAGENT_CLI_SKILL)
            .await?;
        for (relative_path, content) in BUILTIN_WEIAGENT_CLI_FILES {
            let path = builtin_root.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            crate::fsutil::write_atomic(&path, content).await?;
        }
        Ok(())
    }

    async fn migrate_legacy_builtin_skill(root: &Path) -> AppResult<()> {
        let legacy_root = root.join(LEGACY_BUILTIN_SKILL_NAME);
        let builtin_root = root.join(BUILTIN_WEIAGENT_CLI_SKILL_NAME);
        if fs::try_exists(&builtin_root).await? || !fs::try_exists(&legacy_root).await? {
            return Ok(());
        }
        let legacy_file = legacy_root.join("SKILL.md");
        let Ok(content) = fs::read_to_string(&legacy_file).await else {
            return Ok(());
        };
        if !content.contains("name: client") || !content.contains("group: built-in") {
            return Ok(());
        }
        fs::rename(&legacy_root, &builtin_root).await?;
        let migrated = content.replacen("name: client", "name: weiagentcli", 1);
        if migrated != content {
            fs::write(builtin_root.join("SKILL.md"), migrated).await?;
        }
        Ok(())
    }

    pub fn start_monitor(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.monitor_loop().await;
        })
    }

    async fn monitor_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let Ok(next_snapshot) = self.capture_file_snapshot().await else {
                continue;
            };
            let changed_paths = {
                let mut current = self.file_snapshot.write().await;
                if *current == next_snapshot {
                    Vec::new()
                } else {
                    let changed = changed_snapshot_paths(&current, &next_snapshot);
                    *current = next_snapshot;
                    changed
                }
            };
            if changed_paths.is_empty() {
                continue;
            }
            let before = self.catalog.read().await.manifests.clone();
            if self.refresh().await.is_err() {
                continue;
            }
            let after = self.catalog.read().await.manifests.clone();
            let changed_skills = changed_skill_names(&self.root, &before, &after, &changed_paths);
            let list_changed = skill_catalog_changed(&before, &after);
            self.record_change(list_changed, changed_skills).await;
        }
    }

    async fn capture_file_snapshot(&self) -> AppResult<Vec<FileSnapshotEntry>> {
        let mut entries = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(current) = stack.pop() {
            let mut children = match fs::read_dir(&current).await {
                Ok(children) => children,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = children.next_entry().await? {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).await?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let modified_ns = metadata
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                if metadata.is_dir() {
                    entries.push(FileSnapshotEntry {
                        path: relative,
                        kind: 0,
                        size: 0,
                        modified_ns,
                    });
                    if !is_collapsed_directory(&entry.file_name()) {
                        stack.push(path);
                    }
                } else if metadata.is_file() {
                    entries.push(FileSnapshotEntry {
                        path: relative,
                        kind: 1,
                        size: metadata.len(),
                        modified_ns,
                    });
                }
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    async fn record_change(&self, list_changed: bool, changed_skills: Vec<String>) {
        let version = self.change_version.fetch_add(1, Ordering::Relaxed) + 1;
        let event = SkillChangeEvent {
            version,
            list_changed,
            changed_skills,
        };
        let mut events = self.change_events.write().await;
        events.push_back(event);
        while events.len() > 64 {
            events.pop_front();
        }
    }

    pub async fn changes_since(&self, since: u64) -> SkillChangeFeed {
        let version = self.change_version.load(Ordering::Relaxed);
        let events = self.change_events.read().await;
        let full_refresh = events
            .front()
            .is_some_and(|event| since + 1 < event.version);
        SkillChangeFeed {
            version,
            events: if full_refresh {
                events.iter().cloned().collect()
            } else {
                events
                    .iter()
                    .filter(|event| event.version > since)
                    .cloned()
                    .collect()
            },
            full_refresh,
        }
    }

    pub async fn refresh(&self) -> AppResult<()> {
        let mut candidates = Vec::new();
        let mut entries = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            let skill_md_metadata = match fs::symlink_metadata(&skill_md).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if skill_md_metadata.file_type().is_symlink() || !skill_md_metadata.is_file() {
                continue;
            }
            candidates.push(path);
        }
        candidates.sort();
        candidates.dedup();
        let mut manifests = Vec::new();
        let mut diagnostics = Vec::new();
        for root in candidates {
            match parse_skill(root.clone()).await {
                Ok(manifest) => manifests.push(manifest),
                Err(error) => diagnostics.push(SkillDiagnostic {
                    path: display_path(&root),
                    message: error.to_string(),
                }),
            }
        }
        manifests.sort_by(|a, b| a.name.cmp(&b.name).then(a.root_dir.cmp(&b.root_dir)));
        let mut duplicate_names = HashSet::new();
        for pair in manifests.windows(2) {
            if pair[0].name == pair[1].name {
                duplicate_names.insert(pair[0].name.clone());
            }
        }
        for name in duplicate_names {
            diagnostics.push(SkillDiagnostic {
                path: name.clone(),
                message: format!("duplicate skill name: {name}"),
            });
        }
        let mut catalog = self.catalog.write().await;
        catalog.manifests = manifests;
        catalog.diagnostics = diagnostics;
        Ok(())
    }

    pub async fn create_import_task_with_context(
        &self,
        source: &str,
        kind: &str,
        group_id: Option<String>,
        proxy_url: Option<String>,
        expected_name: Option<String>,
    ) -> AppResult<SkillImportTask> {
        let source = source.trim();
        if source.is_empty() {
            return Err(AppError::BadRequest(
                "Skill path or URL is required".to_string(),
            ));
        }
        if looks_like_url(source) {
            normalize_remote_source(source)?;
        } else {
            validate_local_skill_directory(source).await?;
        }

        let now = Utc::now().to_rfc3339();
        let task = SkillImportTask {
            id: Uuid::new_v4(),
            source: source.to_string(),
            kind: kind.to_string(),
            group_id,
            status: "queued".to_string(),
            phase: "preparing".to_string(),
            progress: 0,
            current_file: None,
            downloaded_files: 0,
            total_files: 0,
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
            finished_at: None,
            proxy_url,
            expected_name,
        };
        let mut tasks = self.import_tasks.write().await;
        tasks.insert(task.id, task.clone());
        if tasks.len() > 20 {
            let mut ids = tasks.values().cloned().collect::<Vec<_>>();
            ids.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            for item in ids.into_iter().take(tasks.len().saturating_sub(20)) {
                tasks.remove(&item.id);
            }
        }
        Ok(task)
    }

    #[cfg(test)]
    async fn create_import_task(
        &self,
        source: &str,
        kind: &str,
        group_id: Option<String>,
    ) -> AppResult<SkillImportTask> {
        self.create_import_task_with_context(source, kind, group_id, None, None)
            .await
    }

    pub async fn list_import_tasks(&self) -> Vec<SkillImportTask> {
        let tasks = self.import_tasks.read().await;
        let mut result = tasks.values().cloned().collect::<Vec<_>>();
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        result
    }

    pub async fn start_update_task(self: &Arc<Self>, name: &str) -> AppResult<SkillImportTask> {
        let name = name.trim().to_ascii_lowercase();
        if name == BUILTIN_WEIAGENT_CLI_SKILL_NAME {
            return Err(AppError::BadRequest(
                "the built-in weiagentcli Skill cannot be updated".to_string(),
            ));
        }
        let settings = self.settings(None).await?;
        let manifest = settings
            .defaults
            .into_iter()
            .find(|item| item.name == name)
            .ok_or(AppError::NotFound)?;
        let source = manifest
            .source_url
            .clone()
            .ok_or_else(|| AppError::BadRequest("Skill source is not recorded".to_string()))?;
        let kind = if looks_like_url(&source) {
            "network"
        } else {
            "local"
        };
        let task = self
            .create_import_task_with_context(
                &source,
                kind,
                manifest.group_id.clone(),
                if kind == "network" {
                    manifest.update_proxy_url.clone()
                } else {
                    None
                },
                Some(name.clone()),
            )
            .await?;
        let manager = self.clone();
        let proxy_url = task.proxy_url.clone();
        let group_id = manifest.group_id.clone();
        tokio::spawn(async move {
            manager
                .run_update_task(task.id, name, source, proxy_url, group_id)
                .await;
        });
        Ok(task)
    }

    pub async fn run_import_task(
        &self,
        task_id: Uuid,
        source: String,
        proxy_url: Option<String>,
        group_id: Option<String>,
    ) {
        self.run_import_task_with_expected(task_id, source, proxy_url, group_id, None)
            .await;
    }

    async fn run_import_task_with_expected(
        &self,
        task_id: Uuid,
        source: String,
        proxy_url: Option<String>,
        group_id: Option<String>,
        expected_name: Option<String>,
    ) {
        self.update_import_task(task_id, "running", "preparing", 5, None, false)
            .await;
        let result = if looks_like_url(&source) {
            self.import_source_with_progress(
                &source,
                proxy_url.as_deref(),
                Some(task_id),
                group_id.as_deref(),
                expected_name.as_deref(),
            )
            .await
        } else {
            self.import_directory_with_progress(
                &source,
                Some(task_id),
                group_id.as_deref(),
                expected_name.as_deref(),
            )
            .await
        };
        match result {
            Ok(()) => {
                self.update_import_task(task_id, "completed", "completed", 100, None, true)
                    .await;
            }
            Err(error) => {
                self.update_import_task(
                    task_id,
                    "failed",
                    "failed",
                    100,
                    Some(error.to_string()),
                    true,
                )
                .await;
            }
        }
    }

    pub async fn retry_import_task(self: &Arc<Self>, task_id: Uuid) -> AppResult<SkillImportTask> {
        let task = {
            let tasks = self.import_tasks.read().await;
            tasks.get(&task_id).cloned().ok_or(AppError::NotFound)?
        };
        if task.status != "failed" {
            return Err(AppError::BadRequest(
                "only failed Skill downloads can be retried".to_string(),
            ));
        }
        {
            let mut tasks = self.import_tasks.write().await;
            let current = tasks.get_mut(&task_id).ok_or(AppError::NotFound)?;
            current.status = "queued".to_string();
            current.phase = "preparing".to_string();
            current.progress = 0;
            current.current_file = None;
            current.downloaded_files = 0;
            current.total_files = 0;
            current.downloaded_bytes = 0;
            current.total_bytes = None;
            current.error = None;
            current.updated_at = Utc::now().to_rfc3339();
            current.finished_at = None;
        }
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .run_import_task_with_expected(
                    task_id,
                    task.source,
                    task.proxy_url,
                    task.group_id,
                    task.expected_name,
                )
                .await;
        });
        let tasks = self.import_tasks.read().await;
        tasks.get(&task_id).cloned().ok_or(AppError::NotFound)
    }

    pub async fn run_update_task(
        &self,
        task_id: Uuid,
        name: String,
        source: String,
        proxy_url: Option<String>,
        group_id: Option<String>,
    ) {
        self.update_import_task(task_id, "running", "preparing", 5, None, false)
            .await;
        let result = if looks_like_url(&source) {
            self.import_source_with_progress(
                &source,
                proxy_url.as_deref(),
                Some(task_id),
                group_id.as_deref(),
                Some(&name),
            )
            .await
        } else {
            self.import_directory_with_progress(
                &source,
                Some(task_id),
                group_id.as_deref(),
                Some(&name),
            )
            .await
        };
        match result {
            Ok(()) => {
                self.update_import_task(task_id, "completed", "completed", 100, None, true)
                    .await;
            }
            Err(error) => {
                self.update_import_task(
                    task_id,
                    "failed",
                    "failed",
                    100,
                    Some(error.to_string()),
                    true,
                )
                .await;
            }
        }
    }

    async fn update_import_task(
        &self,
        task_id: Uuid,
        status: &str,
        phase: &str,
        progress: u8,
        error: Option<String>,
        finished: bool,
    ) {
        let mut tasks = self.import_tasks.write().await;
        let Some(task) = tasks.get_mut(&task_id) else {
            return;
        };
        task.status = status.to_string();
        task.phase = phase.to_string();
        task.progress = progress;
        task.error = error;
        task.updated_at = Utc::now().to_rfc3339();
        if finished {
            task.finished_at = Some(task.updated_at.clone());
        }
    }

    async fn update_import_download_progress(
        &self,
        task_id: Option<Uuid>,
        current_file: Option<String>,
        downloaded_files: usize,
        total_files: usize,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    ) {
        let Some(task_id) = task_id else {
            return;
        };
        let mut tasks = self.import_tasks.write().await;
        let Some(task) = tasks.get_mut(&task_id) else {
            return;
        };
        let progress = total_bytes
            .filter(|total| *total > 0)
            .map(|total| {
                downloaded_bytes
                    .saturating_mul(60)
                    .checked_div(total)
                    .unwrap_or(0) as u8
            })
            .unwrap_or_else(|| {
                if total_files == 0 {
                    0
                } else {
                    downloaded_files
                        .saturating_mul(60)
                        .checked_div(total_files)
                        .unwrap_or(0) as u8
                }
            })
            .min(60);
        task.phase = "downloading".to_string();
        task.progress = 20u8.saturating_add(progress);
        task.current_file = current_file;
        task.downloaded_files = downloaded_files;
        task.total_files = total_files;
        task.downloaded_bytes = downloaded_bytes;
        task.total_bytes = total_bytes;
        task.updated_at = Utc::now().to_rfc3339();
    }

    async fn import_directory_with_progress(
        &self,
        source_directory: &str,
        task_id: Option<Uuid>,
        group_id: Option<&str>,
        expected_name: Option<&str>,
    ) -> AppResult<()> {
        let source = source_directory.trim();
        if source.is_empty() {
            return Err(AppError::BadRequest(
                "Skill directory is required".to_string(),
            ));
        }

        let source = validate_local_skill_directory(source).await?;
        let source_display = display_path(&source);
        if let Some(task_id) = task_id {
            self.update_import_task(task_id, "running", "copying", 25, None, false)
                .await;
        }

        let temporary = self.import_temporary_path(task_id);
        if let Err(error) = copy_skill_directory(&source, &temporary).await {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(error);
        }
        let result = self
            .finish_import(
                temporary.clone(),
                None,
                Some(&source_display),
                group_id,
                expected_name,
            )
            .await;
        if result.is_err() && task_id.is_none() {
            let _ = fs::remove_dir_all(&temporary).await;
        }
        result
    }

    async fn import_source_with_progress(
        &self,
        source: &str,
        proxy_url: Option<&str>,
        task_id: Option<Uuid>,
        group_id: Option<&str>,
        expected_name: Option<&str>,
    ) -> AppResult<()> {
        let source = source.trim();
        if source.is_empty() {
            return Err(AppError::BadRequest(
                "Skill path or URL is required".to_string(),
            ));
        }
        if !looks_like_url(source) {
            return self
                .import_directory_with_progress(source, task_id, group_id, expected_name)
                .await;
        }

        let parsed = normalize_remote_source(source)?;
        let cdn = github_jsdelivr_source(&parsed)?;
        if let Some(task_id) = task_id {
            self.update_import_task(task_id, "running", "connecting", 20, None, false)
                .await;
        }
        let selected = select_remote_route(parsed, cdn, proxy_url).await?;
        let temporary = self.import_temporary_path(task_id);
        let skill_temporary = temporary.join("skill");
        let sibling_temporary = temporary.join("siblings");
        fs::create_dir_all(&skill_temporary).await?;

        let result = async {
            let skill_md = selected.skill_md;
            validate_skill_markdown_text(&skill_md)?;
            if let Some(task_id) = task_id {
                self.update_import_task(task_id, "running", "downloading", 45, None, false)
                    .await;
            }
            write_skill_download(&skill_temporary.join("SKILL.md"), skill_md.as_bytes()).await?;

            let mut downloaded = 1usize;
            let mut downloaded_bytes = skill_md.len() as u64;
            let referenced = referenced_skill_files(&skill_md);

            if let Some(github) = selected
                .routes
                .first()
                .and_then(|route| route.source.github.as_ref())
            {
                if let Some(tree) = fetch_github_tree_with_fallback(&selected.routes, github).await
                {
                    let files =
                        select_github_skill_files(&tree, &github.skill_directory, &referenced)
                            .into_iter()
                            .filter(|file| {
                                !file.relative_path.eq_ignore_ascii_case("SKILL.md")
                                    && valid_download_path(&file.relative_path)
                            })
                            .collect::<Vec<_>>();
                    let total_files = files.len() + 1;
                    let total_bytes = files
                        .iter()
                        .map(|file| file.size)
                        .collect::<Option<Vec<_>>>()
                        .map(|sizes| {
                            sizes
                                .into_iter()
                                .fold(skill_md.len() as u64, |total, size| {
                                    total.saturating_add(size)
                                })
                        });
                    if total_bytes.is_some_and(|total| total > MAX_REMOTE_SKILL_BYTES) {
                        return Err(AppError::BadRequest(
                            "remote Skill exceeds the 1 GiB download limit".to_string(),
                        ));
                    }
                    self.update_import_download_progress(
                        task_id,
                        files.first().map(|file| file.relative_path.clone()),
                        downloaded,
                        total_files,
                        downloaded_bytes,
                        total_bytes,
                    )
                    .await;
                    let mut pending_files = Vec::new();
                    for file in files {
                        let destination_root = if file.sibling {
                            &sibling_temporary
                        } else {
                            &skill_temporary
                        };
                        let destination = destination_root.join(&file.relative_path);
                        if is_cached_download(&destination, file.size).await {
                            downloaded += 1;
                            downloaded_bytes = downloaded_bytes.saturating_add(
                                fs::metadata(&destination)
                                    .await
                                    .map(|item| item.len())
                                    .unwrap_or(0),
                            );
                        } else {
                            pending_files.push(file);
                        }
                    }
                    self.update_import_download_progress(
                        task_id,
                        pending_files.first().map(|file| file.relative_path.clone()),
                        downloaded,
                        total_files,
                        downloaded_bytes,
                        total_bytes,
                    )
                    .await;
                    let routes = Arc::new(selected.routes.clone());
                    let github = github.clone();
                    let downloads = stream::iter(pending_files.into_iter().map(|file| {
                        let routes = routes.clone();
                        let github = github.clone();
                        async move {
                            let bytes = download_bytes_with_fallback(
                                routes.as_ref(),
                                &github,
                                &file.remote_path,
                            )
                            .await?;
                            Ok::<_, AppError>((file, bytes))
                        }
                    }))
                    .buffer_unordered(REMOTE_DOWNLOAD_CONCURRENCY);
                    tokio::pin!(downloads);
                    while let Some(result) = downloads.next().await {
                        let (file, bytes) = result?;
                        downloaded_bytes = downloaded_bytes.saturating_add(bytes.len() as u64);
                        if downloaded_bytes > MAX_REMOTE_SKILL_BYTES {
                            return Err(AppError::BadRequest(
                                "remote Skill exceeds the 1 GiB download limit".to_string(),
                            ));
                        }
                        let destination_root = if file.sibling {
                            &sibling_temporary
                        } else {
                            &skill_temporary
                        };
                        let destination = destination_root.join(&file.relative_path);
                        if let Some(parent) = destination.parent() {
                            fs::create_dir_all(parent).await?;
                        }
                        write_skill_download(&destination, &bytes).await?;
                        downloaded += 1;
                        self.update_import_download_progress(
                            task_id,
                            Some(file.relative_path),
                            downloaded,
                            total_files,
                            downloaded_bytes,
                            total_bytes,
                        )
                        .await;
                    }
                } else {
                    download_referenced_files(
                        self,
                        &selected.routes,
                        &skill_temporary,
                        task_id,
                        &referenced,
                        referenced.iter().filter(|item| !item.sibling).count() + 1,
                        &mut downloaded,
                        &mut downloaded_bytes,
                    )
                    .await?;
                }
            } else {
                download_referenced_files(
                    self,
                    &selected.routes,
                    &skill_temporary,
                    task_id,
                    &referenced,
                    referenced.iter().filter(|item| !item.sibling).count() + 1,
                    &mut downloaded,
                    &mut downloaded_bytes,
                )
                .await?;
            }

            if let Some(task_id) = task_id {
                self.update_import_task(task_id, "running", "installing", 85, None, false)
                    .await;
            }
            self.finish_import(
                skill_temporary.clone(),
                Some(&sibling_temporary),
                Some(source),
                group_id,
                expected_name,
            )
            .await
        }
        .await;

        if result.is_err() && task_id.is_none() {
            let _ = fs::remove_dir_all(&temporary).await;
        }
        result
    }

    fn import_temporary_path(&self, task_id: Option<Uuid>) -> PathBuf {
        self.root.join(match task_id {
            Some(task_id) => format!(".import-{task_id}"),
            None => format!(".import-{}", Uuid::new_v4().simple()),
        })
    }

    async fn finish_import(
        &self,
        temporary: PathBuf,
        sibling_temporary: Option<&Path>,
        source_url: Option<&str>,
        group_id: Option<&str>,
        expected_name: Option<&str>,
    ) -> AppResult<()> {
        let manifest = parse_skill(temporary.clone()).await?;
        if expected_name.is_some_and(|name| manifest.name != name) {
            let expected = expected_name.unwrap_or_default();
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(AppError::BadRequest(format!(
                "updated Skill name changed from {expected} to {}",
                manifest.name
            )));
        }
        if manifest.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(AppError::BadRequest(
                "the built-in weiagentcli Skill cannot be replaced by an import".to_string(),
            ));
        }
        if group_id == Some(BUILTIN_SKILL_GROUP_ID) {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(AppError::BadRequest(
                "only the built-in weiagentcli Skill can belong to the built-in group".to_string(),
            ));
        }
        let destination = self.root.join(&manifest.name);
        if temporary == destination {
            self.refresh().await?;
            return Ok(());
        }
        if let Err(error) = async {
            if fs::try_exists(&destination).await? {
                fs::remove_dir_all(&destination).await?;
            }
            fs::rename(&temporary, &destination).await?;
            Ok::<(), std::io::Error>(())
        }
        .await
        {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(error.into());
        }
        let attached_skill_names = if let Some(sibling_temporary) = sibling_temporary {
            match Self::install_sibling_directories(sibling_temporary, &self.root).await {
                Ok(names) => names,
                Err(error) => {
                    let _ = fs::remove_dir_all(&destination).await;
                    return Err(error);
                }
            }
        } else {
            Vec::new()
        };
        if let Some(sibling_temporary) = sibling_temporary {
            if let Some(staging_root) = sibling_temporary.parent() {
                let _ = fs::remove_dir_all(staging_root).await;
            }
        }
        self.update_skill_relations(&manifest.name, &attached_skill_names)
            .await?;
        self.store
            .update_skill_source(&manifest.name, source_url.map(str::to_string))
            .await?;
        self.store.set_skill_group(&manifest.name, group_id).await?;
        self.refresh().await
    }

    async fn read_skill_relations(&self) -> AppResult<(BTreeMap<String, Vec<String>>, bool)> {
        let path = self.root.join(SKILL_RELATIONS_FILE);
        match fs::read_to_string(path).await {
            Ok(content) => Ok((serde_json::from_str(&content)?, true)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((BTreeMap::new(), false))
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn write_skill_relations(
        &self,
        relations: &BTreeMap<String, Vec<String>>,
    ) -> AppResult<()> {
        let path = self.root.join(SKILL_RELATIONS_FILE);
        if relations.is_empty() {
            match fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            return Ok(());
        }
        fsutil::write_atomic(&path, serde_json::to_string_pretty(relations)?).await
    }

    async fn update_skill_relations(&self, source: &str, attached: &[String]) -> AppResult<()> {
        let (mut relations, _) = self.read_skill_relations().await?;
        for names in relations.values_mut() {
            names.retain(|name| !attached.iter().any(|attached_name| attached_name == name));
        }
        relations.retain(|_, names| {
            names.sort();
            names.dedup();
            !names.is_empty()
        });
        relations.remove(source);
        let mut attached = attached.to_vec();
        attached.sort();
        attached.dedup();
        if !attached.is_empty() {
            relations.insert(source.to_string(), attached);
        }
        self.write_skill_relations(&relations).await
    }

    async fn sync_skill_relations(
        &self,
        manifests: &[SkillManifest],
    ) -> AppResult<HashMap<String, String>> {
        let (mut relations, existed) = self.read_skill_relations().await?;
        let known = manifests
            .iter()
            .map(|manifest| manifest.name.as_str())
            .collect::<HashSet<_>>();
        for names in relations.values_mut() {
            names.retain(|name| known.contains(name.as_str()));
            names.sort();
            names.dedup();
        }
        relations.retain(|source, names| known.contains(source.as_str()) && !names.is_empty());
        if existed || !relations.is_empty() {
            self.write_skill_relations(&relations).await?;
        }
        let mut source_by_skill = HashMap::new();
        for (source, names) in relations {
            for name in names {
                source_by_skill
                    .entry(name)
                    .or_insert_with(|| source.clone());
            }
        }
        Ok(source_by_skill)
    }

    async fn install_sibling_directories(staging: &Path, root: &Path) -> AppResult<Vec<String>> {
        if !fs::try_exists(staging).await? {
            return Ok(Vec::new());
        }
        let mut attached_names = Vec::new();
        let mut entries = fs::read_dir(staging).await?;
        while let Some(entry) = entries.next_entry().await? {
            let source = entry.path();
            let metadata = fs::symlink_metadata(&source).await?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AppError::BadRequest(
                    "downloaded sibling resources must be directories".to_string(),
                ));
            }
            if let Ok(manifest) = parse_skill(source.clone()).await {
                attached_names.push(manifest.name);
            }
            let destination = root.join(entry.file_name());
            if fs::try_exists(&destination).await? {
                fs::remove_dir_all(&destination).await?;
            }
            fs::rename(source, destination).await?;
        }
        Ok(attached_names)
    }

    pub async fn settings(&self, agent_id: Option<Uuid>) -> AppResult<SkillSettings> {
        let mut global = self.store.read_skill_global_overrides().await?;
        let icons = self.store.read_skill_icons().await?;
        let sources = self.store.read_skill_sources().await?;
        let proxies = self.store.read_skill_proxies().await?;
        let group_records = self.store.list_skill_groups().await?;
        let catalog = self.catalog.read().await;
        let duplicate_names = duplicate_names(&catalog.manifests);
        let mut conflicts = duplicate_names
            .iter()
            .filter_map(|name| {
                let mut paths = catalog
                    .manifests
                    .iter()
                    .filter(|manifest| &manifest.name == name)
                    .map(|manifest| Self::display_path(Path::new(&manifest.root_dir)))
                    .collect::<Vec<_>>();
                paths.sort();
                (!paths.is_empty()).then(|| SkillConflict {
                    name: name.clone(),
                    count: paths.len(),
                    paths,
                })
            })
            .collect::<Vec<_>>();
        conflicts.sort_by(|a, b| a.name.cmp(&b.name));
        let catalog_manifests = catalog.manifests.clone();
        let catalog_diagnostics = catalog.diagnostics.clone();
        let known_skill_names = catalog
            .manifests
            .iter()
            .filter(|manifest| !duplicate_names.contains(&manifest.name))
            .map(|manifest| manifest.name.clone())
            .collect::<Vec<_>>();
        drop(catalog);
        self.store
            .ensure_skill_group_assignments(&known_skill_names)
            .await?;
        let source_by_skill = self.sync_skill_relations(&catalog_manifests).await?;
        let assignments = self.store.read_skill_group_assignments().await?;
        let assignments_by_skill = assignments
            .iter()
            .map(|item| (item.skill_name.as_str(), item))
            .collect::<HashMap<_, _>>();
        let groups_by_id = group_records
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect::<HashMap<_, _>>();
        let groups_by_name = group_records
            .iter()
            .map(|item| (item.name.to_ascii_lowercase(), item))
            .collect::<HashMap<_, _>>();
        if let Some(id) = agent_id {
            self.store
                .sync_skill_agent_catalog(id, &known_skill_names)
                .await?;
        }
        let mut agent = match agent_id {
            Some(id) => self.store.read_skill_agent_overrides(id).await?,
            None => SkillOverrides::default(),
        };
        global.groups = normalize_group_overrides(global.groups, &groups_by_name);
        agent.groups = normalize_group_overrides(agent.groups, &groups_by_name);
        let mut defaults = Vec::new();
        let mut effective = Vec::new();
        let groups = group_records
            .iter()
            .map(|group| {
                (
                    group.id.clone(),
                    SkillGroupState {
                        id: group.id.clone(),
                        name: group.name.clone(),
                        sort_order: group.sort_order,
                        skill_count: assignments
                            .iter()
                            .filter(|item| item.group_id.as_deref() == Some(group.id.as_str()))
                            .count(),
                        enabled: true,
                        global_override: None,
                        agent_override: None,
                        is_builtin: group.id == BUILTIN_SKILL_GROUP_ID,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for manifest in &catalog_manifests {
            if duplicate_names.contains(&manifest.name) {
                continue;
            }
            let global_item = find_skill(&global, &manifest.name);
            let assignment = assignments_by_skill.get(manifest.name.as_str()).copied();
            let assigned_group = assignment
                .and_then(|item| item.group_id.as_deref())
                .and_then(|id| groups_by_id.get(id).copied());
            let agent_item = agent_id.and_then(|_| find_skill(&agent, &manifest.name));
            let global_group_item = assigned_group.and_then(|group| find_group(&global, group));
            let agent_group_item =
                agent_id.and_then(|_| assigned_group.and_then(|group| find_group(&agent, group)));
            let is_builtin = manifest.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME;
            let enabled = agent_item
                .or(agent_group_item)
                .or(if is_builtin { None } else { global_item })
                .or(if is_builtin { None } else { global_group_item })
                .unwrap_or(true);
            let mut item = manifest.clone();
            item.group = assigned_group.map(|group| group.name.clone());
            item.group_id = assigned_group.map(|group| group.id.clone());
            item.is_builtin = is_builtin;
            // Attached Skills share the icon owned by their source Skill. Keep the
            // storage entries independent so an attached Skill can never shadow it.
            let icon_owner = source_by_skill
                .get(&manifest.name)
                .map(String::as_str)
                .unwrap_or(manifest.name.as_str());
            item.icon_url = icons.get(icon_owner).cloned();
            item.source_url = sources
                .get(&manifest.name)
                .cloned()
                .or_else(|| (!is_builtin).then(|| manifest.root_dir.clone()));
            item.source_skill = source_by_skill.get(&manifest.name).cloned();
            item.icon_proxy_url = proxies.get(icon_owner).and_then(|item| item.0.clone());
            item.update_proxy_url = proxies.get(&manifest.name).and_then(|item| item.1.clone());
            item.enabled = enabled;
            item.global_override = if is_builtin { None } else { global_item };
            item.group_override = if is_builtin { None } else { global_group_item };
            item.agent_override = agent_item;
            item.agent_group_override = agent_group_item;
            defaults.push(item.clone());
            if enabled {
                effective.push(item.clone());
            }
        }
        let known_names = defaults
            .iter()
            .map(|s| s.name.as_str())
            .collect::<HashSet<_>>();
        let orphan_skills = global
            .skills
            .iter()
            .chain(agent.skills.iter())
            .map(|item| item.name.clone())
            .filter(|name| !known_names.contains(name.as_str()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let orphan_groups = Vec::new();
        let mut diagnostics = catalog_diagnostics;
        for item in &defaults {
            diagnostics.extend(item.diagnostics.clone());
        }
        let overrides = if agent_id.is_some() { agent } else { global };
        let mut group_values = groups.into_values().collect::<Vec<_>>();
        group_values.sort_by_key(|group| group.sort_order);
        let group_order = group_values
            .iter()
            .map(|group| (group.id.as_str(), group.sort_order))
            .collect::<HashMap<_, _>>();
        let assignment_order = assignments
            .iter()
            .map(|item| (item.skill_name.as_str(), item.sort_order))
            .collect::<HashMap<_, _>>();
        let sort_skills = |items: &mut Vec<SkillManifest>| {
            items.sort_by(|a, b| {
                let a_group = a
                    .group_id
                    .as_deref()
                    .and_then(|id| group_order.get(id))
                    .copied()
                    .unwrap_or(i64::MAX);
                let b_group = b
                    .group_id
                    .as_deref()
                    .and_then(|id| group_order.get(id))
                    .copied()
                    .unwrap_or(i64::MAX);
                a_group
                    .cmp(&b_group)
                    .then_with(|| {
                        assignment_order
                            .get(a.name.as_str())
                            .copied()
                            .unwrap_or(i64::MAX)
                            .cmp(
                                &assignment_order
                                    .get(b.name.as_str())
                                    .copied()
                                    .unwrap_or(i64::MAX),
                            )
                    })
                    .then_with(|| a.name.cmp(&b.name))
            });
        };
        sort_skills(&mut defaults);
        sort_skills(&mut effective);
        Ok(SkillSettings {
            skills_dir: Self::display_path(&self.root),
            defaults,
            overrides,
            effective,
            groups: group_values,
            orphan_skills,
            orphan_groups,
            diagnostics,
            conflicts,
        })
    }

    pub async fn create_group(&self, name: &str) -> AppResult<()> {
        self.store.create_skill_group(name).await.map(|_| ())
    }

    pub async fn rename_group(&self, id: &str, name: &str) -> AppResult<()> {
        self.store.rename_skill_group(id, name).await
    }

    pub async fn delete_group(&self, id: &str) -> AppResult<()> {
        self.store.delete_skill_group(id).await
    }

    pub async fn reorder_groups(&self, ids: &[String]) -> AppResult<()> {
        self.store.reorder_skill_groups(ids).await
    }

    pub async fn assign_skill_group(
        &self,
        skill_name: &str,
        group_id: Option<&str>,
    ) -> AppResult<()> {
        self.store.set_skill_group(skill_name, group_id).await
    }

    pub async fn reorder_group_skills(
        &self,
        group_id: Option<&str>,
        skill_names: &[String],
    ) -> AppResult<()> {
        self.store
            .reorder_skill_group_items(group_id, skill_names)
            .await
    }

    fn display_path(path: &Path) -> String {
        fsutil::display_path(path)
    }

    pub async fn content_from_manifest(
        &self,
        manifest: &SkillManifest,
        file_path: Option<&str>,
    ) -> AppResult<SkillContent> {
        self.read_content(manifest.clone(), file_path).await
    }

    pub async fn inspect_content(
        &self,
        name: &str,
        file_path: Option<&str>,
    ) -> AppResult<SkillContent> {
        let name = name.trim().to_ascii_lowercase();
        let settings = self.settings(None).await?;
        let manifest = settings
            .defaults
            .into_iter()
            .find(|item| item.name == name)
            .ok_or_else(|| AppError::NotFound)?;
        self.read_content(manifest, file_path).await
    }

    pub async fn delete_skill(&self, name: &str) -> AppResult<()> {
        let name = name.trim().to_ascii_lowercase();
        if name == BUILTIN_WEIAGENT_CLI_SKILL_NAME {
            return Err(AppError::BadRequest(
                "the built-in weiagentcli Skill cannot be deleted".to_string(),
            ));
        }
        let settings = self.settings(None).await?;
        let manifest = settings
            .defaults
            .into_iter()
            .find(|item| item.name == name)
            .ok_or(AppError::NotFound)?;
        let skill_root = fs::canonicalize(PathBuf::from(&manifest.root_dir)).await?;
        if skill_root == self.root || !skill_root.starts_with(&self.root) {
            return Err(AppError::BadRequest(
                "Skill directory is outside the Skills directory".to_string(),
            ));
        }
        fs::remove_dir_all(&skill_root).await?;
        self.remove_skill_relation(&name).await?;
        self.store.delete_skill_metadata(&name).await?;
        self.refresh().await
    }

    async fn remove_skill_relation(&self, name: &str) -> AppResult<()> {
        let (mut relations, _) = self.read_skill_relations().await?;
        relations.remove(name);
        for names in relations.values_mut() {
            names.retain(|item| item != name);
        }
        relations.retain(|_, names| !names.is_empty());
        self.write_skill_relations(&relations).await
    }

    async fn read_content(
        &self,
        manifest: SkillManifest,
        file_path: Option<&str>,
    ) -> AppResult<SkillContent> {
        let root = PathBuf::from(&manifest.root_dir);
        let path = relative_file(&root, file_path.unwrap_or("SKILL.md"))?;
        let bytes = fs::read(&path).await?;
        let media_type = Self::image_media_type(&path).unwrap_or("text/plain");
        let (content, data_base64) = if media_type == "text/plain" {
            (String::from_utf8_lossy(&bytes).into_owned(), None)
        } else {
            (
                String::new(),
                Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    bytes,
                )),
            )
        };
        let mut files = discover_display_files(&root).await?;
        for file in &mut files {
            if file.kind != COLLAPSED_FILE_KIND {
                let path = root.join(&file.path);
                file.kind = Self::file_kind(&path, &root);
            }
        }
        Ok(SkillContent {
            name: manifest.name,
            path: path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/"),
            root_dir: display_path(&root),
            cwd: display_path(&root),
            content,
            media_type: media_type.to_string(),
            data_base64,
            files,
            created_at: manifest.created_at,
            updated_at: manifest.updated_at,
            scripts: manifest.scripts,
            references: manifest.references,
            examples: manifest.examples,
        })
    }

    fn image_media_type(path: &Path) -> Option<&'static str> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            "svg" => Some("image/svg+xml"),
            _ => None,
        }
    }

    fn file_kind(path: &Path, root: &Path) -> String {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let first = relative
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .unwrap_or_default();
        match first.to_ascii_lowercase().as_str() {
            "skill.md" => "skill_md".to_string(),
            "references" => "reference".to_string(),
            "examples" => "example".to_string(),
            "scripts" => "script".to_string(),
            _ => "other".to_string(),
        }
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition { kind: "function".to_string(), function: FunctionDefinition { name: SKILL_VIEW_TOOL_NAME.to_string(), description: "Load a bounded window of SKILL.md or a reference file for an enabled Skill. file_path may be relative to the Skill directory or an absolute path inside that same Skill directory. Use offset to continue after a truncated result.".to_string(), parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string"},"file_path":{"type":"string"},"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":1,"maximum":MAX_SKILL_VIEW_MAX_CHARS}},"required":["name"],"additionalProperties":false}) } },
        ]
    }

    pub async fn execute_tool_with_snapshot(
        &self,
        snapshot: &[SkillManifest],
        name: &str,
        arguments: Value,
    ) -> ToolResult {
        match name {
            SKILL_VIEW_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Args {
                    name: String,
                    file_path: Option<String>,
                    offset: Option<usize>,
                    max_chars: Option<usize>,
                }
                let args = match serde_json::from_value::<Args>(arguments) {
                    Ok(value) => value,
                    Err(error) => {
                        return ToolResult::Error {
                            message: error.to_string(),
                        };
                    }
                };
                let name = args.name.trim().to_ascii_lowercase();
                let Some(manifest) = snapshot.iter().find(|item| item.name == name) else {
                    return ToolResult::Error {
                        message: format!("Skill '{}' is not enabled for this Agent", args.name),
                    };
                };
                match self
                    .content_from_manifest(manifest, args.file_path.as_deref())
                    .await
                {
                    Ok(content) => {
                        let offset = args.offset.unwrap_or(0);
                        let max_chars = args
                            .max_chars
                            .unwrap_or(DEFAULT_SKILL_VIEW_MAX_CHARS)
                            .clamp(1, MAX_SKILL_VIEW_MAX_CHARS);
                        let (window, truncated, total_chars) =
                            bounded_skill_content(&content.content, offset, max_chars);
                        let file_path = content.path;
                        let stdout = if truncated {
                            format!(
                                "[skill_view name={name} file_path={file_path} offset={offset} returned_chars={} total_chars={total_chars} truncated=true]\n\n{window}",
                                window.chars().count()
                            )
                        } else {
                            window
                        };
                        ToolResult::Completed {
                            stdout,
                            stderr: String::new(),
                            exit_code: 0,
                            duration_ms: 0,
                            runtime: Some("skill".to_string()),
                            cwd: None,
                            output_cursor: 0,
                            structured_content: None,
                            is_error: false,
                        }
                    }
                    Err(error) => ToolResult::Error {
                        message: error.to_string(),
                    },
                }
            }
            _ => ToolResult::Error {
                message: format!("Unknown Skill tool: {name}"),
            },
        }
    }
}

fn bounded_skill_content(content: &str, offset: usize, max_chars: usize) -> (String, bool, usize) {
    let total_chars = content.chars().count();
    let window = content
        .chars()
        .skip(offset)
        .take(max_chars)
        .collect::<String>();
    let truncated = offset > 0 || offset.saturating_add(window.chars().count()) < total_chars;
    (window, truncated, total_chars)
}

async fn copy_skill_directory(source: &Path, destination: &Path) -> AppResult<()> {
    fs::create_dir_all(destination).await?;
    let mut directories = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((current_source, current_destination)) = directories.pop() {
        let mut entries = fs::read_dir(&current_source).await?;
        while let Some(entry) = entries.next_entry().await? {
            let source_path = entry.path();
            let destination_path = current_destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path).await?;
            if metadata.file_type().is_symlink() {
                return Err(AppError::BadRequest(
                    "Skill directories cannot contain symbolic links".to_string(),
                ));
            }
            if metadata.is_dir() {
                fs::create_dir_all(&destination_path).await?;
                directories.push((source_path, destination_path));
            } else if metadata.is_file() {
                fs::copy(&source_path, &destination_path).await?;
            }
        }
    }
    Ok(())
}

async fn validate_local_skill_directory(source: &str) -> AppResult<PathBuf> {
    let source = fs::canonicalize(source)
        .await
        .map_err(|_| AppError::NotFound)?;
    let metadata = fs::symlink_metadata(&source).await?;
    if !metadata.is_dir() || source.join("SKILL.md").is_symlink() {
        return Err(AppError::BadRequest(
            "selected directory must contain a regular SKILL.md".to_string(),
        ));
    }
    if !fs::try_exists(source.join("SKILL.md")).await? {
        return Err(AppError::BadRequest(
            "selected directory must contain SKILL.md".to_string(),
        ));
    }
    Ok(source)
}

#[derive(Debug, Clone)]
struct GithubSkillSource {
    owner: String,
    repository: String,
    branch: String,
    skill_directory: String,
}

#[derive(Debug, Clone)]
struct RemoteSkillSource {
    skill_md_url: Url,
    github: Option<GithubSkillSource>,
}

#[derive(Clone)]
struct RemoteRoute {
    source: RemoteSkillSource,
    client: reqwest::Client,
}

struct SelectedRemoteRoute {
    routes: Vec<RemoteRoute>,
    skill_md: String,
}

#[derive(Debug, Deserialize)]
struct GithubTreeResponse {
    tree: Vec<GithubTreeEntry>,
}

#[derive(Debug, Deserialize)]
struct GithubTreeEntry {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SkillReference {
    path: String,
    sibling: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GithubSkillFile {
    remote_path: String,
    relative_path: String,
    sibling: bool,
    size: Option<u64>,
}

fn looks_like_url(value: &str) -> bool {
    value.contains("://")
}

fn normalize_remote_source(source: &str) -> AppResult<RemoteSkillSource> {
    let mut url = Url::parse(source)
        .map_err(|error| AppError::BadRequest(format!("invalid Skill URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::BadRequest(
            "Skill URL must use HTTP or HTTPS".to_string(),
        ));
    }
    url.set_fragment(None);

    if url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
    {
        let segments = url
            .path_segments()
            .ok_or_else(|| AppError::BadRequest("invalid GitHub Skill URL".to_string()))?
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if segments.len() < 4 {
            return Err(AppError::BadRequest(
                "GitHub repository URL must include a Skill directory or SKILL.md".to_string(),
            ));
        }
        let owner = segments[0].clone();
        let repository = segments[1].clone();
        let kind = segments[2].as_str();
        if kind != "tree" && kind != "blob" {
            return Err(AppError::BadRequest(
                "GitHub Skill URL must use /tree/ or /blob/".to_string(),
            ));
        }
        let branch = segments[3].clone();
        let path = segments[4..].join("/");
        if path.is_empty() || !valid_download_path(&path) {
            return Err(AppError::BadRequest(
                "invalid GitHub Skill path".to_string(),
            ));
        }
        let (skill_directory, skill_md_path) = if kind == "blob" {
            if !path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
            {
                return Err(AppError::BadRequest(
                    "GitHub file URL must point to SKILL.md".to_string(),
                ));
            }
            let directory = path
                .rsplit_once('/')
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_default();
            (directory, path)
        } else {
            let directory = path.trim_end_matches('/').to_string();
            if directory.is_empty() || directory.eq_ignore_ascii_case("SKILL.md") {
                return Err(AppError::BadRequest(
                    "GitHub repository URL must include a specific Skill directory".to_string(),
                ));
            }
            (directory.clone(), format!("{directory}/SKILL.md"))
        };
        let raw_url = Url::parse(&format!(
            "https://raw.githubusercontent.com/{owner}/{repository}/{branch}/{skill_md_path}"
        ))
        .map_err(|error| AppError::BadRequest(format!("invalid GitHub Skill URL: {error}")))?;
        return Ok(RemoteSkillSource {
            skill_md_url: raw_url,
            github: Some(GithubSkillSource {
                owner,
                repository,
                branch,
                skill_directory,
            }),
        });
    }

    let is_skill_md = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"));
    if is_skill_md {
        let github = github_source_from_raw_url(&url);
        return Ok(RemoteSkillSource {
            skill_md_url: url,
            github,
        });
    }

    let mut path = url.path().trim_end_matches('/').to_string();
    path.push_str("/SKILL.md");
    url.set_path(&path);
    Ok(RemoteSkillSource {
        skill_md_url: url,
        github: None,
    })
}

fn github_source_from_raw_url(url: &Url) -> Option<GithubSkillSource> {
    if !url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("raw.githubusercontent.com"))
    {
        return None;
    }
    let segments = url
        .path_segments()?
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if segments.len() < 5 || !segments.last()?.eq_ignore_ascii_case("SKILL.md") {
        return None;
    }
    Some(GithubSkillSource {
        owner: segments[0].clone(),
        repository: segments[1].clone(),
        branch: segments[2].clone(),
        skill_directory: segments[3..segments.len() - 1].join("/"),
    })
}

fn github_jsdelivr_source(source: &RemoteSkillSource) -> AppResult<Option<RemoteSkillSource>> {
    let Some(github) = source.github.as_ref() else {
        return Ok(None);
    };
    let path = source
        .skill_md_url
        .path_segments()
        .and_then(|segments| {
            let segments = segments.collect::<Vec<_>>();
            (segments.len() >= 4).then(|| segments[3..].join("/"))
        })
        .ok_or_else(|| AppError::BadRequest("invalid GitHub Skill CDN path".to_string()))?;
    let skill_md_url = Url::parse(&format!(
        "https://cdn.jsdelivr.net/gh/{}/{}@{}/{}",
        github.owner, github.repository, github.branch, path
    ))
    .map_err(|error| AppError::BadRequest(format!("invalid GitHub Skill CDN URL: {error}")))?;
    Ok(Some(RemoteSkillSource {
        skill_md_url,
        github: Some(github.clone()),
    }))
}

fn build_remote_client(proxy_url: Option<&str>, timeout: Duration) -> AppResult<reqwest::Client> {
    crate::http_client::build_reqwest_client(
        crate::http_client::HttpClientConfig::new()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(12)))
            .user_agent("WeiAgent Skill importer")
            .no_system_proxy()
            .proxy_url(proxy_url),
    )
    .map_err(|error| AppError::Process(format!("failed to create Skill download client: {error}")))
}

async fn select_remote_route(
    source: RemoteSkillSource,
    cdn: Option<RemoteSkillSource>,
    proxy_url: Option<&str>,
) -> AppResult<SelectedRemoteRoute> {
    let has_proxy = proxy_url
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let mut candidates = Vec::new();
    if has_proxy {
        candidates.push((source.clone(), proxy_url.map(str::to_string)));
        if let Some(cdn) = cdn.clone() {
            candidates.push((cdn, proxy_url.map(str::to_string)));
        }
    }
    candidates.push((source, None));
    if let Some(cdn) = cdn {
        candidates.push((cdn, None));
    }

    let mut routes = Vec::new();
    let mut errors = Vec::new();
    for (source, proxy_url) in candidates.iter() {
        let client = match build_remote_client(proxy_url.as_deref(), REMOTE_PREFLIGHT_TIMEOUT) {
            Ok(client) => client,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        match download_text_with_retries(&client, &source.skill_md_url).await {
            Ok(skill_md) => match validate_skill_markdown_text(&skill_md) {
                Ok(()) => {
                    routes.push(RemoteRoute {
                        source: source.clone(),
                        client,
                    });
                    for (remaining_source, remaining_proxy_url) in candidates.iter() {
                        if remaining_source.skill_md_url == source.skill_md_url
                            && remaining_proxy_url == proxy_url
                        {
                            continue;
                        }
                        let Ok(remaining_client) = build_remote_client(
                            remaining_proxy_url.as_deref(),
                            REMOTE_PREFLIGHT_TIMEOUT,
                        ) else {
                            continue;
                        };
                        routes.push(RemoteRoute {
                            source: remaining_source.clone(),
                            client: remaining_client,
                        });
                    }
                    return Ok(SelectedRemoteRoute { routes, skill_md });
                }
                Err(error) => errors.push(error.to_string()),
            },
            Err(error) => errors.push(error.to_string()),
        }
    }

    Err(AppError::Process(format!(
        "could not reach a valid Skill source after fallback attempts: {}",
        errors.join("; "),
    )))
}

async fn download_bytes_with_fallback(
    routes: &[RemoteRoute],
    github: &GithubSkillSource,
    remote_path: &str,
) -> AppResult<Vec<u8>> {
    let mut errors = Vec::new();
    for route in routes {
        let url = remote_skill_file_url(&route.source, github, remote_path)?;
        match download_bytes_with_retries(&route.client, &url).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => errors.push(format!("{url}: {error}")),
        }
    }
    Err(AppError::Process(format!(
        "failed to download Skill resource after fallback attempts: {}",
        errors.join("; "),
    )))
}

async fn download_referenced_bytes_with_fallback(
    routes: &[RemoteRoute],
    relative: &str,
) -> AppResult<Option<Vec<u8>>> {
    let mut errors = Vec::new();
    for route in routes {
        let file_url = route.source.skill_md_url.join(relative).map_err(|error| {
            AppError::BadRequest(format!("invalid referenced Skill path: {error}"))
        })?;
        match download_bytes_if_exists_with_retries(&route.client, &file_url).await {
            Ok(Some(bytes)) => return Ok(Some(bytes)),
            Ok(None) => continue,
            Err(error) => errors.push(format!("{file_url}: {error}")),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(AppError::Process(format!(
            "failed to download referenced Skill resource after fallback attempts: {}",
            errors.join("; "),
        )))
    }
}

async fn download_text_with_retries(client: &reqwest::Client, url: &Url) -> AppResult<String> {
    let bytes = download_bytes_with_retries(client, url).await?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("Skill resource is not valid UTF-8".to_string()))
}

async fn download_bytes_with_retries(client: &reqwest::Client, url: &Url) -> AppResult<Vec<u8>> {
    let mut last_error = None;
    for attempt in 0..REMOTE_REQUEST_RETRIES {
        match download_bytes(client, url).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REMOTE_REQUEST_RETRIES {
                    tokio::time::sleep(REMOTE_RETRY_BACKOFF * (attempt as u32 + 1)).await;
                }
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| AppError::Process(format!("failed to download Skill resource: {url}"))))
}

async fn download_bytes_if_exists_with_retries(
    client: &reqwest::Client,
    url: &Url,
) -> AppResult<Option<Vec<u8>>> {
    let mut last_error = None;
    for attempt in 0..REMOTE_REQUEST_RETRIES {
        match download_bytes_if_exists(client, url).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REMOTE_REQUEST_RETRIES {
                    tokio::time::sleep(REMOTE_RETRY_BACKOFF * (attempt as u32 + 1)).await;
                }
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| AppError::Process(format!("failed to download Skill resource: {url}"))))
}

async fn download_bytes(client: &reqwest::Client, url: &Url) -> AppResult<Vec<u8>> {
    download_bytes_if_exists(client, url)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("Skill resource returned HTTP 404: {}", url)))
}

async fn download_bytes_if_exists(
    client: &reqwest::Client,
    url: &Url,
) -> AppResult<Option<Vec<u8>>> {
    let response = client.get(url.clone()).send().await.map_err(|error| {
        AppError::Process(format!("failed to download Skill resource: {error}"))
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::BadRequest(format!(
            "Skill resource returned HTTP {}: {}",
            status.as_u16(),
            url
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| AppError::Process(format!("failed to read Skill resource: {error}")))?;
    Ok(Some(bytes.to_vec()))
}

fn validate_skill_markdown_text(content: &str) -> AppResult<()> {
    let trimmed = content.trim_start_matches('\u{feff}').trim_start();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "downloaded file is empty, expected SKILL.md frontmatter".to_string(),
        ));
    }

    let leading = trimmed
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if leading.starts_with("<!doctype html")
        || leading.starts_with("<html")
        || leading.starts_with("<head")
    {
        return Err(AppError::BadRequest(
            "downloaded Skill URL returned an HTML page instead of SKILL.md".to_string(),
        ));
    }

    let Some(frontmatter_rest) = trimmed.strip_prefix("---") else {
        return Err(AppError::BadRequest(
            "downloaded file is not a valid SKILL.md: missing frontmatter".to_string(),
        ));
    };
    if !frontmatter_rest.starts_with('\n') && !frontmatter_rest.starts_with("\r\n") {
        return Err(AppError::BadRequest(
            "downloaded file is not a valid SKILL.md: invalid frontmatter opening".to_string(),
        ));
    }

    let Some((frontmatter, _)) = frontmatter_rest.split_once("\n---") else {
        return Err(AppError::BadRequest(
            "downloaded file is not a valid SKILL.md: missing frontmatter closing".to_string(),
        ));
    };
    let mut name = None;
    let mut description = None;
    for line in frontmatter.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['\"', '\'']);
        match key.trim().to_ascii_lowercase().as_str() {
            "name" if !value.is_empty() => name = Some(value),
            "description" if !value.is_empty() => description = Some(value),
            _ => {}
        }
    }
    if name.is_none() || description.is_none() {
        return Err(AppError::BadRequest(
            "downloaded file is not a valid SKILL.md with name and description".to_string(),
        ));
    }
    Ok(())
}

async fn fetch_github_tree(
    client: &reqwest::Client,
    source: &GithubSkillSource,
) -> AppResult<Vec<GithubTreeEntry>> {
    let url = Url::parse(&format!(
        "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
        source.owner, source.repository, source.branch
    ))
    .map_err(|error| AppError::Process(format!("invalid GitHub API URL: {error}")))?;
    let response =
        client.get(url).send().await.map_err(|error| {
            AppError::Process(format!("failed to read GitHub Skill tree: {error}"))
        })?;
    if !response.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "GitHub tree lookup returned HTTP {}",
            response.status().as_u16()
        )));
    }
    response
        .json::<GithubTreeResponse>()
        .await
        .map(|value| value.tree)
        .map_err(|error| AppError::Process(format!("invalid GitHub tree response: {error}")))
}

async fn fetch_github_tree_with_fallback(
    routes: &[RemoteRoute],
    source: &GithubSkillSource,
) -> Option<Vec<GithubTreeEntry>> {
    for route in routes {
        if let Ok(tree) = fetch_github_tree_with_retries(&route.client, source).await {
            return Some(tree);
        }
    }
    None
}

async fn fetch_github_tree_with_retries(
    client: &reqwest::Client,
    source: &GithubSkillSource,
) -> AppResult<Vec<GithubTreeEntry>> {
    let mut last_error = None;
    for attempt in 0..REMOTE_REQUEST_RETRIES {
        match fetch_github_tree(client, source).await {
            Ok(tree) => return Ok(tree),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REMOTE_REQUEST_RETRIES {
                    tokio::time::sleep(REMOTE_RETRY_BACKOFF * (attempt as u32 + 1)).await;
                }
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| AppError::Process("failed to read GitHub Skill tree".to_string())))
}

fn select_github_skill_files(
    tree: &[GithubTreeEntry],
    skill_directory: &str,
    referenced: &HashSet<SkillReference>,
) -> Vec<GithubSkillFile> {
    let skill_directory = skill_directory.trim_matches('/');
    let prefix = format!("{skill_directory}/");
    let (sibling_prefix, skill_name) = skill_directory
        .rsplit_once('/')
        .map(|(parent, name)| (format!("{parent}/"), name))
        .unwrap_or_else(|| (String::new(), skill_directory));
    let referenced_directories = referenced
        .iter()
        .filter(|item| !item.sibling)
        .filter_map(|item| item.path.split_once('/').map(|(directory, _)| directory))
        .collect::<HashSet<_>>();
    let mut files = tree
        .iter()
        .filter(|entry| entry.entry_type == "blob")
        .filter_map(|entry| {
            let (relative, sibling) = if let Some(relative) = entry.path.strip_prefix(&prefix) {
                (relative.to_string(), false)
            } else if let Some(relative) = entry.path.strip_prefix(&sibling_prefix) {
                let first = relative.split('/').next().unwrap_or_default();
                // Every directory next to the requested Skill is an attached resource.
                // Files directly in the parent directory are not sibling directories.
                if first.is_empty() || first == skill_name || !relative.contains('/') {
                    return None;
                }
                (entry.path.clone(), true)
            } else {
                return None;
            };
            let first = relative.split('/').next().unwrap_or_default();
            let in_standard_directory = SKILL_RESOURCE_DIRECTORIES
                .iter()
                .any(|directory| !sibling && first == *directory);
            let in_referenced_directory = referenced_directories.contains(first);
            (sibling
                || in_standard_directory
                || in_referenced_directory
                || referenced
                    .iter()
                    .any(|item| !item.sibling && item.path == relative)
                || relative.eq_ignore_ascii_case("SKILL.md"))
            .then_some(GithubSkillFile {
                remote_path: entry.path.clone(),
                relative_path: if sibling {
                    entry
                        .path
                        .strip_prefix(&sibling_prefix)
                        .unwrap_or(&entry.path)
                        .to_string()
                } else {
                    relative
                },
                sibling,
                size: entry.size,
            })
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| a.remote_path.cmp(&b.remote_path));
    files.dedup_by(|a, b| a.remote_path == b.remote_path);
    files
}

fn remote_skill_file_url(
    source: &RemoteSkillSource,
    github: &GithubSkillSource,
    remote_path: &str,
) -> AppResult<Url> {
    let url = if source
        .skill_md_url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("raw.githubusercontent.com"))
    {
        Url::parse(&format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            github.owner, github.repository, github.branch, remote_path
        ))
        .map_err(|error| AppError::BadRequest(format!("invalid Skill resource path: {error}")))?
    } else if source
        .skill_md_url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("cdn.jsdelivr.net"))
    {
        Url::parse(&format!(
            "https://cdn.jsdelivr.net/gh/{}/{}@{}/{}",
            github.owner, github.repository, github.branch, remote_path
        ))
        .map_err(|error| AppError::BadRequest(format!("invalid Skill resource path: {error}")))?
    } else {
        return Err(AppError::BadRequest(
            "unsupported GitHub Skill resource source".to_string(),
        ));
    };
    Ok(url)
}

async fn download_referenced_files(
    manager: &SkillManager,
    routes: &[RemoteRoute],
    temporary: &Path,
    task_id: Option<Uuid>,
    referenced: &HashSet<SkillReference>,
    total_files: usize,
    downloaded: &mut usize,
    downloaded_bytes: &mut u64,
) -> AppResult<()> {
    for reference in referenced {
        if reference.sibling {
            continue;
        }
        let relative = &reference.path;
        if !valid_download_path(relative) || relative.eq_ignore_ascii_case("SKILL.md") {
            continue;
        }
        manager
            .update_import_download_progress(
                task_id,
                Some(relative.clone()),
                *downloaded,
                total_files,
                *downloaded_bytes,
                None,
            )
            .await;
        let destination = temporary.join(relative);
        if is_cached_download(&destination, None).await {
            *downloaded_bytes = downloaded_bytes.saturating_add(
                fs::metadata(&destination)
                    .await
                    .map(|metadata| metadata.len())
                    .unwrap_or(0),
            );
            *downloaded += 1;
            continue;
        }
        let Some(bytes) = download_referenced_bytes_with_fallback(routes, relative).await? else {
            continue;
        };
        *downloaded_bytes = downloaded_bytes.saturating_add(bytes.len() as u64);
        if *downloaded_bytes > MAX_REMOTE_SKILL_BYTES {
            return Err(AppError::BadRequest(
                "remote Skill is too large".to_string(),
            ));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        write_skill_download(&destination, &bytes).await?;
        *downloaded += 1;
        manager
            .update_import_download_progress(
                task_id,
                Some(relative.clone()),
                *downloaded,
                total_files,
                *downloaded_bytes,
                None,
            )
            .await;
    }
    Ok(())
}

async fn is_cached_download(path: &Path, expected_size: Option<u64>) -> bool {
    fs::metadata(path)
        .await
        .map(|metadata| {
            metadata.is_file() && expected_size.is_none_or(|size| metadata.len() == size)
        })
        .unwrap_or(false)
}

async fn write_skill_download(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let mut last_error = None;
    for attempt in 0..REMOTE_REQUEST_RETRIES {
        match fsutil::write_atomic(path, bytes).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REMOTE_REQUEST_RETRIES {
                    tokio::time::sleep(REMOTE_RETRY_BACKOFF * (attempt as u32 + 1)).await;
                }
            }
        }
    }
    Err(AppError::Process(format!(
        "failed to store Skill resource {} after retries: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown storage error".to_string())
    )))
}

fn referenced_skill_files(content: &str) -> HashSet<SkillReference> {
    let mut result = HashSet::new();
    for line in content.lines() {
        let mut remainder = line;
        while let Some(start) = remainder.find("](") {
            let after = &remainder[start + 2..];
            let Some(end) = after.find(')') else { break };
            add_referenced_path(&mut result, &after[..end]);
            remainder = &after[end + 1..];
        }
        for token in line.split_whitespace() {
            add_referenced_path(
                &mut result,
                token.trim_matches([
                    '`', '<', '>', '(', ')', '[', ']', '"', '\'', ',', '.', ';', ':',
                ]),
            );
        }
    }
    result
}

fn add_referenced_path(result: &mut HashSet<SkillReference>, value: &str) {
    let value = value.split(['#', '?']).next().unwrap_or_default().trim();
    let sibling = value.starts_with('/');
    let value = value.trim_start_matches('/');
    if value.is_empty()
        || value.contains("://")
        || value.starts_with('#')
        || !valid_download_path(value)
    {
        return;
    }
    let normalized = value.trim_start_matches("./").replace('\\', "/");
    result.insert(SkillReference {
        path: normalized,
        sibling,
    });
}

fn valid_download_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\0')
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

pub fn is_skill_tool(name: &str) -> bool {
    name == SKILL_VIEW_TOOL_NAME
}

fn duplicate_names(items: &[SkillManifest]) -> HashSet<String> {
    let mut counts = HashMap::new();
    for item in items {
        *counts.entry(item.name.clone()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

fn find_skill(overrides: &SkillOverrides, name: &str) -> Option<bool> {
    overrides
        .skills
        .iter()
        .find(|item| item.name == name)
        .and_then(|item| item.enabled)
}

fn find_group(
    overrides: &SkillOverrides,
    group: &crate::storage::SkillGroupRecord,
) -> Option<bool> {
    overrides
        .groups
        .iter()
        .find(|item| {
            item.group_id.as_deref() == Some(group.id.as_str())
                || item.group.eq_ignore_ascii_case(&group.name)
        })
        .and_then(|item| item.enabled)
}

fn normalize_group_overrides(
    groups: Vec<crate::domain::SkillGroupOverride>,
    by_name: &HashMap<String, &crate::storage::SkillGroupRecord>,
) -> Vec<crate::domain::SkillGroupOverride> {
    groups
        .into_iter()
        .map(|mut item| {
            if let Some(group) = by_name.get(&item.group.to_ascii_lowercase()) {
                item.group_id = Some(group.id.clone());
                item.group = group.name.clone();
            }
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SkillGroupOverride, SkillOverride};

    #[test]
    fn normalizes_supported_remote_skill_urls() {
        let tree = normalize_remote_source(
            "https://github.com/vercel-labs/skills/tree/main/skills/find-skills",
        )
        .unwrap();
        assert_eq!(
            tree.skill_md_url.as_str(),
            "https://raw.githubusercontent.com/vercel-labs/skills/main/skills/find-skills/SKILL.md"
        );
        assert_eq!(
            tree.github.as_ref().unwrap().skill_directory,
            "skills/find-skills"
        );

        let blob = normalize_remote_source(
            "https://github.com/vercel-labs/skills/blob/main/skills/find-skills/SKILL.md",
        )
        .unwrap();
        assert_eq!(
            blob.skill_md_url.as_str(),
            "https://raw.githubusercontent.com/vercel-labs/skills/main/skills/find-skills/SKILL.md"
        );

        let raw = normalize_remote_source(
            "https://raw.githubusercontent.com/vercel-labs/skills/main/skills/find-skills/SKILL.md",
        )
        .unwrap();
        assert!(raw.github.is_some());

        let cdn = github_jsdelivr_source(&tree).unwrap().unwrap();
        assert_eq!(
            cdn.skill_md_url.as_str(),
            "https://cdn.jsdelivr.net/gh/vercel-labs/skills@main/skills/find-skills/SKILL.md"
        );
    }

    #[test]
    fn rejects_ambiguous_and_invalid_remote_skill_urls() {
        assert!(normalize_remote_source("https://github.com/vercel-labs/skills").is_err());
        assert!(
            normalize_remote_source(
                "https://github.com/vercel-labs/skills/blob/main/skills/find-skills/README.md"
            )
            .is_err()
        );
        assert!(
            normalize_remote_source("https://github.com/vercel-labs/skills/tree/main/../private")
                .is_err()
        );
    }

    #[test]
    fn validates_skill_frontmatter_without_rejecting_html_examples() {
        let archify = "---\nname: archify\ndescription: Create diagrams with HTML output\n---\n\nThe generated document uses an <html lang> element and may contain HTML examples.\n";
        assert!(validate_skill_markdown_text(archify).is_ok());

        let github_page =
            "\n<!DOCTYPE html>\n<html lang=\"en\"><head></head><body>name: fake</body></html>";
        assert!(validate_skill_markdown_text(github_page).is_err());

        let body_only = "# Not frontmatter\nname: fake\ndescription: fake\n";
        assert!(validate_skill_markdown_text(body_only).is_err());
    }

    #[test]
    fn extracts_referenced_relative_skill_resources() {
        let files = referenced_skill_files(
            "See [API](references/api.md), [Config](docs/config.md), [Shared](/hyperframes-animation/SKILL.md), `scripts/check.ps1`, and https://example.test/nope.",
        );
        assert!(files.contains(&SkillReference {
            path: "references/api.md".to_string(),
            sibling: false,
        }));
        assert!(files.contains(&SkillReference {
            path: "docs/config.md".to_string(),
            sibling: false,
        }));
        assert!(files.contains(&SkillReference {
            path: "hyperframes-animation/SKILL.md".to_string(),
            sibling: true,
        }));
        assert!(files.contains(&SkillReference {
            path: "scripts/check.ps1".to_string(),
            sibling: false,
        }));
        assert!(!files.iter().any(|file| file.path.contains("example.test")));
    }

    #[test]
    fn selects_existing_referenced_files_from_custom_directories() {
        let tree = vec![
            GithubTreeEntry {
                path: "skills/example/SKILL.md".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/example/docs/config.md".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/example/docs/unreferenced.txt".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/example/docs/nested/guide.md".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/example/guides/setup.md".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/hyperframes-animation/SKILL.md".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/hyperframes-animation/references/animation.md".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
            GithubTreeEntry {
                path: "skills/another-attached-resource/data.json".to_string(),
                entry_type: "blob".to_string(),
                size: Some(10),
            },
        ];
        let referenced = referenced_skill_files(
            "[Config](docs/config.md) [Setup](guides/setup.md) [Animation](/hyperframes-animation/SKILL.md)",
        );

        let files = select_github_skill_files(&tree, "skills/example", &referenced);

        assert!(
            files
                .iter()
                .any(|file| file.remote_path == "skills/example/docs/config.md")
        );
        assert!(
            files
                .iter()
                .any(|file| file.remote_path == "skills/example/docs/unreferenced.txt")
        );
        assert!(
            files
                .iter()
                .any(|file| file.remote_path == "skills/example/docs/nested/guide.md")
        );
        assert!(
            files
                .iter()
                .any(|file| file.remote_path == "skills/example/guides/setup.md")
        );
        assert!(files.iter().any(|file| {
            file.remote_path == "skills/hyperframes-animation/SKILL.md"
                && file.relative_path == "hyperframes-animation/SKILL.md"
                && file.sibling
        }));
        assert!(files.iter().any(|file| {
            file.remote_path == "skills/hyperframes-animation/references/animation.md"
                && file.relative_path == "hyperframes-animation/references/animation.md"
                && file.sibling
        }));
        assert!(files.iter().any(|file| {
            file.remote_path == "skills/another-attached-resource/data.json"
                && file.relative_path == "another-attached-resource/data.json"
                && file.sibling
        }));
    }

    #[test]
    fn bounded_skill_content_reports_continuation_metadata() {
        let (window, truncated, total_chars) = bounded_skill_content("0123456789", 3, 4);
        assert_eq!(window, "3456");
        assert!(truncated);
        assert_eq!(total_chars, 10);

        let (window, truncated, total_chars) = bounded_skill_content("short", 0, 20);
        assert_eq!(window, "short");
        assert!(!truncated);
        assert_eq!(total_chars, 5);
    }

    async fn write_skill(root: &std::path::Path, directory: &str, name: &str, group: &str) {
        let skill_root = root.join(directory);
        fs::create_dir_all(&skill_root).await.unwrap();
        fs::write(
            skill_root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} description\ngroup: {group}\n---\nInstructions for {name}.\n"),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn duplicate_names_are_diagnosed_and_excluded() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        write_skill(&data.path().join("skills"), "one", "same-name", "group").await;
        write_skill(&data.path().join("skills"), "two", "same-name", "group").await;
        let manager = SkillManager::new(store.clone()).await.unwrap();

        let settings = manager.settings(None).await.unwrap();
        assert_eq!(settings.defaults.len(), 1);
        assert_eq!(settings.effective.len(), 1);
        assert_eq!(settings.defaults[0].name, BUILTIN_WEIAGENT_CLI_SKILL_NAME);
        assert_eq!(settings.conflicts.len(), 1);
        assert_eq!(settings.conflicts[0].name, "same-name");
        assert_eq!(settings.conflicts[0].count, 2);
        assert_eq!(settings.conflicts[0].paths.len(), 2);
        assert!(
            settings.conflicts[0]
                .paths
                .iter()
                .all(|path| path.contains("skills"))
        );
        assert!(
            settings
                .diagnostics
                .iter()
                .any(|item| item.message.contains("duplicate"))
        );
    }

    #[tokio::test]
    async fn nested_skill_markdown_is_a_file_not_another_skill() {
        let data = tempfile::tempdir().unwrap();
        let skills_root = data.path().join("skills");
        write_skill(&skills_root, "dashi-ppt", "dashi-ppt", "development").await;
        let nested_root = skills_root
            .join("dashi-ppt")
            .join("project")
            .join("node_modules")
            .join("nested-skill");
        fs::create_dir_all(&nested_root).await.unwrap();
        fs::write(
            nested_root.join("SKILL.md"),
            "---\nname: nested-skill\ndescription: nested skill description\n---\nNested instructions.\n",
        )
        .await
        .unwrap();

        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store).await.unwrap();
        let settings = manager.settings(None).await.unwrap();

        assert!(
            settings
                .defaults
                .iter()
                .any(|skill| skill.name == "dashi-ppt")
        );
        assert!(
            !settings
                .defaults
                .iter()
                .any(|skill| skill.name == "nested-skill")
        );
        assert!(
            !settings
                .effective
                .iter()
                .any(|skill| skill.name == "nested-skill")
        );

        let dashi = settings
            .defaults
            .iter()
            .find(|skill| skill.name == "dashi-ppt")
            .unwrap();
        let content = manager.content_from_manifest(dashi, None).await.unwrap();
        assert!(content.files.iter().any(|file| {
            file.path == "project/node_modules/..." && file.kind == COLLAPSED_FILE_KIND
        }));
        assert!(
            !content
                .files
                .iter()
                .any(|file| file.path.starts_with("project/node_modules/nested-skill/"))
        );
    }

    #[tokio::test]
    async fn completed_import_task_remains_available_for_cli_polling() {
        let data = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("SKILL.md"),
            "---\nname: local-skill\ndescription: local-skill description\ngroup: development\n---\nInstructions for local-skill.\n",
        )
        .await
        .unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store).await.unwrap();

        let task = manager
            .create_import_task(source.path().to_string_lossy().as_ref(), "local", None)
            .await
            .unwrap();
        manager
            .run_import_task(task.id, task.source.clone(), None, None)
            .await;

        let tasks = manager.list_import_tasks().await;
        let completed = tasks.iter().find(|item| item.id == task.id).unwrap();
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.phase, "completed");
        assert_eq!(completed.progress, 100);
        assert!(data.path().join("skills").join("local-skill").exists());
        let expected_source = super::display_path(&fs::canonicalize(source.path()).await.unwrap());
        let settings = manager.settings(None).await.unwrap();
        assert_eq!(
            settings
                .defaults
                .iter()
                .find(|skill| skill.name == "local-skill")
                .and_then(|skill| skill.source_url.as_deref()),
            Some(expected_source.as_str()),
        );
    }

    #[tokio::test]
    async fn attached_skill_relations_are_exposed_on_manifests() {
        let data = tempfile::tempdir().unwrap();
        let skills_root = data.path().join("skills");
        write_skill(&skills_root, "source", "source-skill", "development").await;
        write_skill(&skills_root, "attached", "attached-skill", "development").await;
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store).await.unwrap();

        manager
            .update_skill_relations("source-skill", &["attached-skill".to_string()])
            .await
            .unwrap();

        let settings = manager.settings(None).await.unwrap();
        let attached = settings
            .defaults
            .iter()
            .find(|skill| skill.name == "attached-skill")
            .unwrap();
        assert_eq!(attached.source_skill.as_deref(), Some("source-skill"));
        assert_eq!(
            settings
                .defaults
                .iter()
                .find(|skill| skill.name == "source-skill")
                .unwrap()
                .source_skill,
            None
        );
        let relations = serde_json::from_str::<BTreeMap<String, Vec<String>>>(
            &fs::read_to_string(skills_root.join(SKILL_RELATIONS_FILE))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            relations.get("source-skill"),
            Some(&vec!["attached-skill".to_string()])
        );
    }

    #[tokio::test]
    async fn attached_skill_inherits_source_icon_and_icon_proxy() {
        let data = tempfile::tempdir().unwrap();
        let skills_root = data.path().join("skills");
        write_skill(&skills_root, "source", "source-skill", "development").await;
        write_skill(&skills_root, "attached", "attached-skill", "development").await;
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store.clone()).await.unwrap();

        manager
            .update_skill_relations("source-skill", &["attached-skill".to_string()])
            .await
            .unwrap();
        store
            .update_skill_icon("source-skill", Some("source-icon.png".to_string()))
            .await
            .unwrap();
        store
            .update_skill_proxies(
                "source-skill",
                Some("https://proxy.example/icon".to_string()),
                None,
            )
            .await
            .unwrap();

        let settings = manager.settings(None).await.unwrap();
        let source = settings
            .defaults
            .iter()
            .find(|skill| skill.name == "source-skill")
            .unwrap();
        let attached = settings
            .defaults
            .iter()
            .find(|skill| skill.name == "attached-skill")
            .unwrap();
        assert_eq!(source.icon_url.as_deref(), Some("source-icon.png"));
        assert_eq!(attached.icon_url, source.icon_url);
        assert_eq!(attached.icon_proxy_url, source.icon_proxy_url);

        store.update_skill_icon("source-skill", None).await.unwrap();
        store
            .update_skill_proxies("source-skill", None, None)
            .await
            .unwrap();
        let settings = manager.settings(None).await.unwrap();
        let attached = settings
            .defaults
            .iter()
            .find(|skill| skill.name == "attached-skill")
            .unwrap();
        assert_eq!(attached.icon_url, None);
        assert_eq!(attached.icon_proxy_url, None);
    }

    #[tokio::test]
    async fn deleting_skill_removes_directory_and_metadata() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        write_skill(
            &data.path().join("skills"),
            "removable",
            "removable-skill",
            "development",
        )
        .await;
        let manager = SkillManager::new(store.clone()).await.unwrap();
        store
            .update_skill_global_override("removable-skill", Some(false))
            .await
            .unwrap();
        store
            .update_skill_icon("removable-skill", Some("icon.png".to_string()))
            .await
            .unwrap();

        manager.delete_skill("removable-skill").await.unwrap();

        let settings = manager.settings(None).await.unwrap();
        assert_eq!(settings.defaults.len(), 1);
        assert_eq!(settings.defaults[0].name, BUILTIN_WEIAGENT_CLI_SKILL_NAME);
        assert!(settings.orphan_skills.is_empty());
        assert!(!data.path().join("skills").join("removable").exists());
        assert!(
            !store
                .read_skill_global_overrides()
                .await
                .unwrap()
                .skills
                .iter()
                .any(|item| item.name == "removable-skill")
        );
        assert!(
            !store
                .read_skill_icons()
                .await
                .unwrap()
                .contains_key("removable-skill")
        );
    }

    #[tokio::test]
    async fn startup_overwrites_existing_builtin_weiagentcli_content() {
        let data = tempfile::tempdir().unwrap();
        let skill_root = data
            .path()
            .join("skills")
            .join(BUILTIN_WEIAGENT_CLI_SKILL_NAME);
        fs::create_dir_all(&skill_root).await.unwrap();
        let existing = "---\nname: weiagentcli\ndescription: User customized weiagentcli skill.\n---\nKeep this content.\n";
        fs::write(skill_root.join("SKILL.md"), existing)
            .await
            .unwrap();
        fs::write(skill_root.join("stale.txt"), "stale bundled file")
            .await
            .unwrap();

        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store).await.unwrap();

        assert_eq!(
            fs::read_to_string(skill_root.join("SKILL.md"))
                .await
                .unwrap(),
            BUILTIN_WEIAGENT_CLI_SKILL
        );
        assert!(!skill_root.join("stale.txt").exists());
        for (relative_path, content) in BUILTIN_WEIAGENT_CLI_FILES {
            assert_eq!(
                fs::read_to_string(skill_root.join(relative_path))
                    .await
                    .unwrap(),
                *content,
                "bundled builtin Skill file was not materialized: {relative_path}"
            );
        }
        let settings = manager.settings(None).await.unwrap();
        let builtin = settings
            .groups
            .iter()
            .find(|group| group.is_builtin)
            .unwrap();
        assert_eq!(builtin.id, BUILTIN_SKILL_GROUP_ID);
        assert!(settings.defaults.iter().any(|skill| skill.is_builtin));
    }

    #[tokio::test]
    async fn migrates_legacy_builtin_client_skill_to_weiagentcli() {
        let data = tempfile::tempdir().unwrap();
        let legacy_root = data.path().join("skills").join(LEGACY_BUILTIN_SKILL_NAME);
        fs::create_dir_all(&legacy_root).await.unwrap();
        fs::write(
            legacy_root.join("SKILL.md"),
            "---\nname: client\ndescription: Operate and understand the WeiAgent project.\ngroup: built-in\n---\ncustomized\n",
        )
        .await
        .unwrap();

        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store).await.unwrap();

        assert!(!legacy_root.exists());
        let migrated_path = data
            .path()
            .join("skills")
            .join(BUILTIN_WEIAGENT_CLI_SKILL_NAME)
            .join("SKILL.md");
        let migrated = fs::read_to_string(migrated_path).await.unwrap();
        assert!(migrated.contains("name: weiagentcli"));
        assert!(
            manager
                .settings(None)
                .await
                .unwrap()
                .defaults
                .iter()
                .any(|skill| skill.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME)
        );
    }

    #[tokio::test]
    async fn builtin_skill_group_is_protected() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let manager = SkillManager::new(store.clone()).await.unwrap();

        manager.create_group("custom").await.unwrap();
        let custom_group = store
            .list_skill_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|group| group.name == "custom")
            .unwrap();

        assert!(
            manager
                .rename_group(BUILTIN_SKILL_GROUP_ID, "Changed")
                .await
                .is_err()
        );
        assert!(manager.delete_group(BUILTIN_SKILL_GROUP_ID).await.is_err());
        assert!(
            manager
                .assign_skill_group("other-skill", Some(BUILTIN_SKILL_GROUP_ID))
                .await
                .is_err()
        );
        assert!(
            manager
                .assign_skill_group(BUILTIN_WEIAGENT_CLI_SKILL_NAME, None)
                .await
                .is_err()
        );
        assert!(
            manager
                .reorder_group_skills(
                    Some(&custom_group.id),
                    &[BUILTIN_WEIAGENT_CLI_SKILL_NAME.to_string()],
                )
                .await
                .is_err()
        );
        assert!(
            manager
                .reorder_group_skills(None, &[BUILTIN_WEIAGENT_CLI_SKILL_NAME.to_string()],)
                .await
                .is_err()
        );

        let settings = manager.settings(None).await.unwrap();
        assert!(
            settings
                .groups
                .iter()
                .any(|group| group.id == BUILTIN_SKILL_GROUP_ID && group.is_builtin)
        );
        assert!(
            settings
                .defaults
                .iter()
                .any(|skill| skill.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME && skill.is_builtin)
        );
    }

    #[tokio::test]
    async fn removing_skill_md_removes_skill_even_when_directory_remains() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let skill_root = data.path().join("skills").join("orphaned");
        write_skill(
            &data.path().join("skills"),
            "orphaned",
            "orphaned-skill",
            "development",
        )
        .await;
        fs::create_dir_all(skill_root.join("references"))
            .await
            .unwrap();
        fs::write(skill_root.join("references").join("notes.md"), "still here")
            .await
            .unwrap();
        fs::write(skill_root.join("asset.png"), b"asset")
            .await
            .unwrap();

        let manager = SkillManager::new(store).await.unwrap();
        assert!(
            manager
                .settings(None)
                .await
                .unwrap()
                .defaults
                .iter()
                .any(|skill| skill.name == "orphaned-skill")
        );

        fs::remove_file(skill_root.join("SKILL.md")).await.unwrap();
        manager.refresh().await.unwrap();

        let settings = manager.settings(None).await.unwrap();
        assert!(
            !settings
                .defaults
                .iter()
                .any(|skill| skill.name == "orphaned-skill")
        );
        assert!(skill_root.is_dir());
        assert!(skill_root.join("references").join("notes.md").is_file());
        assert!(skill_root.join("asset.png").is_file());
    }

    #[tokio::test]
    async fn override_precedence_and_orphans_are_reported() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        write_skill(
            &data.path().join("skills"),
            "first",
            "first-skill",
            "development",
        )
        .await;
        write_skill(
            &data.path().join("skills"),
            "second",
            "second-skill",
            "development",
        )
        .await;
        let agent_id = store.list_agents().await.unwrap()[0].id;
        let manager = SkillManager::new(store.clone()).await.unwrap();
        manager.create_group("development").await.unwrap();
        let development_group = store
            .list_skill_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|group| group.name == "development")
            .unwrap();
        manager
            .assign_skill_group("first-skill", Some(&development_group.id))
            .await
            .unwrap();
        manager
            .assign_skill_group("second-skill", Some(&development_group.id))
            .await
            .unwrap();
        manager.settings(Some(agent_id)).await.unwrap();

        store
            .update_skill_global_group_override("development", Some(false))
            .await
            .unwrap();
        store
            .update_skill_global_override("first-skill", Some(true))
            .await
            .unwrap();
        store
            .update_skill_agent_overrides(
                agent_id,
                SkillOverrides {
                    skills: vec![SkillOverride {
                        name: "first-skill".to_string(),
                        enabled: Some(false),
                    }],
                    groups: vec![SkillGroupOverride {
                        group: "development".to_string(),
                        group_id: None,
                        enabled: Some(true),
                    }],
                },
            )
            .await
            .unwrap();

        let settings = manager.settings(Some(agent_id)).await.unwrap();
        let first = settings
            .defaults
            .iter()
            .find(|item| item.name == "first-skill")
            .unwrap();
        let second = settings
            .defaults
            .iter()
            .find(|item| item.name == "second-skill")
            .unwrap();
        assert!(!first.enabled);
        assert!(second.enabled);

        store
            .update_skill_global_override("deleted-skill", Some(false))
            .await
            .unwrap();
        let settings = manager.settings(None).await.unwrap();
        assert_eq!(settings.orphan_skills, vec!["deleted-skill"]);
    }

    #[tokio::test]
    async fn agent_skill_defaults_are_builtin_inherited_and_user_disabled() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        write_skill(
            &data.path().join("skills"),
            "first",
            "first-skill",
            "development",
        )
        .await;
        let agent_id = store.list_agents().await.unwrap()[0].id;
        let manager = SkillManager::new(store.clone()).await.unwrap();

        let initial = manager.settings(Some(agent_id)).await.unwrap();
        let builtin = initial
            .defaults
            .iter()
            .find(|item| item.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME)
            .unwrap();
        assert!(builtin.is_builtin);
        assert!(builtin.agent_override.is_none());
        assert!(builtin.enabled);
        let first = initial
            .defaults
            .iter()
            .find(|item| item.name == "first-skill")
            .unwrap();
        assert_eq!(first.agent_override, Some(false));
        assert!(!first.enabled);

        write_skill(
            &data.path().join("skills"),
            "second",
            "second-skill",
            "development",
        )
        .await;
        manager.refresh().await.unwrap();

        let updated = manager.settings(Some(agent_id)).await.unwrap();
        let second = updated
            .defaults
            .iter()
            .find(|item| item.name == "second-skill")
            .unwrap();
        assert_eq!(second.agent_override, Some(false));
        assert!(!second.enabled);

        store
            .update_skill_agent_overrides(
                agent_id,
                SkillOverrides {
                    skills: Vec::new(),
                    groups: Vec::new(),
                },
            )
            .await
            .unwrap();
        let inherited = manager.settings(Some(agent_id)).await.unwrap();
        assert!(
            inherited
                .defaults
                .iter()
                .find(|item| item.name == "second-skill")
                .unwrap()
                .agent_override
                .is_none()
        );
    }

    #[tokio::test]
    async fn builtin_skill_agent_override_can_disable_but_global_override_is_protected() {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(data.path()).await.unwrap());
        let agent_id = store.list_agents().await.unwrap()[0].id;
        let manager = SkillManager::new(store.clone()).await.unwrap();

        store
            .update_skill_global_override(BUILTIN_WEIAGENT_CLI_SKILL_NAME, Some(false))
            .await
            .unwrap();
        store
            .update_skill_agent_overrides(
                agent_id,
                SkillOverrides {
                    skills: vec![SkillOverride {
                        name: BUILTIN_WEIAGENT_CLI_SKILL_NAME.to_string(),
                        enabled: Some(false),
                    }],
                    groups: Vec::new(),
                },
            )
            .await
            .unwrap();

        let settings = manager.settings(Some(agent_id)).await.unwrap();
        let builtin = settings
            .defaults
            .iter()
            .find(|item| item.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME)
            .unwrap();
        assert!(builtin.is_builtin);
        assert_eq!(builtin.agent_override, Some(false));
        assert!(builtin.global_override.is_none());
        assert!(!builtin.enabled);
        assert!(settings.overrides.skills.iter().any(|item| item.name
            == BUILTIN_WEIAGENT_CLI_SKILL_NAME
            && item.enabled == Some(false)));
        assert!(
            !store
                .read_skill_global_overrides()
                .await
                .unwrap()
                .skills
                .iter()
                .any(|item| item.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME)
        );
    }
}
