use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{Agent, AgentResponse, CreateAgentRequest, UpdateAgentRequest},
    error::{AppError, AppResult},
    storage::{
        json_store::JsonStore,
        validation::{
            normalize_avatar_color, normalize_avatar_icon, validate_agent_text,
            validate_provider_model, validate_unique_avatar,
        },
    },
};

impl JsonStore {
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
            is_builtin: false,
            avatar_icon,
            avatar_color,
            created_at: now,
            updated_at: now,
        };
        agents.push(agent.clone());
        self.ensure_agent_workspace_unlocked(&agent).await?;
        self.write_agents_unlocked(&agents).await?;
        Ok(AgentResponse::from_agent(&agent))
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

        if let Some(name) = request.name.filter(|_| !agent.is_builtin) {
            agent.name = crate::storage::validation::validate_non_empty("agent name", &name)?;
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
        Ok(AgentResponse::from_agent(&updated))
    }

    pub async fn delete_agent(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if agents
            .iter()
            .any(|agent| agent.id == id && agent.is_builtin)
        {
            return Err(AppError::BadRequest(
                "built-in agents cannot be deleted".to_string(),
            ));
        }
        let mut agents = agents;
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

    pub(super) async fn agent_responses(&self, agents: &[Agent]) -> AppResult<Vec<AgentResponse>> {
        let mut responses = Vec::with_capacity(agents.len());
        for agent in agents {
            responses.push(AgentResponse::from_agent(agent));
        }
        Ok(responses)
    }
}
