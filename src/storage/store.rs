use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{
    fs,
    sync::{Mutex, RwLock},
};
use uuid::Uuid;

use super::providers;
use crate::{
    config::{load_or_init_settings, write_settings},
    domain::{Agent, AppSettings, ModelProvider},
    error::AppResult,
    storage::{
        agents::{read_agents_conn, write_agents_conn},
        db,
        files::read_json,
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
    pub(super) settings_lock: Mutex<()>,
    settings_snapshot: RwLock<AppSettings>,
    pub(super) agents_lock: Mutex<()>,
    pub(super) workspace_lock: Mutex<()>,
    pub(super) provider_lock: Mutex<()>,
    pub(super) ip_proxy_lock: Mutex<()>,
    pub(super) tool_settings_lock: Mutex<()>,
    pub(super) mcp_lock: Mutex<()>,
    pub(super) skill_lock: Mutex<()>,
}

impl Store {
    pub async fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let terminal_workspace_dir = crate::config::terminal_workspace_dir(&root);
        let attachments_dir = crate::config::attachments_dir(&root);
        let settings_file = root.join("settings.json");
        let agents_file = root.join("agents.json");
        let agents_dir = root.join("agents");
        crate::fsutil::ensure_private_dir(&root).await?;
        fs::create_dir_all(&terminal_workspace_dir).await?;
        fs::create_dir_all(&attachments_dir).await?;
        fs::create_dir_all(&agents_dir).await?;
        let initial_settings = load_or_init_settings(&settings_file).await?;
        let database = Arc::new(db::Database::open(&root)?);

        let store = Self {
            data_dir: root,
            terminal_workspace_dir,
            attachments_dir,
            settings_file,
            agents_file,
            agents_dir,
            database,
            settings_lock: Mutex::new(()),
            settings_snapshot: RwLock::new(initial_settings),
            agents_lock: Mutex::new(()),
            workspace_lock: Mutex::new(()),
            provider_lock: Mutex::new(()),
            ip_proxy_lock: Mutex::new(()),
            tool_settings_lock: Mutex::new(()),
            mcp_lock: Mutex::new(()),
            skill_lock: Mutex::new(()),
        };
        store.migrate_agents_from_json_if_needed().await?;
        store.initialize_tool_settings().await?;
        store.initialize_mcp_settings().await?;
        store.initialize_skill_settings().await?;
        store.ensure_builtin_agents().await?;
        store.recover_interrupted_executions().await?;
        Ok(store)
    }

    pub async fn read_settings(&self) -> AppResult<AppSettings> {
        Ok(self.settings_snapshot.read().await.clone())
    }

    pub async fn write_settings(&self, settings: AppSettings) -> AppResult<AppSettings> {
        let _guard = self.settings_lock.lock().await;
        write_settings(&self.settings_file, &settings).await?;
        *self.settings_snapshot.write().await = settings.clone();
        Ok(settings)
    }

    pub(super) async fn read_providers(&self) -> AppResult<Vec<ModelProvider>> {
        let _guard = self.provider_lock.lock().await;
        self.database
            .read(|connection| providers::read_providers_conn(connection))
            .await
    }

    pub fn database(&self) -> Arc<db::Database> {
        self.database.clone()
    }

    pub(super) async fn read_agents(&self) -> AppResult<Vec<Agent>> {
        let _guard = self.agents_lock.lock().await;
        self.read_agents_unlocked().await
    }

    pub(super) async fn read_agents_unlocked(&self) -> AppResult<Vec<Agent>> {
        self.database
            .read(|connection| read_agents_conn(connection))
            .await
    }

    pub(super) async fn write_agents_unlocked(&self, agents: &[Agent]) -> AppResult<()> {
        let agents = agents.to_vec();
        self.database
            .call(move |connection| write_agents_conn(connection, &agents))
            .await
    }

    async fn migrate_agents_from_json_if_needed(&self) -> AppResult<()> {
        if fs::metadata(&self.agents_file).await.is_err() {
            return Ok(());
        }
        let imported: Vec<Agent> = match read_json(&self.agents_file).await {
            Ok(agents) => agents,
            Err(error) => {
                tracing::warn!(
                    path = %self.agents_file.display(),
                    %error,
                    "failed to read legacy agents.json; leaving database agents unchanged"
                );
                return Ok(());
            }
        };
        let existing = self.read_agents_unlocked().await?;
        if existing.is_empty() && !imported.is_empty() {
            self.write_agents_unlocked(&imported).await?;
        }
        let backup = self.agents_file.with_extension("json.bak");
        if let Err(error) = fs::rename(&self.agents_file, &backup).await {
            tracing::warn!(
                path = %self.agents_file.display(),
                %error,
                "failed to archive legacy agents.json after database import"
            );
        }
        Ok(())
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
