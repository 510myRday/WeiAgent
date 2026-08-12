use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{
        Agent, AgentProfileResponse, AgentResponse, AgentWorkspace, CreateAgentRequest,
        ModelProvider, UpdateAgentProfileRequest, UpdateAgentRequest,
    },
    error::{AppError, AppResult},
    storage::{
        db,
        store::Store,
        validation::{
            normalize_avatar_color, normalize_avatar_icon, validate_agent_text,
            validate_provider_model, validate_unique_avatar,
        },
    },
};

impl Store {
    pub async fn list_agents(&self) -> AppResult<Vec<AgentResponse>> {
        let agents = self.read_agents().await?;
        Ok(agents.iter().map(AgentResponse::from_agent).collect())
    }

    pub async fn get_agent(&self, id: Uuid) -> AppResult<Agent> {
        self.read_agents()
            .await?
            .into_iter()
            .find(|agent| agent.id == id)
            .ok_or(AppError::NotFound)
    }

    pub async fn create_agent(&self, request: CreateAgentRequest) -> AppResult<AgentResponse> {
        validate_agent_text(&request.name, &request.description, &request.model_id)?;
        let avatar_icon = normalize_avatar_icon(request.avatar_icon.as_deref())?;
        let avatar_color = normalize_avatar_color(request.avatar_color.as_deref())?;

        let _guard = self.agents_lock.lock().await;
        let _workspace_guard = self.workspace_lock.lock().await;
        let providers = self
            .database
            .call(|connection| db::read_providers_conn(connection))
            .await?;
        validate_provider_model(&providers, request.model_provider_id, &request.model_id)?;

        let mut agents = self.read_agents_unlocked().await?;
        validate_unique_avatar(&agents, None, &avatar_icon, &avatar_color)?;
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: request.name.trim().to_string(),
            description: request.description.trim().to_string(),
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
        let _guard = self.agents_lock.lock().await;
        let _workspace_guard = self.workspace_lock.lock().await;
        let providers = self
            .database
            .call(|connection| db::read_providers_conn(connection))
            .await?;
        let mut agents = self.read_agents_unlocked().await?;
        let updated = apply_agent_update(&mut agents, &providers, id, request)?;
        self.write_agents_unlocked(&agents).await?;
        Ok(AgentResponse::from_agent(&updated))
    }

    pub async fn update_agent_profile(
        &self,
        id: Uuid,
        request: UpdateAgentProfileRequest,
    ) -> AppResult<AgentProfileResponse> {
        let _guard = self.agents_lock.lock().await;
        let _workspace_guard = self.workspace_lock.lock().await;
        let providers = self
            .database
            .call(|connection| db::read_providers_conn(connection))
            .await?;
        let mut agents = self.read_agents_unlocked().await?;
        let original_workspace = self.read_agent_workspace_unlocked(id).await?;
        let updated = apply_agent_update(&mut agents, &providers, id, request.agent)?;
        let workspace = AgentWorkspace {
            personality: request.workspace.personality,
            memory: request.workspace.memory,
            instructions: request.workspace.instructions,
        };

        if let Err(error) = self.write_agent_workspace_unlocked(id, &workspace).await {
            if let Err(rollback_error) = self
                .write_agent_workspace_unlocked(id, &original_workspace)
                .await
            {
                tracing::error!(%rollback_error, "failed to roll back agent workspace");
            }
            return Err(error);
        }
        if let Err(error) = self.write_agents_unlocked(&agents).await {
            if let Err(rollback_error) = self
                .write_agent_workspace_unlocked(id, &original_workspace)
                .await
            {
                tracing::error!(%rollback_error, "failed to roll back agent profile workspace");
            }
            return Err(error);
        }

        Ok(AgentProfileResponse {
            agent: AgentResponse::from_agent(&updated),
            workspace,
        })
    }

    pub async fn delete_agent(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.agents_lock.lock().await;
        let _workspace_guard = self.workspace_lock.lock().await;
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
        match self
            .database
            .call(move |conn| {
                conn.execute(
                    "DELETE FROM tool_mcp_agent_tools WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM tool_mcp_agent_servers WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM tool_mcp_agent_seen_tools WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM tool_mcp_agent_seen_servers WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM tool_mcp_agent_baselines WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM skill_agent_overrides WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM skill_agent_group_overrides WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM skill_agent_seen WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM skill_agent_baselines WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                crate::storage::conversation_db::delete_agent_conversations_conn(conn, id)
            })
            .await
        {
            Ok(removed) => {
                for (conversation_id, messages) in removed {
                    self.cleanup_removed_generated_attachments(conversation_id, &messages)
                        .await;
                }
            }
            Err(error) => {
                let _ = self.write_agents_unlocked(&agents).await;
                return Err(error);
            }
        }
        if let Err(error) = self.delete_agent_tool_settings_unlocked(id).await {
            tracing::warn!(agent_id = %id, %error, "failed to remove deleted agent tool settings");
        }
        let workspace = self.agent_workspace_dir(id);
        if fs::metadata(&workspace).await.is_ok()
            && let Err(error) = fs::remove_dir_all(&workspace).await
        {
            tracing::warn!(path = %workspace.display(), %error, "failed to remove deleted agent workspace");
        }
        Ok(())
    }
}

fn apply_agent_update(
    agents: &mut [Agent],
    providers: &[ModelProvider],
    id: Uuid,
    request: UpdateAgentRequest,
) -> AppResult<Agent> {
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
    validate_unique_avatar(agents, Some(id), &next_avatar_icon, &next_avatar_color)?;
    let agent = agents.get_mut(agent_index).ok_or(AppError::NotFound)?;

    if let Some(name) = request.name.filter(|_| !agent.is_builtin) {
        agent.name = crate::storage::validation::validate_non_empty("agent name", &name)?;
    }
    if let Some(description) = request.description.filter(|_| !agent.is_builtin) {
        agent.description =
            crate::storage::validation::validate_non_empty("agent description", &description)?;
    }

    let next_provider_id = request.model_provider_id.unwrap_or(agent.model_provider_id);
    let next_model_id = request
        .model_id
        .as_deref()
        .unwrap_or(&agent.model_id)
        .trim()
        .to_string();
    validate_provider_model(providers, next_provider_id, &next_model_id)?;
    agent.model_provider_id = next_provider_id;
    agent.model_id = next_model_id;
    agent.avatar_icon = next_avatar_icon;
    agent.avatar_color = next_avatar_color;
    agent.updated_at = Utc::now();
    Ok(agent.clone())
}
