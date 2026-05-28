use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use crate::{
    config::{load_or_init_settings, write_settings},
    error::{AppError, AppResult},
    models::{
        Agent, AgentResponse, AgentWorkspace, AgentWorkspaceSettings, AppSettings, ChatMessage,
        Conversation, CreateAgentRequest, CreateConversationRequest, CreateModelProviderRequest,
        ModelProvider, ModelProviderResponse, UpdateAgentRequest, UpdateAgentWorkspaceRequest,
        UpdateModelProviderRequest,
    },
};

pub struct JsonStore {
    settings_file: PathBuf,
    providers_file: PathBuf,
    agents_file: PathBuf,
    agents_dir: PathBuf,
    conversations_file: PathBuf,
    conversations_dir: PathBuf,
    lock: Mutex<()>,
}

const BUILTIN_CEO_KEY: &str = "wei-ceo";
const DEFAULT_AGENT_ICON: &str = "bot";
const DEFAULT_AGENT_COLOR: &str = "#22c55e";
const BUILTIN_CEO_ICON: &str = "crown";
const BUILTIN_CEO_COLOR: &str = "#2563eb";
const WORKSPACE_PERSONALITY: &str = "personality.md";
const WORKSPACE_MEMORY: &str = "memory.md";
const WORKSPACE_INSTRUCTIONS: &str = "instructions.md";
const WORKSPACE_TOOLS: &str = "tools.json";
const WORKSPACE_SETTINGS: &str = "settings.json";

