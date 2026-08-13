use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::{Mutex, RwLock};

use crate::{
    domain::{EnvironmentRuntimeStatus, RuntimeComponentKind, RuntimeSource},
    error::{AppError, AppResult},
};

use super::supporting_runtime;
use super::terminal::TerminalEnvironment;

#[derive(Clone, Copy)]
struct BundledTool {
    file_name: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/bundled_ripgrep.rs"));

/// Detects and manages optional local development tools used by terminal tasks.
///
/// This service deliberately has no required executable. The terminal uses the
/// host shell directly; these components only extend the developer environment.
pub struct EnvironmentRuntimeManager {
    runtimes_root: PathBuf,
    client: Client,
    status: RwLock<EnvironmentRuntimeStatus>,
    latest_check_lock: Mutex<()>,
    active_install: RwLock<Option<RuntimeComponentKind>>,
    bundled_ripgrep: OnceLock<Option<PathBuf>>,
}

impl EnvironmentRuntimeManager {
    pub fn new(data_dir: impl AsRef<Path>) -> AppResult<Self> {
        // Terminal child processes may change cwd before resolving PATH entries. Keep all
        // managed runtime directories absolute so relative dev-mode data paths remain usable.
        let data_dir = data_dir.as_ref();
        let data_dir = if data_dir.is_absolute() {
            data_dir.to_path_buf()
        } else {
            std::env::current_dir()?.join(data_dir)
        };
        let client = crate::http_client::build_reqwest_client(
            crate::http_client::HttpClientConfig::new()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(15 * 60))
                .user_agent(format!("WeiAgent/{}", env!("CARGO_PKG_VERSION"))),
        )
        .map_err(|error| AppError::Process(format!("failed to build runtime client: {error}")))?;
        Ok(Self {
            runtimes_root: data_dir.join("runtimes"),
            client,
            status: RwLock::new(empty_status()),
            latest_check_lock: Mutex::new(()),
            active_install: RwLock::new(None),
            bundled_ripgrep: OnceLock::new(),
        })
    }

    pub async fn initialize(self: &Arc<Self>) -> AppResult<EnvironmentRuntimeStatus> {
        tokio::fs::create_dir_all(&self.runtimes_root).await?;
        let status = self.refresh_local().await?;
        let manager = self.clone();
        tokio::spawn(async move {
            manager.refresh_all_components().await;
            let mut ticker = tokio::time::interval(supporting_runtime::latest_version_cache_ttl());
            ticker.tick().await;
            loop {
                ticker.tick().await;
                manager.refresh_all_components().await;
            }
        });
        Ok(status)
    }

    async fn refresh_all_components(&self) {
        for component in supported_components() {
            if let Err(error) = self.refresh_component(component).await {
                tracing::warn!(?component, %error, "failed to refresh environment component");
            }
        }
    }

    pub async fn status(&self) -> EnvironmentRuntimeStatus {
        self.status.read().await.clone()
    }

    pub async fn refresh_component(
        &self,
        component: RuntimeComponentKind,
    ) -> AppResult<EnvironmentRuntimeStatus> {
        let _guard = self.latest_check_lock.lock().await;
        let mut status = self.refresh_local().await?;
        let result = supporting_runtime::latest_component_version(
            component,
            &self.client,
            &self.runtimes_root,
        )
        .await;
        if let Some(component_status) = status
            .components
            .iter_mut()
            .find(|item| item.kind == component)
        {
            match result {
                Ok(target) => {
                    component_status.update_available = component_status
                        .detected_version
                        .as_deref()
                        .map(|current| current != target.as_str())
                        .unwrap_or(false);
                    component_status.target_version = Some(target);
                    component_status.error = None;
                }
                Err(error) => component_status.error = Some(error.to_string()),
            }
        }
        status.update_aggregate_fields();
        *self.status.write().await = status.clone();
        Ok(status)
    }

    pub async fn start_install(
        self: &Arc<Self>,
        component: RuntimeComponentKind,
    ) -> AppResult<EnvironmentRuntimeStatus> {
        {
            let mut active = self.active_install.write().await;
            if active.is_some() {
                return Err(AppError::Conflict(
                    "runtime installation is already in progress".to_string(),
                ));
            }
            *active = Some(component);
        }
        let manager = self.clone();
        tokio::spawn(async move {
            let result = supporting_runtime::install_component(
                component,
                &manager.runtimes_root,
                &manager.client,
            )
            .await;
            *manager.active_install.write().await = None;
            if let Err(error) = result {
                tracing::warn!(?component, %error, "environment component installation failed");
            }
            let _ = manager.refresh_local().await;
        });
        Ok(self.status().await)
    }

