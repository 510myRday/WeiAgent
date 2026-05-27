use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use crate::{
    config::{load_or_init_settings, write_settings},
    error::{AppError, AppResult},
    models::{
        Agent, AppSettings, ChatMessage, Conversation, CreateAgentRequest,
        CreateConversationRequest, CreateModelProviderRequest, ModelProvider,
        ModelProviderResponse, UpdateAgentRequest, UpdateModelProviderRequest,
    },
};

pub struct JsonStore {
    settings_file: PathBuf,
    providers_file: PathBuf,
    agents_file: PathBuf,
    conversations_file: PathBuf,
    conversations_dir: PathBuf,
    lock: Mutex<()>,
}

impl JsonStore {
    pub async fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let settings_file = root.join("settings.json");
        let providers_file = root.join("model_providers.json");
        let agents_file = root.join("agents.json");
        let conversations_file = root.join("conversations.json");
        let conversations_dir = root.join("conversations");
        fs::create_dir_all(&root).await?;
        fs::create_dir_all(&conversations_dir).await?;
        ensure_json_array(&providers_file).await?;
        ensure_json_array(&agents_file).await?;
        ensure_json_array(&conversations_file).await?;
        load_or_init_settings(&settings_file).await?;

        let store = Self {
            settings_file,
            providers_file,
            agents_file,
            conversations_file,
            conversations_dir,
            lock: Mutex::new(()),
        };
        store.migrate_legacy_agents().await?;
        Ok(store)
    }

    pub async fn read_settings(&self) -> AppResult<AppSettings> {
        load_or_init_settings(&self.settings_file).await
    }

    pub async fn write_settings(&self, settings: AppSettings) -> AppResult<AppSettings> {
        let _guard = self.lock.lock().await;
        write_settings(&self.settings_file, &settings).await?;
        Ok(settings)
    }

    pub async fn list_model_providers(&self) -> AppResult<Vec<ModelProviderResponse>> {
        let providers = self.read_providers().await?;
        Ok(providers
            .iter()
            .map(ModelProviderResponse::from)
            .collect::<Vec<_>>())
    }

    pub async fn create_model_provider(
        &self,
        request: CreateModelProviderRequest,
    ) -> AppResult<ModelProviderResponse> {
        let name = validate_non_empty("provider name", &request.name)?;
        let base_url = validate_non_empty("base_url", &request.base_url)?;
        let api_key = validate_non_empty("api_key", &request.api_key)?;
        let models = normalize_models(request.models)?;

        let _guard = self.lock.lock().await;
        let mut providers = self.read_providers_unlocked().await?;
        let now = Utc::now();
        let provider = ModelProvider {
            id: Uuid::new_v4(),
            name,
            base_url,
            api_key,
            models,
            created_at: now,
            updated_at: now,
        };
        providers.push(provider.clone());
        self.write_providers_unlocked(&providers).await?;
        Ok(ModelProviderResponse::from(&provider))
    }

    pub async fn update_model_provider(
        &self,
        id: Uuid,
        request: UpdateModelProviderRequest,
    ) -> AppResult<ModelProviderResponse> {
        let _guard = self.lock.lock().await;
        let mut providers = self.read_providers_unlocked().await?;
        let agents = self.read_agents_unlocked().await?;
        let provider = providers
            .iter_mut()
            .find(|provider| provider.id == id)
            .ok_or(AppError::NotFound)?;

        if let Some(name) = request.name {
            provider.name = validate_non_empty("provider name", &name)?;
        }
        if let Some(base_url) = request.base_url {
            provider.base_url = validate_non_empty("base_url", &base_url)?;
        }
        if let Some(api_key) = request.api_key {
            if !api_key.trim().is_empty() {
                provider.api_key = api_key.trim().to_string();
            }
        }
        if let Some(models) = request.models {
            let models = normalize_models(models)?;
            for agent in agents
                .iter()
                .filter(|agent| agent.model_provider_id == provider.id)
            {
                if !models.contains(&agent.model_id) {
                    return Err(AppError::BadRequest(format!(
                        "model {} is used by agent {}",
                        agent.model_id, agent.name
                    )));
                }
            }
            provider.models = models;
        }

        provider.updated_at = Utc::now();
        let updated = provider.clone();
        self.write_providers_unlocked(&providers).await?;
        Ok(ModelProviderResponse::from(&updated))
    }

    pub async fn delete_model_provider(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if let Some(agent) = agents.iter().find(|agent| agent.model_provider_id == id) {
            return Err(AppError::BadRequest(format!(
                "provider is used by agent {}",
                agent.name
            )));
        }

        let mut providers = self.read_providers_unlocked().await?;
        let before = providers.len();
        providers.retain(|provider| provider.id != id);
        if providers.len() == before {
            return Err(AppError::NotFound);
        }
        self.write_providers_unlocked(&providers).await
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

    pub async fn get_model_provider(&self, id: Uuid) -> AppResult<ModelProvider> {
        self.read_providers()
            .await?
            .into_iter()
            .find(|provider| provider.id == id)
            .ok_or(AppError::NotFound)
    }

    pub async fn create_agent(&self, request: CreateAgentRequest) -> AppResult<Agent> {
        validate_agent_text(&request.name, &request.system_prompt, &request.model_id)?;

        let _guard = self.lock.lock().await;
        let providers = self.read_providers_unlocked().await?;
        validate_provider_model(&providers, request.model_provider_id, &request.model_id)?;

        let mut agents = self.read_agents_unlocked().await?;
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: request.name.trim().to_string(),
            system_prompt: request.system_prompt.trim().to_string(),
            model_provider_id: request.model_provider_id,
            model_id: request.model_id.trim().to_string(),
            created_at: now,
            updated_at: now,
        };
        agents.push(agent.clone());
        self.write_agents_unlocked(&agents).await?;
        Ok(agent)
    }

    pub async fn update_agent(&self, id: Uuid, request: UpdateAgentRequest) -> AppResult<Agent> {
        let _guard = self.lock.lock().await;
        let providers = self.read_providers_unlocked().await?;
        let mut agents = self.read_agents_unlocked().await?;
        let agent = agents
            .iter_mut()
            .find(|agent| agent.id == id)
            .ok_or(AppError::NotFound)?;

        if let Some(name) = request.name {
            agent.name = validate_non_empty("agent name", &name)?;
        }
        if let Some(system_prompt) = request.system_prompt {
            agent.system_prompt = validate_non_empty("system prompt", &system_prompt)?;
        }

        let next_provider_id = request.model_provider_id.unwrap_or(agent.model_provider_id);
        let next_model_id = request
            .model_id
            .as_deref()
            .unwrap_or(&agent.model_id)
            .trim()
            .to_string();
        validate_provider_model(&providers, next_provider_id, &next_model_id)?;
        agent.model_provider_id = next_provider_id;
        agent.model_id = next_model_id;
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

        let mut conversations = self.read_conversations_unlocked().await?;
        let deleted = conversations
            .iter()
            .filter(|conversation| conversation.agent_id == id)
            .map(|conversation| conversation.id)
            .collect::<Vec<_>>();
        conversations.retain(|conversation| conversation.agent_id != id);
        self.write_conversations_unlocked(&conversations).await?;
        for conversation_id in deleted {
            let file = self.conversation_file(conversation_id);
            if fs::metadata(&file).await.is_ok() {
                fs::remove_file(file).await?;
            }
        }
        Ok(())
    }

    pub async fn list_conversations(&self, agent_id: Uuid) -> AppResult<Vec<Conversation>> {
        self.get_agent(agent_id).await?;
        let mut conversations = self
            .read_conversations()
            .await?
            .into_iter()
            .filter(|conversation| conversation.agent_id == agent_id)
            .collect::<Vec<_>>();
        conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(conversations)
    }

    pub async fn get_conversation(&self, id: Uuid) -> AppResult<Conversation> {
        self.read_conversations()
            .await?
            .into_iter()
            .find(|conversation| conversation.id == id)
            .ok_or(AppError::NotFound)
    }

    pub async fn create_conversation(
        &self,
        agent_id: Uuid,
        request: CreateConversationRequest,
    ) -> AppResult<Conversation> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if !agents.iter().any(|agent| agent.id == agent_id) {
            return Err(AppError::NotFound);
        }
        let mut conversations = self.read_conversations_unlocked().await?;
        let now = Utc::now();
        let conversation = Conversation {
            id: Uuid::new_v4(),
            agent_id,
            title: request
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or("New Conversation")
                .to_string(),
            created_at: now,
            updated_at: now,
        };
        conversations.push(conversation.clone());
        self.write_conversations_unlocked(&conversations).await?;
        fs::write(self.conversation_file(conversation.id), "[]").await?;
        Ok(conversation)
    }

    pub async fn delete_conversation(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let before = conversations.len();
        conversations.retain(|conversation| conversation.id != id);
        if conversations.len() == before {
            return Err(AppError::NotFound);
        }
        self.write_conversations_unlocked(&conversations).await?;
        let file = self.conversation_file(id);
        if fs::metadata(&file).await.is_ok() {
            fs::remove_file(file).await?;
        }
        Ok(())
    }

    pub async fn read_conversation_messages(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Vec<ChatMessage>> {
        self.get_conversation(conversation_id).await?;
        self.read_conversation_messages_unchecked(conversation_id)
            .await
    }

    pub async fn append_conversation_messages(
        &self,
        conversation_id: Uuid,
        messages: &[ChatMessage],
    ) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let conversation = conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            .ok_or(AppError::NotFound)?;

        let mut existing = self
            .read_conversation_messages_unchecked(conversation_id)
            .await?;
        existing.extend_from_slice(messages);
        fs::write(
            self.conversation_file(conversation_id),
            serde_json::to_string_pretty(&existing)?,
        )
        .await?;
        conversation.updated_at = Utc::now();
        self.write_conversations_unlocked(&conversations).await
    }

    async fn read_providers(&self) -> AppResult<Vec<ModelProvider>> {
        let _guard = self.lock.lock().await;
        self.read_providers_unlocked().await
    }

    async fn read_providers_unlocked(&self) -> AppResult<Vec<ModelProvider>> {
        let content = fs::read_to_string(&self.providers_file).await?;
        Ok(serde_json::from_str(&content)?)
    }

    async fn write_providers_unlocked(&self, providers: &[ModelProvider]) -> AppResult<()> {
        fs::write(
            &self.providers_file,
            serde_json::to_string_pretty(providers)?,
        )
        .await?;
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
        fs::write(&self.agents_file, serde_json::to_string_pretty(agents)?).await?;
        Ok(())
    }

    async fn read_conversations(&self) -> AppResult<Vec<Conversation>> {
        let _guard = self.lock.lock().await;
        self.read_conversations_unlocked().await
    }

    async fn read_conversations_unlocked(&self) -> AppResult<Vec<Conversation>> {
        let content = fs::read_to_string(&self.conversations_file).await?;
        Ok(serde_json::from_str(&content)?)
    }

    async fn write_conversations_unlocked(&self, conversations: &[Conversation]) -> AppResult<()> {
        fs::write(
            &self.conversations_file,
            serde_json::to_string_pretty(conversations)?,
        )
        .await?;
        Ok(())
    }

    async fn read_conversation_messages_unchecked(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Vec<ChatMessage>> {
        let file = self.conversation_file(conversation_id);
        if fs::metadata(&file).await.is_err() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(file).await?;
        Ok(serde_json::from_str(&content)?)
    }

    fn conversation_file(&self, conversation_id: Uuid) -> PathBuf {
        self.conversations_dir
            .join(format!("{conversation_id}.json"))
    }

    async fn migrate_legacy_agents(&self) -> AppResult<()> {
        let content = fs::read_to_string(&self.agents_file).await?;
        let mut raw_agents: Vec<Value> = serde_json::from_str(&content)?;
        let legacy_models = raw_agents
            .iter()
            .filter(|agent| agent.get("model_provider_id").is_none())
            .filter_map(|agent| agent.get("model").and_then(Value::as_str))
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        if legacy_models.is_empty() {
            return Ok(());
        }

        let mut providers = self.read_providers_unlocked().await?;
        let now = Utc::now();
        let provider_id = providers
            .iter()
            .find(|provider| provider.name == "Default Provider")
            .map(|provider| provider.id)
            .unwrap_or_else(Uuid::new_v4);

        if let Some(provider) = providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            for model in legacy_models.iter() {
                if !provider.models.contains(model) {
                    provider.models.push(model.clone());
                }
            }
            provider.models.sort();
            provider.models.dedup();
            provider.updated_at = now;
        } else {
            let mut models = legacy_models.clone();
            models.sort();
            models.dedup();
            providers.push(ModelProvider {
                id: provider_id,
                name: "Default Provider".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                models,
                created_at: now,
                updated_at: now,
            });
        }

        for agent in raw_agents.iter_mut() {
            if agent.get("model_provider_id").is_some() {
                continue;
            }
            let model = agent
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if let Some(object) = agent.as_object_mut() {
                object.remove("model");
                object.insert("model_provider_id".to_string(), json!(provider_id));
                object.insert("model_id".to_string(), json!(model));
            }
        }

        let agents = raw_agents
            .into_iter()
            .map(serde_json::from_value::<Agent>)
            .collect::<Result<Vec<_>, _>>()?;
        self.write_providers_unlocked(&providers).await?;
        self.write_agents_unlocked(&agents).await
    }

    #[allow(dead_code)]
    async fn migrate_legacy_chats(&self) -> AppResult<()> {
        let _ = self.read_conversations_unlocked().await?;
        Ok(())
    }
}


