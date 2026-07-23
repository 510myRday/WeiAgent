use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use crate::{
    config::{load_or_init_settings, write_settings},
    domain::{Agent, AppSettings, ChatMessage, Conversation, ModelProvider},
    error::AppResult,
    storage::{
        db,
        files::{ensure_json_array, read_json, write_json},
    },
};

pub struct JsonStore {
    pub data_dir: PathBuf,
    pub terminal_workspace_dir: PathBuf,
    pub attachments_dir: PathBuf,
    pub(super) settings_file: PathBuf,
    pub(super) agents_file: PathBuf,
    pub(super) agents_dir: PathBuf,
    pub(super) conversations_file: PathBuf,
    pub(super) conversations_dir: PathBuf,
    pub(super) database: Arc<db::Database>,
    pub(super) lock: Mutex<()>,
}

impl JsonStore {
    pub async fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let terminal_workspace_dir = crate::config::terminal_workspace_dir(&root);
        let attachments_dir = crate::config::attachments_dir(&root);
        let settings_file = root.join("settings.json");
        let agents_file = root.join("agents.json");
        let agents_dir = root.join("agents");
        let conversations_file = root.join("conversations.json");
        let conversations_dir = root.join("conversations");
        fs::create_dir_all(&root).await?;
        fs::create_dir_all(&terminal_workspace_dir).await?;
        fs::create_dir_all(&attachments_dir).await?;
        fs::create_dir_all(&agents_dir).await?;
        fs::create_dir_all(&conversations_dir).await?;
        ensure_json_array(&agents_file).await?;
        ensure_json_array(&conversations_file).await?;
        load_or_init_settings(&settings_file).await?;
        let database = Arc::new(db::Database::open(&root)?);

        let store = Self {
            data_dir: root,
            terminal_workspace_dir,
            attachments_dir,
            settings_file,
            agents_file,
            agents_dir,
            conversations_file,
            conversations_dir,
            database,
            lock: Mutex::new(()),
        };
        store.initialize_tool_settings().await?;
        store.ensure_builtin_agents().await?;
        store.repair_conversation_metadata().await?;
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
        self.read_providers_unlocked().await
    }

    pub(super) async fn read_providers_unlocked(&self) -> AppResult<Vec<ModelProvider>> {
        self.database
            .call(|connection| db::read_providers_conn(connection))
            .await
    }

    pub(super) async fn upsert_provider_unlocked(&self, provider: ModelProvider) -> AppResult<()> {
        self.database
            .call(move |connection| db::upsert_provider_conn(connection, &provider))
            .await
    }

    pub(super) async fn delete_provider_unlocked(&self, id: Uuid) -> AppResult<bool> {
        self.database
            .call(move |connection| db::delete_provider_conn(connection, id))
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

    pub(super) async fn read_conversations(&self) -> AppResult<Vec<Conversation>> {
        let _guard = self.lock.lock().await;
        self.read_conversations_unlocked().await
    }

    pub(super) async fn read_conversations_unlocked(&self) -> AppResult<Vec<Conversation>> {
        read_json(&self.conversations_file).await
    }

    pub(super) async fn write_conversations_unlocked(
        &self,
        conversations: &[Conversation],
    ) -> AppResult<()> {
        write_json(&self.conversations_file, conversations).await
    }

    pub(super) async fn read_conversation_messages_unchecked(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Vec<ChatMessage>> {
        let file = self.conversation_file(conversation_id);
        if fs::metadata(&file).await.is_err() {
            return Ok(Vec::new());
        }
        read_json(&file).await
    }

    pub(super) fn conversation_file(&self, conversation_id: Uuid) -> PathBuf {
        self.conversations_dir
            .join(format!("{conversation_id}.json"))
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
        let store = JsonStore::new(root.path()).await.unwrap();

        assert_eq!(
            store.terminal_workspace_dir,
            root.path().join(crate::config::TERMINAL_WORKSPACE_DIR_NAME)
        );
        assert!(store.terminal_workspace_dir.is_dir());
    }
}
