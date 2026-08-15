use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime},
};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{StreamExt, stream::FuturesUnordered};
use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs, sync::RwLock};
use uuid::Uuid;

use crate::{
    domain::{
        SkillChangeEvent, SkillChangeFeed, SkillConflict, SkillContent, SkillDiagnostic, SkillFile,
        SkillGroupState, SkillImportTask, SkillManifest, SkillOverrides, SkillSettings, ToolResult,
    },
    error::{AppError, AppResult},
    fsutil,
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::Store,
    tools::registry::{AgentTool, ToolExecutionContext},
};

use super::{
    parser::{discover_all_files, parse_skill},
    path_policy::{MAX_SKILL_FILE_BYTES, relative_file},
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

pub const SKILLS_LIST_TOOL_NAME: &str = "skills_list";
pub const SKILL_VIEW_TOOL_NAME: &str = "skill_view";

const MAX_REMOTE_SKILL_FILES: usize = 256;
const MAX_REMOTE_SKILL_BYTES: usize = 32 * 1024 * 1024;
const REMOTE_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(8);
const SKILL_RESOURCE_DIRECTORIES: [&str; 5] =
    ["references", "scripts", "examples", "templates", "assets"];

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

    pub fn start_monitor(self: Arc<Self>) {
        tokio::spawn(async move {
            self.monitor_loop().await;
        });
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
                    stack.push(path);
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
        let mut stack = vec![self.root.clone()];
        while let Some(current) = stack.pop() {
            let mut entries = match fs::read_dir(&current).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).await?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    stack.push(path);
                } else if metadata.is_file() && entry.file_name().eq_ignore_ascii_case("SKILL.md") {
                    candidates.push(path.parent().unwrap_or(&self.root).to_path_buf());
                }
            }
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

    pub async fn create_import_task(
        &self,
        source: &str,
        kind: &str,
        group_id: Option<String>,
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
            error: None,
            created_at: now.clone(),
            updated_at: now,
            finished_at: None,
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

    pub async fn list_import_tasks(&self) -> Vec<SkillImportTask> {
        let tasks = self.import_tasks.read().await;
        let mut result = tasks.values().cloned().collect::<Vec<_>>();
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        result
    }

    pub async fn run_import_task(
        &self,
        task_id: Uuid,
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
            )
            .await
        } else {
            self.import_directory_with_progress(&source, Some(task_id), group_id.as_deref())
                .await
        };
        match result {
            Ok(()) => {
                self.import_tasks.write().await.remove(&task_id);
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

    async fn import_directory_with_progress(
        &self,
        source_directory: &str,
        task_id: Option<Uuid>,
        group_id: Option<&str>,
    ) -> AppResult<()> {
        let source = source_directory.trim();
        if source.is_empty() {
            return Err(AppError::BadRequest(
                "Skill directory is required".to_string(),
            ));
        }

        let source = validate_local_skill_directory(source).await?;
        if let Some(task_id) = task_id {
            self.update_import_task(task_id, "running", "copying", 25, None, false)
                .await;
        }

        let temporary = self
            .root
            .join(format!(".import-{}", Uuid::new_v4().simple()));
        if let Err(error) = copy_skill_directory(&source, &temporary).await {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(error);
        }
        let result = self.finish_import(temporary.clone(), None, group_id).await;
        if result.is_err() {
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
    ) -> AppResult<()> {
        let source = source.trim();
        if source.is_empty() {
            return Err(AppError::BadRequest(
                "Skill path or URL is required".to_string(),
            ));
        }
        if !looks_like_url(source) {
            return self
                .import_directory_with_progress(source, task_id, group_id)
                .await;
        }

        let parsed = normalize_remote_source(source)?;
        let cdn = github_jsdelivr_source(&parsed)?;
        if let Some(task_id) = task_id {
            self.update_import_task(task_id, "running", "connecting", 20, None, false)
                .await;
        }
        let selected = select_remote_route(parsed, cdn, proxy_url).await?;
        let temporary = self
            .root
            .join(format!(".import-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temporary).await?;

        let result = async {
            let skill_md = selected.skill_md;
            validate_skill_markdown_text(&skill_md)?;
            if let Some(task_id) = task_id {
                self.update_import_task(task_id, "running", "downloading", 45, None, false)
                    .await;
            }
            fs::write(temporary.join("SKILL.md"), skill_md.as_bytes()).await?;

            let mut downloaded = 1usize;
            let mut downloaded_bytes = skill_md.len();
            let referenced = referenced_skill_files(&skill_md);

            if let Some(github) = selected.github.as_ref() {
                if let Ok(tree) = fetch_github_tree(&selected.client, github).await {
                    let files =
                        select_github_skill_files(&tree, &github.skill_directory, &referenced);
                    for path in files {
                        if downloaded >= MAX_REMOTE_SKILL_FILES {
                            return Err(AppError::BadRequest(
                                "remote Skill contains too many files".to_string(),
                            ));
                        }
                        let relative = path
                            .strip_prefix(&format!("{}/", github.skill_directory))
                            .unwrap_or(&path)
                            .to_string();
                        if relative.eq_ignore_ascii_case("SKILL.md")
                            || !valid_download_path(&relative)
                        {
                            continue;
                        }
                        let file_url = selected
                            .source
                            .skill_md_url
                            .join(relative.as_str())
                            .map_err(|error| {
                                AppError::BadRequest(format!(
                                    "invalid Skill resource path: {error}"
                                ))
                            })?;
                        let bytes = download_bytes(&selected.client, &file_url).await?;
                        downloaded_bytes = downloaded_bytes.saturating_add(bytes.len());
                        if downloaded_bytes > MAX_REMOTE_SKILL_BYTES {
                            return Err(AppError::BadRequest(
                                "remote Skill is too large".to_string(),
                            ));
                        }
                        let destination = temporary.join(&relative);
                        if let Some(parent) = destination.parent() {
                            fs::create_dir_all(parent).await?;
                        }
                        fs::write(destination, &bytes).await?;
                        downloaded += 1;
                    }
                } else {
                    download_referenced_files(
                        &selected.client,
                        &temporary,
                        &selected.source.skill_md_url,
                        &referenced,
                        &mut downloaded,
                        &mut downloaded_bytes,
                    )
                    .await?;
                }
            } else {
                download_referenced_files(
                    &selected.client,
                    &temporary,
                    &selected.source.skill_md_url,
                    &referenced,
                    &mut downloaded,
                    &mut downloaded_bytes,
                )
                .await?;
            }

            if let Some(task_id) = task_id {
                self.update_import_task(task_id, "running", "installing", 85, None, false)
                    .await;
            }
            self.finish_import(temporary.clone(), Some(source), group_id)
                .await
        }
        .await;

        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary).await;
        }
        result
    }

    async fn finish_import(
        &self,
        temporary: PathBuf,
        source_url: Option<&str>,
        group_id: Option<&str>,
    ) -> AppResult<()> {
        let manifest = parse_skill(temporary.clone()).await?;
        if manifest.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME {
            let _ = fs::remove_dir_all(&temporary).await;
            return Err(AppError::BadRequest(
                "the built-in weiagentcli Skill cannot be replaced by an import".to_string(),
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
        self.store
            .update_skill_source(&manifest.name, source_url.map(str::to_string))
            .await?;
        self.store.set_skill_group(&manifest.name, group_id).await?;
        self.refresh().await
    }

    pub async fn settings(&self, agent_id: Option<Uuid>) -> AppResult<SkillSettings> {
        let mut global = self.store.read_skill_global_overrides().await?;
        let icons = self.store.read_skill_icons().await?;
        let sources = self.store.read_skill_sources().await?;
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
        let catalog = self.catalog.read().await;
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
        for manifest in &catalog.manifests {
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
            item.icon_url = icons.get(&manifest.name).cloned();
            item.source_url = sources.get(&manifest.name).cloned();
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
        let mut diagnostics = catalog.diagnostics.clone();
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
        self.store.delete_skill_metadata(&name).await?;
        self.refresh().await
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
        let files = discover_all_files(&root)
            .await?
            .into_iter()
            .map(|(file, metadata)| SkillFile {
                path: file
                    .strip_prefix(&root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/"),
                kind: Self::file_kind(&file, &root),
                size_bytes: metadata.len(),
            })
            .collect::<Vec<_>>();
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
            ToolDefinition { kind: "function".to_string(), function: FunctionDefinition { name: SKILLS_LIST_TOOL_NAME.to_string(), description: "List the enabled Skills available to the current Agent. Use this before loading a relevant Skill.".to_string(), parameters: serde_json::json!({"type":"object","properties":{"query":{"type":"string"},"group":{"type":"string"}},"additionalProperties":false}) } },
            ToolDefinition { kind: "function".to_string(), function: FunctionDefinition { name: SKILL_VIEW_TOOL_NAME.to_string(), description: "Load the SKILL.md or a reference file for an enabled Skill. Paths are relative to the Skill directory.".to_string(), parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string"},"file_path":{"type":"string"}},"required":["name"],"additionalProperties":false}) } },
        ]
    }

    pub async fn execute_tool_with_snapshot(
        &self,
        snapshot: &[SkillManifest],
        name: &str,
        arguments: Value,
    ) -> ToolResult {
        match name {
            SKILLS_LIST_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Args {
                    query: Option<String>,
                    group: Option<String>,
                }
                let args = match serde_json::from_value::<Args>(arguments) {
                    Ok(value) => value,
                    Err(error) => {
                        return ToolResult::Error {
                            message: error.to_string(),
                        };
                    }
                };
                let query = args.query.unwrap_or_default().to_ascii_lowercase();
                let items = snapshot
                    .iter()
                    .filter(|item| {
                        query.is_empty()
                            || item.name.contains(&query)
                            || item.description.to_ascii_lowercase().contains(&query)
                            || item
                                .tags
                                .iter()
                                .any(|tag| tag.to_ascii_lowercase().contains(&query))
                    })
                    .filter(|item| {
                        args.group
                            .as_deref()
                            .is_none_or(|group| item.group.as_deref() == Some(group))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                ToolResult::Completed {
                    stdout: serde_json::to_string_pretty(&items).unwrap_or_default(),
                    stderr: String::new(),
                    exit_code: 0,
                    duration_ms: 0,
                    runtime: Some("skill".to_string()),
                    output_cursor: 0,
                    structured_content: Some(serde_json::to_value(items).unwrap_or(Value::Null)),
                    is_error: false,
                }
            }
            SKILL_VIEW_TOOL_NAME => {
                #[derive(Deserialize)]
                struct Args {
                    name: String,
                    file_path: Option<String>,
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
                    Ok(content) => ToolResult::Completed {
                        stdout: content.content.clone(),
                        stderr: String::new(),
                        exit_code: 0,
                        duration_ms: 0,
                        runtime: Some("skill".to_string()),
                        output_cursor: 0,
                        structured_content: serde_json::to_value(content).ok(),
                        is_error: false,
                    },
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

struct SelectedRemoteRoute {
    source: RemoteSkillSource,
    github: Option<GithubSkillSource>,
    client: reqwest::Client,
    skill_md: String,
}

struct RemoteProbeResult {
    source: RemoteSkillSource,
    proxy_url: Option<String>,
    elapsed: Duration,
    skill_md: AppResult<String>,
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
    let mut candidates = Vec::new();
    candidates.push((source, None));
    if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        candidates.push((candidates[0].0.clone(), Some(proxy_url.to_string())));
    }
    if let Some(cdn) = cdn {
        candidates.push((cdn, None));
        if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
            let cdn = candidates
                .last()
                .map(|item| item.0.clone())
                .ok_or_else(|| {
                    AppError::Process("failed to prepare Skill CDN route".to_string())
                })?;
            candidates.push((cdn, Some(proxy_url.to_string())));
        }
    }
    let mut probes = FuturesUnordered::new();
    for (source, proxy_url) in candidates {
        let probe_client = build_remote_client(proxy_url.as_deref(), REMOTE_PREFLIGHT_TIMEOUT)?;
        probes.push(async move {
            let started = Instant::now();
            let skill_md = download_text(&probe_client, &source.skill_md_url).await;
            RemoteProbeResult {
                source,
                proxy_url,
                elapsed: started.elapsed(),
                skill_md,
            }
        });
    }

    let mut successful = Vec::new();
    while let Some(result) = probes.next().await {
        if let Ok(skill_md) = result.skill_md {
            if validate_skill_markdown_text(&skill_md).is_ok() {
                successful.push((result.elapsed, result.source, result.proxy_url, skill_md));
            }
        }
    }

    if let Some((_, source, proxy_url, skill_md)) = successful
        .into_iter()
        .min_by_key(|(elapsed, _, _, _)| *elapsed)
    {
        let client = build_remote_client(proxy_url.as_deref(), Duration::from_secs(30))?;
        let github = source.github.clone();
        return Ok(SelectedRemoteRoute {
            source,
            github,
            client,
            skill_md,
        });
    }

    Err(AppError::BadRequest(format!(
        "could not reach a valid Skill source within {} seconds; check the URL or proxy",
        REMOTE_PREFLIGHT_TIMEOUT.as_secs()
    )))
}

async fn download_bytes(client: &reqwest::Client, url: &Url) -> AppResult<Vec<u8>> {
    let response = client.get(url.clone()).send().await.map_err(|error| {
        AppError::Process(format!("failed to download Skill resource: {error}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::BadRequest(format!(
            "Skill resource returned HTTP {}: {}",
            status.as_u16(),
            url
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SKILL_FILE_BYTES)
    {
        return Err(AppError::BadRequest(format!(
            "Skill resource exceeds the {MAX_SKILL_FILE_BYTES} byte limit"
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| AppError::Process(format!("failed to read Skill resource: {error}")))?;
    if bytes.len() as u64 > MAX_SKILL_FILE_BYTES {
        return Err(AppError::BadRequest(format!(
            "Skill resource exceeds the {MAX_SKILL_FILE_BYTES} byte limit"
        )));
    }
    Ok(bytes.to_vec())
}

async fn download_text(client: &reqwest::Client, url: &Url) -> AppResult<String> {
    let bytes = download_bytes(client, url).await?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("Skill resource is not valid UTF-8".to_string()))
}

fn validate_skill_markdown_text(content: &str) -> AppResult<()> {
    let trimmed = content.trim_start_matches('\u{feff}').trim_start();
    if trimmed.is_empty() || !trimmed.contains("name:") || !trimmed.contains("description:") {
        return Err(AppError::BadRequest(
            "downloaded file is not a valid SKILL.md with name and description".to_string(),
        ));
    }
    if trimmed.contains("<html") || trimmed.contains("<!doctype html") {
        return Err(AppError::BadRequest(
            "downloaded Skill URL returned an HTML page instead of SKILL.md".to_string(),
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

fn select_github_skill_files(
    tree: &[GithubTreeEntry],
    skill_directory: &str,
    referenced: &HashSet<String>,
) -> Vec<String> {
    let prefix = format!("{}/", skill_directory.trim_matches('/'));
    let mut files = tree
        .iter()
        .filter(|entry| entry.entry_type == "blob" && entry.path.starts_with(&prefix))
        .filter_map(|entry| {
            let relative = entry.path.strip_prefix(&prefix)?.to_string();
            let first = relative.split('/').next().unwrap_or_default();
            let in_standard_directory = SKILL_RESOURCE_DIRECTORIES
                .iter()
                .any(|directory| first == *directory);
            (in_standard_directory
                || referenced.contains(&relative)
                || relative.eq_ignore_ascii_case("SKILL.md"))
            .then_some(entry.path.clone())
        })
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files
}

async fn download_referenced_files(
    client: &reqwest::Client,
    temporary: &Path,
    skill_md_url: &Url,
    referenced: &HashSet<String>,
    downloaded: &mut usize,
    downloaded_bytes: &mut usize,
) -> AppResult<()> {
    for relative in referenced {
        if *downloaded >= MAX_REMOTE_SKILL_FILES {
            return Err(AppError::BadRequest(
                "remote Skill contains too many files".to_string(),
            ));
        }
        if !valid_download_path(relative) || relative.eq_ignore_ascii_case("SKILL.md") {
            continue;
        }
        let file_url = skill_md_url.join(relative).map_err(|error| {
            AppError::BadRequest(format!("invalid referenced Skill path: {error}"))
        })?;
        let bytes = download_bytes(client, &file_url).await?;
        *downloaded_bytes = downloaded_bytes.saturating_add(bytes.len());
        if *downloaded_bytes > MAX_REMOTE_SKILL_BYTES {
            return Err(AppError::BadRequest(
                "remote Skill is too large".to_string(),
            ));
        }
        let destination = temporary.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(destination, &bytes).await?;
        *downloaded += 1;
    }
    Ok(())
}

fn referenced_skill_files(content: &str) -> HashSet<String> {
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

fn add_referenced_path(result: &mut HashSet<String>, value: &str) {
    let value = value.split(['#', '?']).next().unwrap_or_default().trim();
    if value.is_empty()
        || value.contains("://")
        || value.starts_with('#')
        || !valid_download_path(value)
    {
        return;
    }
    let normalized = value.trim_start_matches("./").replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or_default();
    if SKILL_RESOURCE_DIRECTORIES
        .iter()
        .any(|directory| first == *directory)
    {
        result.insert(normalized);
    }
}

fn valid_download_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\0')
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

#[async_trait]
impl AgentTool for SkillManager {
    fn definition(&self, _config: &crate::domain::AgentToolsConfig) -> ToolDefinition {
        self.tool_definitions().into_iter().next().unwrap()
    }
    fn is_enabled(&self, _config: &crate::domain::AgentToolsConfig) -> bool {
        true
    }
    async fn execute(&self, _arguments: Value, _context: ToolExecutionContext) -> ToolResult {
        ToolResult::Error {
            message: "Skill tools require the SkillManager execution route".to_string(),
        }
    }
}

pub fn is_skill_tool(name: &str) -> bool {
    matches!(name, SKILLS_LIST_TOOL_NAME | SKILL_VIEW_TOOL_NAME)
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
    fn extracts_only_supported_relative_skill_resources() {
        let files = referenced_skill_files(
            "See [API](references/api.md), `scripts/check.ps1`, and https://example.test/nope.",
        );
        assert!(files.contains("references/api.md"));
        assert!(files.contains("scripts/check.ps1"));
        assert!(!files.iter().any(|file| file.contains("example.test")));
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
        assert!(
            settings
                .overrides
                .skills
                .iter()
                .any(|item| item.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME && item.enabled == Some(false))
        );
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