async fn ensure_json_array(path: &Path) -> AppResult<()> {
    if fs::metadata(path).await.is_err() {
        fs::write(path, "[]").await?;
    }
    Ok(())
}

fn validate_non_empty(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn normalize_models(models: Vec<String>) -> AppResult<Vec<String>> {
    let mut models = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(AppError::BadRequest(
            "models must contain at least one model".to_string(),
        ));
    }
    Ok(models)
}

fn validate_agent_text(name: &str, system_prompt: &str, model_id: &str) -> AppResult<()> {
    validate_non_empty("agent name", name)?;
    validate_non_empty("system prompt", system_prompt)?;
    validate_non_empty("model_id", model_id)?;
    Ok(())
}

fn validate_provider_model(
    providers: &[ModelProvider],
    provider_id: Uuid,
    model_id: &str,
) -> AppResult<()> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err(AppError::BadRequest("model_id cannot be empty".to_string()));
    }
    let provider = providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| AppError::BadRequest("model provider does not exist".to_string()))?;
    if !provider.models.iter().any(|model| model == model_id) {
        return Err(AppError::BadRequest(
            "model does not exist on selected provider".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initializes_settings_and_providers_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let settings = store.read_settings().await.unwrap();
        assert_eq!(settings.host, "127.0.0.1");
        assert!(temp_dir.path().join("model_providers.json").exists());
    }

    #[tokio::test]
    async fn stores_providers_with_masked_keys_and_preserves_blank_update() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                name: "OpenAI".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "sk-test-secret".to_string(),
                models: vec!["gpt-4o-mini".to_string()],
            })
            .await
            .unwrap();

        assert!(provider.has_api_key);
        assert_eq!(provider.masked_api_key, "****cret");

        let updated = store
            .update_model_provider(
                provider.id,
                UpdateModelProviderRequest {
                    name: Some("OpenAI Compatible".to_string()),
                    base_url: None,
                    api_key: Some(String::new()),
                    models: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "OpenAI Compatible");
        assert!(updated.has_api_key);
    }

    #[tokio::test]
    async fn validates_agent_provider_and_stores_conversations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                name: "OpenAI".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "sk-test".to_string(),
                models: vec!["gpt-4o-mini".to_string()],
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Planner".to_string(),
                system_prompt: "Plan work clearly.".to_string(),
                model_provider_id: provider.id,
                model_id: "gpt-4o-mini".to_string(),
            })
            .await
            .unwrap();
        let conversation = store
            .create_conversation(agent.id, CreateConversationRequest { title: None })
            .await
            .unwrap();
        let message = ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id: conversation.id,
            role: ChatRole::User,
            content: "hello".to_string(),
            created_at: Utc::now(),
        };
        store
            .append_conversation_messages(conversation.id, std::slice::from_ref(&message))
            .await
            .unwrap();

        assert_eq!(
            store
                .read_conversation_messages(conversation.id)
                .await
                .unwrap(),
            vec![message]
        );
    }
}
