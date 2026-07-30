use std::{
    path::{Path, PathBuf},
    sync::Arc,
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
}

impl EnvironmentRuntimeManager {
    pub fn new(data_dir: impl AsRef<Path>) -> AppResult<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(15 * 60))
            .user_agent(format!("WeiAgent/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                AppError::Process(format!("failed to build runtime client: {error}"))
            })?;
        Ok(Self {
            runtimes_root: data_dir.as_ref().join("runtimes"),
            client,
            status: RwLock::new(empty_status()),
            latest_check_lock: Mutex::new(()),
            active_install: RwLock::new(None),
        })
    }

    pub async fn initialize(self: &Arc<Self>) -> AppResult<EnvironmentRuntimeStatus> {
        tokio::fs::create_dir_all(&self.runtimes_root).await?;
        let status = self.refresh_local().await?;
        let manager = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(supporting_runtime::latest_version_cache_ttl());
            ticker.tick().await;
            loop {
                ticker.tick().await;
                for component in supported_components() {
                    if let Err(error) = manager.refresh_component(component).await {
                        tracing::warn!(?component, %error, "failed to refresh environment component");
                    }
                }
            }
        });
        Ok(status)
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
                        .map(|current| current != target)
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
        let components = supporting_runtime::detect_components(&self.runtimes_root, active).await;
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
        supporting_runtime::managed_bin_paths(&self.runtimes_root).await
    }
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
}