impl JsonStore {
    pub async fn new(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let settings_file = root.join("settings.json");
        let providers_file = root.join("model_providers.json");
        let agents_file = root.join("agents.json");
        let agents_dir = root.join("agents");
        let conversations_file = root.join("conversations.json");
        let conversations_dir = root.join("conversations");
        fs::create_dir_all(&root).await?;
        fs::create_dir_all(&agents_dir).await?;
        fs::create_dir_all(&conversations_dir).await?;
        ensure_json_array(&providers_file).await?;
        ensure_json_array(&agents_file).await?;
        ensure_json_array(&conversations_file).await?;
        load_or_init_settings(&settings_file).await?;

        let store = Self {
            settings_file,
            providers_file,
            agents_file,
            agents_dir,
            conversations_file,
            conversations_dir,
            lock: Mutex::new(()),
        };
        store.migrate_legacy_agents().await?;
        store.ensure_builtin_agents().await?;
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
            extra_body: request.extra_body,
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
        if let Some(extra_body) = request.extra_body {
            provider.extra_body = extra_body;
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

    pub async fn list_agents(&self) -> AppResult<Vec<AgentResponse>> {
        let agents = self.read_agents().await?;
        self.agent_responses(&agents).await
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

    pub async fn create_agent(&self, request: CreateAgentRequest) -> AppResult<AgentResponse> {
        validate_agent_text(&request.name, &request.model_id)?;
        let avatar_icon = normalize_avatar_icon(request.avatar_icon.as_deref())?;
        let avatar_color = normalize_avatar_color(request.avatar_color.as_deref())?;

        let _guard = self.lock.lock().await;
        let providers = self.read_providers_unlocked().await?;
        validate_provider_model(&providers, request.model_provider_id, &request.model_id)?;

        let mut agents = self.read_agents_unlocked().await?;
        validate_unique_avatar(&agents, None, &avatar_icon, &avatar_color)?;
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: request.name.trim().to_string(),
            model_provider_id: request.model_provider_id,
            model_id: request.model_id.trim().to_string(),
            avatar_icon,
            avatar_color,
            created_at: now,
            updated_at: now,
        };
        agents.push(agent.clone());
        self.ensure_agent_workspace_unlocked(&agent, None).await?;
        self.write_agents_unlocked(&agents).await?;
        Ok(AgentResponse::from_agent(
            &agent,
            AgentWorkspaceSettings::default(),
        ))
    }

    pub async fn update_agent(
        &self,
        id: Uuid,
        request: UpdateAgentRequest,
    ) -> AppResult<AgentResponse> {
        let _guard = self.lock.lock().await;
        let providers = self.read_providers_unlocked().await?;
        let mut agents = self.read_agents_unlocked().await?;
        let agent_index = agents
            .iter()
            .position(|agent| agent.id == id)
            .ok_or(AppError::NotFound)?;
        let next_avatar_icon = request
            .avatar_icon
            .as_deref()
            .map(|value| normalize_avatar_icon(Some(value)))
            .transpose()?
            .unwrap_or_else(|| agents[agent_index].avatar_icon.clone());
        let next_avatar_color = request
            .avatar_color
            .as_deref()
            .map(|value| normalize_avatar_color(Some(value)))
            .transpose()?
            .unwrap_or_else(|| agents[agent_index].avatar_color.clone());
        validate_unique_avatar(&agents, Some(id), &next_avatar_icon, &next_avatar_color)?;
        let agent = agents.get_mut(agent_index).ok_or(AppError::NotFound)?;

        if let Some(name) = request.name {
            agent.name = validate_non_empty("agent name", &name)?;
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
        agent.avatar_icon = next_avatar_icon;
        agent.avatar_color = next_avatar_color;
        agent.updated_at = Utc::now();

        let updated = agent.clone();
        self.write_agents_unlocked(&agents).await?;
        let settings = self.read_workspace_settings_unlocked(updated.id).await?;
        Ok(AgentResponse::from_agent(&updated, settings))
    }

    pub async fn delete_agent(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let settings = self.read_workspace_settings_unlocked(id).await?;
        if settings.is_builtin || !settings.can_delete {
            return Err(AppError::BadRequest(
                "built-in agents cannot be deleted".to_string(),
            ));
        }
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
        let workspace = self.agent_workspace_dir(id);
        if fs::metadata(&workspace).await.is_ok() {
            fs::remove_dir_all(workspace).await?;
        }
        Ok(())
    }

    pub async fn read_agent_workspace(&self, id: Uuid) -> AppResult<AgentWorkspace> {
        self.get_agent(id).await?;
        let _guard = self.lock.lock().await;
        self.read_agent_workspace_unlocked(id).await
    }

    pub async fn update_agent_workspace(
        &self,
        id: Uuid,
        request: UpdateAgentWorkspaceRequest,
    ) -> AppResult<AgentWorkspace> {
        self.get_agent(id).await?;
        let _guard = self.lock.lock().await;
        self.write_workspace_text_unlocked(id, WORKSPACE_PERSONALITY, &request.personality)
            .await?;
        self.write_workspace_text_unlocked(id, WORKSPACE_MEMORY, &request.memory)
            .await?;
        self.write_workspace_text_unlocked(id, WORKSPACE_INSTRUCTIONS, &request.instructions)
            .await?;
        self.read_agent_workspace_unlocked(id).await
    }

    pub async fn build_agent_system_prompt(&self, agent: &Agent) -> AppResult<String> {
        let workspace = self.read_agent_workspace(agent.id).await?;
        Ok(join_prompt_sections([
            ("Personality", workspace.personality.as_str()),
            ("Memory", workspace.memory.as_str()),
            ("Instructions", workspace.instructions.as_str()),
        ]))
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

    async fn agent_responses(&self, agents: &[Agent]) -> AppResult<Vec<AgentResponse>> {
        let mut responses = Vec::with_capacity(agents.len());
        for agent in agents {
            let settings = self.read_workspace_settings_unlocked(agent.id).await?;
            responses.push(AgentResponse::from_agent(agent, settings));
        }
        Ok(responses)
    }

    async fn ensure_builtin_agents(&self) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut agents = self.read_agents_unlocked().await?;
        let now = Utc::now();

        let mut existing_builtin = None;
        for (index, agent) in agents.iter().enumerate() {
            let settings = self.read_workspace_settings_unlocked(agent.id).await?;
            if settings.builtin_key.as_deref() == Some(BUILTIN_CEO_KEY) {
                existing_builtin = Some(index);
                break;
            }
        }

        if let Some(index) = existing_builtin {
            self.ensure_agent_workspace_unlocked(&agents[index], Some(builtin_ceo_settings()))
                .await?;
        } else {
            let agent = Agent {
                id: Uuid::new_v4(),
                name: "Wei 首席专家".to_string(),
                model_provider_id: Uuid::nil(),
                model_id: String::new(),
                avatar_icon: BUILTIN_CEO_ICON.to_string(),
                avatar_color: BUILTIN_CEO_COLOR.to_string(),
                created_at: now,
                updated_at: now,
            };
            self.ensure_agent_workspace_unlocked(&agent, Some(builtin_ceo_settings()))
                .await?;
            agents.push(agent);
        }

        self.write_agents_unlocked(&agents).await
    }

    async fn read_agent_workspace_unlocked(&self, id: Uuid) -> AppResult<AgentWorkspace> {
        Ok(AgentWorkspace {
            personality: self
                .read_workspace_text_unlocked(id, WORKSPACE_PERSONALITY)
                .await?,
            memory: self
                .read_workspace_text_unlocked(id, WORKSPACE_MEMORY)
                .await?,
            instructions: self
                .read_workspace_text_unlocked(id, WORKSPACE_INSTRUCTIONS)
                .await?,
            settings: self.read_workspace_settings_unlocked(id).await?,
        })
    }

    async fn ensure_agent_workspace_unlocked(
        &self,
        agent: &Agent,
        settings: Option<AgentWorkspaceSettings>,
    ) -> AppResult<()> {
        let workspace = self.agent_workspace_dir(agent.id);
        fs::create_dir_all(&workspace).await?;
        ensure_text_file(
            &workspace.join(WORKSPACE_PERSONALITY),
            default_personality(agent),
        )
        .await?;
        ensure_text_file(&workspace.join(WORKSPACE_MEMORY), default_memory()).await?;
        ensure_text_file(
            &workspace.join(WORKSPACE_INSTRUCTIONS),
            default_instructions(agent),
        )
        .await?;
        ensure_text_file(&workspace.join(WORKSPACE_TOOLS), "[]\n".to_string()).await?;
        let settings_file = workspace.join(WORKSPACE_SETTINGS);
        if fs::metadata(&settings_file).await.is_err() {
            let content = serde_json::to_string_pretty(
                &settings.unwrap_or_else(AgentWorkspaceSettings::default),
            )?;
            fs::write(settings_file, content).await?;
        } else if let Some(settings) = settings {
            fs::write(settings_file, serde_json::to_string_pretty(&settings)?).await?;
        }
        Ok(())
    }

    async fn read_workspace_text_unlocked(&self, id: Uuid, file_name: &str) -> AppResult<String> {
        let path = self.agent_workspace_dir(id).join(file_name);
        if fs::metadata(&path).await.is_err() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(path).await?)
    }

    async fn write_workspace_text_unlocked(
        &self,
        id: Uuid,
        file_name: &str,
        content: &str,
    ) -> AppResult<()> {
        let workspace = self.agent_workspace_dir(id);
        fs::create_dir_all(&workspace).await?;
        fs::write(workspace.join(file_name), content).await?;
        Ok(())
    }

    async fn read_workspace_settings_unlocked(
        &self,
        id: Uuid,
    ) -> AppResult<AgentWorkspaceSettings> {
        let path = self.agent_workspace_dir(id).join(WORKSPACE_SETTINGS);
        if fs::metadata(&path).await.is_err() {
            return Ok(AgentWorkspaceSettings::default());
        }
        let content = fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&content)?)
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

    fn agent_workspace_dir(&self, agent_id: Uuid) -> PathBuf {
        self.agents_dir.join(agent_id.to_string())
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

        let providers = self.read_providers_unlocked().await?;
        let provider_id = providers
            .first()
            .map(|p| p.id)
            .unwrap_or_else(Uuid::nil);

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

async fn ensure_text_file(path: &Path, content: String) -> AppResult<()> {
    if fs::metadata(path).await.is_err() {
        fs::write(path, content).await?;
    }
    Ok(())
}

fn builtin_ceo_settings() -> AgentWorkspaceSettings {
    AgentWorkspaceSettings {
        is_builtin: true,
        builtin_key: Some(BUILTIN_CEO_KEY.to_string()),
        display_label: Some("内置 CEO Agent".to_string()),
        model_note: Some(
            "建议为 Wei 首席专家配置能力强、上下文长、推理稳定的模型，因为它承担总控、规划和长期协作入口。"
                .to_string(),
        ),
        can_delete: false,
    }
}

fn default_personality(agent: &Agent) -> String {
    if agent.name == "Wei 首席专家" {
        return "# Personality\n\n你沉稳、负责、主动澄清目标，像 CEO 一样帮助用户做判断、拆解任务、排序优先级，并把复杂事项推进到可执行状态。\n".to_string();
    }
    "# Personality\n\n记录这个 Agent 的个性、人设、沟通风格和长期稳定的行为偏好。\n".to_string()
}

fn default_memory() -> String {
    "# Memory\n\n这里用于记录用户偏好、长期项目背景和重要决策。初始阶段不要编造事实，只在用户明确提供或后续系统沉淀时补充。\n".to_string()
}

fn default_instructions(agent: &Agent) -> String {
    if agent.name == "Wei 首席专家" {
        return "# Instructions\n\n先理解用户目标和约束；信息不足时提出关键追问；再给出计划、风险、取舍和下一步行动。未来可以建议创建或调用其他 Agent，但当前阶段先以清晰统筹和高质量建议为主。\n".to_string();
    }
    "# Instructions\n\n记录这个 Agent 的核心工作规则、输出要求、边界和默认流程。\n".to_string()
}

fn join_prompt_sections<'a>(sections: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    sections
        .into_iter()
        .filter_map(|(title, content)| {
            let content = content.trim();
            if content.is_empty() {
                None
            } else {
                Some(format!("## {title}\n{content}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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

fn validate_agent_text(name: &str, model_id: &str) -> AppResult<()> {
    validate_non_empty("agent name", name)?;
    validate_non_empty("model_id", model_id)?;
    Ok(())
}

fn normalize_avatar_icon(value: Option<&str>) -> AppResult<String> {
    let icon = value.unwrap_or(DEFAULT_AGENT_ICON).trim();
    if icon.is_empty() {
        return Err(AppError::BadRequest(
            "avatar_icon cannot be empty".to_string(),
        ));
    }
    Ok(icon.to_string())
}

fn normalize_avatar_color(value: Option<&str>) -> AppResult<String> {
    let color = value.unwrap_or(DEFAULT_AGENT_COLOR).trim();
    if !is_hex_color(color) {
        return Err(AppError::BadRequest(
            "avatar_color must be a hex color like #22c55e".to_string(),
        ));
    }
    Ok(color.to_ascii_lowercase())
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.chars().skip(1).all(|ch| ch.is_ascii_hexdigit())
}

fn validate_unique_avatar(
    agents: &[Agent],
    current_id: Option<Uuid>,
    icon: &str,
    color: &str,
) -> AppResult<()> {
    if agents.iter().any(|agent| {
        Some(agent.id) != current_id
            && agent.avatar_icon == icon
            && agent.avatar_color.eq_ignore_ascii_case(color)
    }) {
        return Err(AppError::BadRequest(
            "another agent already uses this avatar icon and color".to_string(),
        ));
    }
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
    use crate::models::ChatRole;

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
                model_provider_id: provider.id,
                model_id: "gpt-4o-mini".to_string(),
                avatar_icon: Some("sparkles".to_string()),
                avatar_color: Some("#3b82f6".to_string()),
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

    #[tokio::test]
    async fn initializes_builtin_agent_workspace_and_blocks_delete() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();

        let agents = store.list_agents().await.unwrap();
        let builtin = agents
            .iter()
            .find(|agent| agent.builtin_key.as_deref() == Some(BUILTIN_CEO_KEY))
            .unwrap();

        assert_eq!(builtin.name, "Wei 首席专家");
        assert!(builtin.is_builtin);
        assert!(!builtin.can_delete);
        assert!(
            temp_dir
                .path()
                .join("agents")
                .join(builtin.id.to_string())
                .exists()
        );
        assert!(store.delete_agent(builtin.id).await.is_err());
    }

    #[tokio::test]
    async fn creates_workspace_and_composes_markdown_prompt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                name: "Local".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "sk-test".to_string(),
                models: vec!["local-model".to_string()],
            })
            .await
            .unwrap();
        let agent = store
            .create_agent(CreateAgentRequest {
                name: "Operator".to_string(),
                model_provider_id: provider.id,
                model_id: "local-model".to_string(),
                avatar_icon: Some("brain".to_string()),
                avatar_color: Some("#8b5cf6".to_string()),
            })
            .await
            .unwrap();

        store
            .update_agent_workspace(
                agent.id,
                UpdateAgentWorkspaceRequest {
                    personality: "Calm and direct.".to_string(),
                    memory: "User prefers concise plans.".to_string(),
                    instructions: "Ask only important questions.".to_string(),
                },
            )
            .await
            .unwrap();

        let stored = store.get_agent(agent.id).await.unwrap();
        let prompt = store.build_agent_system_prompt(&stored).await.unwrap();
        assert!(prompt.contains("Calm and direct."));
        assert!(prompt.contains("User prefers concise plans."));
        assert!(prompt.contains("Ask only important questions."));
    }

    #[tokio::test]
    async fn rejects_duplicate_agent_avatar_combination() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(temp_dir.path()).await.unwrap();
        let provider = store
            .create_model_provider(CreateModelProviderRequest {
                name: "Avatar Test".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "sk-test".to_string(),
                models: vec!["avatar-model".to_string()],
            })
            .await
            .unwrap();

        store
            .create_agent(CreateAgentRequest {
                name: "One".to_string(),
                model_provider_id: provider.id,
                model_id: "avatar-model".to_string(),
                avatar_icon: Some("compass".to_string()),
                avatar_color: Some("#06b6d4".to_string()),
            })
            .await
            .unwrap();
        let duplicate = store
            .create_agent(CreateAgentRequest {
                name: "Two".to_string(),
                model_provider_id: provider.id,
                model_id: "avatar-model".to_string(),
                avatar_icon: Some("compass".to_string()),
                avatar_color: Some("#06b6d4".to_string()),
            })
            .await;

        assert!(duplicate.is_err());
    }
}
