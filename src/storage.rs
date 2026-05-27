use std::path::{Path, PathBuf};

use chrono::Utc;
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{Agent, ChatMessage, CreateAgentRequest, UpdateAgentRequest},
};

pub struct JsonStore {
    agents_file: PathBuf,
    chats_dir: PathBuf,
    lock: Mutex<()>,
}

impl JsonStore {
    pub async fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let chats_dir = root.join("chats");
        let agents_file = root.join("agents.json");

        fs::create_dir_all(&chats_dir).await?;
        if fs::metadata(&agents_file).await.is_err() {
            fs::write(&agents_file, "[]").await?;
        }

        Ok(Self {
            agents_file,
            chats_dir,
            lock: Mutex::new(()),
        })
    }

    pub async fn list_agents(&self) -> AppResult<Vec<Agent>> {
        self.read_agents().await
    }

    pub async fn get_agent(&self, id: Uuid) -> AppResult<Agent> {
        self.read_agents()
            .await?
            .into_iter()
            .find(|agent| agent.id == id)
            .ok_or(AppError::NotFound)
    }

    pub async fn create_agent(&self, request: CreateAgentRequest) -> AppResult<Agent> {
        validate_agent_fields(&request.name, &request.system_prompt, &request.model)?;

        let _guard = self.lock.lock().await;
        let mut agents = self.read_agents_unlocked().await?;
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: request.name.trim().to_string(),
            system_prompt: request.system_prompt.trim().to_string(),
            model: request.model.trim().to_string(),
            created_at: now,
            updated_at: now,
        };
        agents.push(agent.clone());
        self.write_agents_unlocked(&agents).await?;
        Ok(agent)
    }

    pub async fn update_agent(&self, id: Uuid, request: UpdateAgentRequest) -> AppResult<Agent> {
        let _guard = self.lock.lock().await;
        let mut agents = self.read_agents_unlocked().await?;
        let agent = agents
            .iter_mut()
            .find(|agent| agent.id == id)
            .ok_or(AppError::NotFound)?;

        if let Some(name) = request.name {
            if name.trim().is_empty() {
                return Err(AppError::BadRequest(
                    "agent name cannot be empty".to_string(),
                ));
            }
            agent.name = name.trim().to_string();
        }

        if let Some(system_prompt) = request.system_prompt {
            if system_prompt.trim().is_empty() {
                return Err(AppError::BadRequest(
                    "system prompt cannot be empty".to_string(),
                ));
            }
            agent.system_prompt = system_prompt.trim().to_string();
        }

        if let Some(model) = request.model {
            if model.trim().is_empty() {
                return Err(AppError::BadRequest("model cannot be empty".to_string()));
            }
            agent.model = model.trim().to_string();
        }

        agent.updated_at = Utc::now();
        let updated = agent.clone();
        self.write_agents_unlocked(&agents).await?;
        Ok(updated)
    }

    pub async fn delete_agent(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut agents = self.read_agents_unlocked().await?;
        let before = agents.len();
        agents.retain(|agent| agent.id != id);
        if agents.len() == before {
            return Err(AppError::NotFound);
        }
        self.write_agents_unlocked(&agents).await?;

        let chat_file = self.chat_file(id);
        if fs::metadata(&chat_file).await.is_ok() {
            fs::remove_file(chat_file).await?;
        }
        Ok(())
    }

    pub async fn read_chat(&self, agent_id: Uuid) -> AppResult<Vec<ChatMessage>> {
        let chat_file = self.chat_file(agent_id);
        if fs::metadata(&chat_file).await.is_err() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(chat_file).await?;
        Ok(serde_json::from_str(&content)?)
    }

    pub async fn append_chat_messages(
        &self,
        agent_id: Uuid,
        messages: &[ChatMessage],
    ) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut existing = self.read_chat_unlocked(agent_id).await?;
        existing.extend_from_slice(messages);
        let content = serde_json::to_string_pretty(&existing)?;
        fs::write(self.chat_file(agent_id), content).await?;
        Ok(())
    }

    async fn read_agents(&self) -> AppResult<Vec<Agent>> {
        let _guard = self.lock.lock().await;
        self.read_agents_unlocked().await
    }

    async fn read_agents_unlocked(&self) -> AppResult<Vec<Agent>> {
        let content = fs::read_to_string(&self.agents_file).await?;
        Ok(serde_json::from_str(&content)?)
    }

    async fn write_agents_unlocked(&self, agents: &[Agent]) -> AppResult<()> {
        let content = serde_json::to_string_pretty(agents)?;
        fs::write(&self.agents_file, content).await?;
        Ok(())
    }

    async fn read_chat_unlocked(&self, agent_id: Uuid) -> AppResult<Vec<ChatMessage>> {
        let chat_file = self.chat_file(agent_id);
        if fs::metadata(&chat_file).await.is_err() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(chat_file).await?;
        Ok(serde_json::from_str(&content)?)
    }

    fn chat_file(&self, agent_id: Uuid) -> PathBuf {
        self.chats_dir.join(format!("{agent_id}.json"))
    }
}

fn validate_agent_fields(name: &str, system_prompt: &str, model: &str) -> AppResult<()> {
    if name.trim().is_empty() {
        return Err(AppError::BadRequest(
            "agent name cannot be empty".to_string(),
        ));
    }
    if system_prompt.trim().is_empty() {
        return Err(AppError::BadRequest(
            "system prompt cannot be empty".to_string(),
        ));
    }
    if model.trim().is_empty() {
        return Err(AppError::BadRequest("model cannot be empty".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ChatRole;

    #[tokio::test]
    async fn stores_agents_and_chat_messages_as_json() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Planner".to_string(),
                system_prompt: "Plan work clearly.".to_string(),
                model: "gpt-4o-mini".to_string(),
            })
            .await
            .unwrap();

        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "Planner");

        let message = ChatMessage {
            role: ChatRole::User,
            content: "hello".to_string(),
            created_at: Utc::now(),
        };
        store
            .append_chat_messages(agent.id, std::slice::from_ref(&message))
            .await
            .unwrap();

        let messages = store.read_chat(agent.id).await.unwrap();
        assert_eq!(messages, vec![message]);
    }
}
