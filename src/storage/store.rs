use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use crate::{
    config::{load_or_init_settings, write_settings},
    domain::{Agent, AppSettings, ModelProvider},
    error::AppResult,
    storage::{
        db,
        files::{ensure_json_array, read_json, write_json},
    },
};

pub struct Store {
    pub data_dir: PathBuf,
    pub terminal_workspace_dir: PathBuf,
    pub attachments_dir: PathBuf,
    pub(super) settings_file: PathBuf,
    pub(super) agents_file: PathBuf,
    pub(super) agents_dir: PathBuf,
    pub(super) database: Arc<db::Database>,
    pub(super) lock: Mutex<()>,
}

impl Store {
    pub async fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let terminal_workspace_dir = crate::config::terminal_workspace_dir(&root);
        let attachments_dir = crate::config::attachments_dir(&root);
        let settings_file = root.join("settings.json");
        let agents_file = root.join("agents.json");
        let agents_dir = root.join("agents");
        fs::create_dir_all(&root).await?;
        fs::create_dir_all(&terminal_workspace_dir).await?;
        fs::create_dir_all(&attachments_dir).await?;
        fs::create_dir_all(&agents_dir).await?;
        ensure_json_array(&agents_file).await?;
        load_or_init_settings(&settings_file).await?;
        let database = Arc::new(db::Database::open(&root)?);

        let store = Self {
            data_dir: root,
            terminal_workspace_dir,
            attachments_dir,
            settings_file,
            agents_file,
            agents_dir,
            database,
            lock: Mutex::new(()),
        };
        store.initialize_tool_settings().await?;
        store.ensure_builtin_agents().await?;
        Ok(store)
    }

    pub async fn read_settings(&self) -> AppResult<AppSettings> {
        let _guard = self.lock.lock().await;
        crate::config::read_settings(&self.settings_file).await
    }

    pub async fn write_settings(&self, settings: AppSettings) -> AppResult<AppSettings> {
        let _guard = self.lock.lock().await;
        write_settings(&self.settings_file, &settings).await?;
        Ok(settings)
    }

    pub(super) async fn read_providers(&self) -> AppResult<Vec<ModelProvider>> {
        let _guard = self.lock.lock().await;
        self.database
            .call(|connection| db::read_providers_conn(connection))
            .await
    }

    pub fn database(&self) -> Arc<db::Database> {
        self.database.clone()
    }

    pub(super) async fn read_agents(&self) -> AppResult<Vec<Agent>> {
        let _guard = self.lock.lock().await;
        self.read_agents_unlocked().await
    }

    pub(super) async fn read_agents_unlocked(&self) -> AppResult<Vec<Agent>> {
        read_json(&self.agents_file).await
    }

    pub(super) async fn write_agents_unlocked(&self, agents: &[Agent]) -> AppResult<()> {
        write_json(&self.agents_file, agents).await
    }

    pub(super) fn agent_workspace_dir(&self, agent_id: Uuid) -> PathBuf {
        self.agents_dir.join(agent_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_dedicated_terminal_workspace() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path()).await.unwrap();

        assert_eq!(
            store.terminal_workspace_dir,
            root.path().join(crate::config::TERMINAL_WORKSPACE_DIR_NAME)
        );
        assert!(store.terminal_workspace_dir.is_dir());
    }
}
