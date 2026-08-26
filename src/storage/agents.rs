use chrono::Utc;
use rusqlite::{Connection, Transaction, params};
use tokio::fs;
use uuid::Uuid;

use super::providers;
use crate::{
    domain::{
        Agent, AgentProfileResponse, AgentResponse, AgentWorkspace, CreateAgentRequest,
        ModelProvider, UpdateAgentProfileRequest, UpdateAgentRequest,
    },
    error::{AppError, AppResult},
    storage::{
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
            .call(|connection| providers::read_providers_conn(connection))
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
        self.initialize_mcp_agent_defaults(agent.id).await?;
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
            .call(|connection| providers::read_providers_conn(connection))
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
            .call(|connection| providers::read_providers_conn(connection))
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

pub(super) const AGENTS_SCHEMA_VERSION: i64 = 19;

const AGENTS_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    model_provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    is_builtin INTEGER NOT NULL DEFAULT 0 CHECK (is_builtin IN (0, 1)),
    avatar_icon TEXT NOT NULL,
    avatar_color TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agents_name ON agents(name COLLATE NOCASE);
";

pub(super) fn migrate_agents(conn: &mut Connection) -> AppResult<()> {
    let applied = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_schema_migrations WHERE version = ?1)",
        [AGENTS_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(AGENTS_SCHEMA_SQL)?;
    tx.execute(
        "INSERT INTO app_schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
        params![
            AGENTS_SCHEMA_VERSION,
            "agents_table_v19",
            crate::storage::sql_util::now_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn read_agents_conn(conn: &Connection) -> AppResult<Vec<Agent>> {
    let mut statement = conn.prepare(
        "SELECT id, name, description, model_provider_id, model_id, is_builtin,
                avatar_icon, avatar_color, created_at, updated_at
         FROM agents
         ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;
    let mut agents = Vec::new();
    for row in rows {
        let (
            id,
            name,
            description,
            model_provider_id,
            model_id,
            is_builtin,
            avatar_icon,
            avatar_color,
            created_at,
            updated_at,
        ) = row?;
        agents.push(Agent {
            id: crate::storage::sql_util::parse_uuid(&id, "agents.id")?,
            name,
            description,
            model_provider_id: crate::storage::sql_util::parse_uuid(
                &model_provider_id,
                "agents.model_provider_id",
            )?,
            model_id,
            is_builtin: crate::storage::sql_util::i64_to_bool(is_builtin, "agents.is_builtin")?,
            avatar_icon,
            avatar_color,
            created_at: crate::storage::sql_util::parse_datetime(&created_at, "agents.created_at")?,
            updated_at: crate::storage::sql_util::parse_datetime(&updated_at, "agents.updated_at")?,
        });
    }
    Ok(agents)
}

fn upsert_agent_tx(tx: &Transaction<'_>, agent: &Agent) -> AppResult<()> {
    tx.execute(
        "INSERT INTO agents (
            id, name, description, model_provider_id, model_id, is_builtin,
            avatar_icon, avatar_color, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            description = excluded.description,
            model_provider_id = excluded.model_provider_id,
            model_id = excluded.model_id,
            is_builtin = excluded.is_builtin,
            avatar_icon = excluded.avatar_icon,
            avatar_color = excluded.avatar_color,
            updated_at = excluded.updated_at",
        params![
            agent.id.to_string(),
            agent.name,
            agent.description,
            agent.model_provider_id.to_string(),
            agent.model_id,
            crate::storage::sql_util::bool_to_i64(agent.is_builtin),
            agent.avatar_icon,
            agent.avatar_color,
            agent.created_at.to_rfc3339(),
            agent.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn write_agents_conn(conn: &mut Connection, agents: &[Agent]) -> AppResult<()> {
    let tx = conn.transaction()?;
    let keep = agents
        .iter()
        .map(|agent| agent.id.to_string())
        .collect::<Vec<_>>();
    if keep.is_empty() {
        tx.execute("DELETE FROM agents", [])?;
    } else {
        let placeholders = keep.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        tx.execute(
            &format!("DELETE FROM agents WHERE id NOT IN ({placeholders})"),
            rusqlite::params_from_iter(keep.iter()),
        )?;
    }
    for agent in agents {
        upsert_agent_tx(&tx, agent)?;
    }
    tx.commit()?;
    Ok(())
}