    async fn refresh_local(&self) -> AppResult<EnvironmentRuntimeStatus> {
        let active = *self.active_install.read().await;
        let previous = self.status.read().await.clone();
        let mut components =
            supporting_runtime::detect_components(&self.runtimes_root, active).await;
        for component in &mut components {
            component.target_version =
                supporting_runtime::cached_component_version(&self.runtimes_root, component.kind)
                    .await
                    .or_else(|| {
                        previous
                            .components
                            .iter()
                            .find(|item| item.kind == component.kind)
                            .and_then(|item| item.target_version.clone())
                    });
            component.update_available = component
                .detected_version
                .as_deref()
                .zip(component.target_version.as_deref())
                .map(|(current, target)| current != target)
                .unwrap_or(false);
        }
        let mut status = EnvironmentRuntimeStatus {
            supported: true,
            available: false,
            managed_installed: false,
            installing: active.is_some(),
            target_version: String::new(),
            update_available: false,
            platform: Some(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )),
            source: None,
            detected_version: None,
            executable: None,
            error: None,
            components,
        };
        status.update_aggregate_fields();
        *self.status.write().await = status.clone();
        Ok(status)
    }
}

impl EnvironmentRuntimeStatus {
    fn update_aggregate_fields(&mut self) {
        self.available = self.components.iter().any(|item| item.available);
        self.managed_installed = self.components.iter().any(|item| item.managed_installed);
        self.update_available = self.components.iter().any(|item| item.update_available);
        self.error = self.components.iter().find_map(|item| item.error.clone());
        self.source = self.components.iter().find_map(|item| item.source.clone());
    }
}

#[async_trait]
impl TerminalEnvironment for EnvironmentRuntimeManager {
    async fn managed_bin_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(path) = self
            .bundled_ripgrep
            .get_or_init(|| materialize_bundled_ripgrep(&self.runtimes_root))
            .clone()
            && let Some(parent) = path.parent()
        {
            paths.push(parent.to_path_buf());
        }
        paths.extend(supporting_runtime::managed_bin_paths(&self.runtimes_root).await);
        paths
    }
}

fn materialize_bundled_ripgrep(runtimes_root: &Path) -> Option<PathBuf> {
    let tool = BUNDLED_RIPGREP?;
    let tool_dir = runtimes_root.join("ripgrep");
    let tool_path = tool_dir.join(tool.file_name);

    let should_write = std::fs::read(&tool_path)
        .map(|existing| existing != tool.bytes)
        .unwrap_or(true);
    if should_write {
        std::fs::create_dir_all(&tool_dir).ok()?;
        std::fs::write(&tool_path, tool.bytes).ok()?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&tool_path).ok()?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool_path, permissions).ok()?;
    }

    Some(tool_path)
}

fn supported_components() -> [RuntimeComponentKind; 4] {
    [
        RuntimeComponentKind::Node,
        RuntimeComponentKind::Python,
        RuntimeComponentKind::Uv,
        RuntimeComponentKind::Git,
    ]
}

fn empty_status() -> EnvironmentRuntimeStatus {
    EnvironmentRuntimeStatus {
        supported: true,
        available: false,
        managed_installed: false,
        installing: false,
        target_version: String::new(),
        update_available: false,
        platform: Some(format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        source: Some(RuntimeSource::System),
        detected_version: None,
        executable: None,
        error: None,
        components: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_components_exclude_terminal_backend() {
        assert_eq!(supported_components().len(), 4);
        assert!(supported_components().contains(&RuntimeComponentKind::Node));
    }

    #[test]
    fn materializes_the_bundled_ripgrep_for_the_current_target() {
        let Some(tool) = BUNDLED_RIPGREP else {
            return;
        };
        let root = tempfile::tempdir().expect("create runtime temp directory");
        let path = materialize_bundled_ripgrep(root.path()).expect("materialize ripgrep");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(tool.file_name)
        );
        assert_eq!(
            std::fs::read(path).expect("read materialized ripgrep"),
            tool.bytes
        );
    }

    #[test]
    fn relative_data_directories_are_promoted_to_absolute_runtime_paths() {
        let manager = EnvironmentRuntimeManager::new("data").expect("create runtime manager");

        assert!(manager.runtimes_root.is_absolute());
    }
}
