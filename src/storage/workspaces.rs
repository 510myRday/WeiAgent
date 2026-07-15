use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{Agent, AgentWorkspace, UpdateAgentWorkspaceRequest},
    error::AppResult,
    storage::{
        BUILTIN_CEO_COLOR, BUILTIN_CEO_ICON, WORKSPACE_INSTRUCTIONS, WORKSPACE_MEMORY,
        WORKSPACE_PERSONALITY, WORKSPACE_TOOLS, files::ensure_text_file, json_store::JsonStore,
    },
};

const BUILTIN_MASTER_NAME: &str = "Master";
const BUILTIN_MASTER_DESCRIPTION: &str =
    "Coordinates tasks, delegates work, and drives complex jobs to completion.";
const LEGACY_AGENT_DESCRIPTION: &str = "Custom AI agent";

impl JsonStore {
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

    pub async fn build_agent_system_prompt(
        &self,
        agent: &Agent,
        tools_config: &crate::domain::AgentToolsConfig,
    ) -> AppResult<String> {
        let workspace = self.read_agent_workspace(agent.id).await?;
        let environment =
            crate::tools::prompt::build_environment_prefix(tools_config, &self.data_dir);
        let agent_sections = join_prompt_sections([
            ("Personality", workspace.personality.as_str()),
            ("Memory", workspace.memory.as_str()),
            ("Instructions", workspace.instructions.as_str()),
        ]);
        if agent_sections.is_empty() {
            Ok(environment)
        } else {
            Ok(format!("{environment}\n\n{agent_sections}"))
        }
    }

    pub(super) async fn ensure_builtin_agents(&self) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut agents = self.read_agents_unlocked().await?;
        let now = Utc::now();

        for agent in &mut agents {
            if agent.description.trim().is_empty() {
                agent.description = if agent.is_builtin {
                    BUILTIN_MASTER_DESCRIPTION.to_string()
                } else {
                    LEGACY_AGENT_DESCRIPTION.to_string()
                };
            }
        }

        let existing_builtin = agents.iter().position(|agent| agent.is_builtin);

        if let Some(index) = existing_builtin {
            agents[index].name = BUILTIN_MASTER_NAME.to_string();
            agents[index].description = BUILTIN_MASTER_DESCRIPTION.to_string();
            self.ensure_agent_workspace_unlocked(&agents[index]).await?;
        } else {
            let agent = Agent {
                id: Uuid::new_v4(),
                name: BUILTIN_MASTER_NAME.to_string(),
                description: BUILTIN_MASTER_DESCRIPTION.to_string(),
                model_provider_id: Uuid::nil(),
                model_id: String::new(),
                is_builtin: true,
                avatar_icon: BUILTIN_CEO_ICON.to_string(),
                avatar_color: BUILTIN_CEO_COLOR.to_string(),
                created_at: now,
                updated_at: now,
            };
            self.ensure_agent_workspace_unlocked(&agent).await?;
            agents.push(agent);
        }

        self.write_agents_unlocked(&agents).await
    }

    pub(super) async fn read_agent_workspace_unlocked(
        &self,
        id: Uuid,
    ) -> AppResult<AgentWorkspace> {
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
        })
    }

    pub async fn read_tool_defaults(&self) -> AppResult<crate::domain::AgentToolsConfig> {
        let _guard = self.lock.lock().await;
        self.read_tool_defaults_unlocked().await
    }

    pub async fn update_tool_defaults(
        &self,
        mut config: crate::domain::AgentToolsConfig,
    ) -> AppResult<crate::domain::AgentToolsConfig> {
        if !config.terminal.approval_grants.is_empty() {
            return Err(crate::error::AppError::BadRequest(
                "global tool defaults cannot contain agent approval grants".to_string(),
            ));
        }
        config.terminal.approval_grants.clear();
        validate_tools_config(&config)?;
        let _guard = self.lock.lock().await;
        let content = serde_json::to_string_pretty(&config)
            .map_err(|error| crate::error::AppError::BadRequest(error.to_string()))?;
        crate::fsutil::write_atomic(&self.tool_defaults_file, content).await?;
        Ok(config)
    }

    pub async fn read_agent_tools_settings(
        &self,
        id: Uuid,
    ) -> AppResult<crate::domain::AgentToolsSettings> {
        self.get_agent(id).await?;
        let _guard = self.lock.lock().await;
        let defaults = self.read_tool_defaults_unlocked().await?;
        let overrides = self
            .read_agent_tools_overrides_unlocked(id, &defaults)
            .await?;
        let effective = overrides.apply_to(&defaults);
        Ok(crate::domain::AgentToolsSettings {
            defaults,
            overrides,
            effective,
        })
    }

    pub async fn read_agent_tools_config(
        &self,
        id: Uuid,
    ) -> AppResult<crate::domain::AgentToolsConfig> {
        Ok(self.read_agent_tools_settings(id).await?.effective)
    }

    pub async fn update_agent_tools_overrides(
        &self,
        id: Uuid,
        overrides: crate::domain::AgentToolsOverrides,
    ) -> AppResult<crate::domain::AgentToolsSettings> {
        self.get_agent(id).await?;
        let _guard = self.lock.lock().await;
        let defaults = self.read_tool_defaults_unlocked().await?;
        validate_tools_overrides(&overrides, &defaults)?;
        self.write_agent_tools_overrides_unlocked(id, &overrides)
            .await?;
        let effective = overrides.apply_to(&defaults);
        Ok(crate::domain::AgentToolsSettings {
            defaults,
            overrides,
            effective,
        })
    }

    #[cfg(test)]
    pub async fn update_agent_tools_config(
        &self,
        id: Uuid,
        config: crate::domain::AgentToolsConfig,
    ) -> AppResult<crate::domain::AgentToolsConfig> {
        self.get_agent(id).await?;
        validate_tools_config(&config)?;
        let _guard = self.lock.lock().await;
        let overrides = crate::domain::AgentToolsOverrides::full_override(&config);
        self.write_agent_tools_overrides_unlocked(id, &overrides)
            .await?;
        Ok(config)
    }

    pub async fn always_allow_terminal_command(
        &self,
        id: Uuid,
        command: &str,
        cwd: Option<&str>,
    ) -> AppResult<crate::domain::AgentToolsConfig> {
        self.get_agent(id).await?;
        let _guard = self.lock.lock().await;
        let defaults = self.read_tool_defaults_unlocked().await?;
        let mut overrides = self
            .read_agent_tools_overrides_unlocked(id, &defaults)
            .await?;
        let grant = crate::domain::TerminalApprovalGrant {
            command: command.to_string(),
            cwd: cwd.map(str::to_string),
            permission_mode: crate::domain::TerminalPermissionMode::RequestApproval,
            created_at: Some(chrono::Utc::now()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(24)),
        };
        overrides.terminal.approval_grants.retain(|existing| {
            existing
                .expires_at
                .is_some_and(|expires_at| expires_at > chrono::Utc::now())
        });
        if !overrides.terminal.approval_grants.iter().any(|existing| {
            existing.command == grant.command
                && existing.cwd == grant.cwd
                && existing.permission_mode == grant.permission_mode
                && existing
                    .expires_at
                    .is_some_and(|expires_at| expires_at > chrono::Utc::now())
        }) {
            overrides.terminal.approval_grants.push(grant);
        }
        validate_tools_overrides(&overrides, &defaults)?;
        self.write_agent_tools_overrides_unlocked(id, &overrides)
            .await?;
        Ok(overrides.apply_to(&defaults))
    }

    async fn read_tool_defaults_unlocked(&self) -> AppResult<crate::domain::AgentToolsConfig> {
        let content = fs::read_to_string(&self.tool_defaults_file).await?;
        let mut config: crate::domain::AgentToolsConfig = serde_json::from_str(content.trim())
            .map_err(|error| {
                crate::error::AppError::BadRequest(format!("invalid tool_defaults.json: {error}"))
            })?;
        if !config.terminal.approval_grants.is_empty() {
            return Err(crate::error::AppError::BadRequest(
                "tool_defaults.json cannot contain agent approval grants".to_string(),
            ));
        }
        config.terminal.approval_grants.clear();
        validate_tools_config(&config)?;
        Ok(config)
    }

    async fn read_agent_tools_overrides_unlocked(
        &self,
        id: Uuid,
        defaults: &crate::domain::AgentToolsConfig,
    ) -> AppResult<crate::domain::AgentToolsOverrides> {
        let path = self.agent_workspace_dir(id).join(WORKSPACE_TOOLS);
        if fs::metadata(&path).await.is_err() {
            return Ok(crate::domain::AgentToolsOverrides::default());
        }
        let content = fs::read_to_string(&path).await?;
        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed == "[]" {
            return Ok(crate::domain::AgentToolsOverrides::default());
        }
        let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|error| {
            crate::error::AppError::BadRequest(format!("invalid tools.json: {error}"))
        })?;
        let overrides = if value.get("version").is_some() {
            serde_json::from_value::<crate::domain::AgentToolsOverrides>(value).map_err(
                |error| crate::error::AppError::BadRequest(format!("invalid tools.json: {error}")),
            )?
        } else {
            let legacy = serde_json::from_value::<crate::domain::AgentToolsConfig>(value).map_err(
                |error| {
                    crate::error::AppError::BadRequest(format!(
                        "invalid legacy tools.json: {error}"
                    ))
                },
            )?;
            let migrated = crate::domain::AgentToolsOverrides::from_full_config(&legacy, defaults);
            self.write_agent_tools_overrides_unlocked(id, &migrated)
                .await?;
            migrated
        };
        validate_tools_overrides(&overrides, defaults)?;
        Ok(overrides)
    }

    async fn write_agent_tools_overrides_unlocked(
        &self,
        id: Uuid,
        overrides: &crate::domain::AgentToolsOverrides,
    ) -> AppResult<()> {
        let content = serde_json::to_string_pretty(overrides)
            .map_err(|error| crate::error::AppError::BadRequest(error.to_string()))?;
        self.write_workspace_text_unlocked(id, WORKSPACE_TOOLS, &content)
            .await
    }

    pub(super) async fn ensure_agent_workspace_unlocked(&self, agent: &Agent) -> AppResult<()> {
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
        let default_tools =
            serde_json::to_string_pretty(&crate::domain::AgentToolsOverrides::default())
                .unwrap_or_else(|_| "{}".to_string());
        ensure_text_file(&workspace.join(WORKSPACE_TOOLS), default_tools).await?;
        Ok(())
    }

    pub(super) async fn read_workspace_text_unlocked(
        &self,
        id: Uuid,
        file_name: &str,
    ) -> AppResult<String> {
        let path = self.agent_workspace_dir(id).join(file_name);
        if fs::metadata(&path).await.is_err() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(path).await?)
    }

    pub(super) async fn write_workspace_text_unlocked(
        &self,
        id: Uuid,
        file_name: &str,
        content: &str,
    ) -> AppResult<()> {
        let workspace = self.agent_workspace_dir(id);
        fs::create_dir_all(&workspace).await?;
        crate::fsutil::write_atomic(&workspace.join(file_name), content).await?;
        Ok(())
    }
}

fn default_personality(agent: &Agent) -> String {
    if agent.is_builtin {
        return "# Personality\n\n你沉稳、负责、主动澄清目标，像 CEO 一样帮助用户做判断、拆解任务、排序优先级，并把复杂事项推进到可执行状态。\n".to_string();
    }
    "# Personality\n\n记录这个 Agent 的个性、人设、沟通风格和长期稳定的行为偏好。\n".to_string()
}

fn default_memory() -> String {
    "# Memory\n\n这里用于记录用户偏好、长期项目背景和重要决策。初始阶段不要编造事实，只在用户明确提供或后续系统沉淀时补充。\n".to_string()
}

fn default_instructions(agent: &Agent) -> String {
    if agent.is_builtin {
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

fn validate_tools_config(config: &crate::domain::AgentToolsConfig) -> AppResult<()> {
    const MAX_ALLOWED_ROOTS: usize = 8;
    if config.max_llm_iterations == 0
        || config.max_llm_iterations > crate::domain::tools::MAX_LLM_ITERATIONS
    {
        return Err(crate::error::AppError::BadRequest(format!(
            "max_llm_iterations must be between 1 and {}",
            crate::domain::tools::MAX_LLM_ITERATIONS
        )));
    }
    if config.terminal.default_timeout_ms == 0
        || config.terminal.default_timeout_ms > crate::tools::terminal::MAX_TERMINAL_TIMEOUT_MS
    {
        return Err(crate::error::AppError::BadRequest(format!(
            "terminal default_timeout_ms must be between 1 and {}",
            crate::tools::terminal::MAX_TERMINAL_TIMEOUT_MS
        )));
    }
    if config.terminal.allowed_cwd_roots.len() > MAX_ALLOWED_ROOTS {
        return Err(crate::error::AppError::BadRequest(format!(
            "at most {MAX_ALLOWED_ROOTS} terminal workspace roots may be configured"
        )));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .and_then(|path| std::fs::canonicalize(path).ok());
    let mut canonical_roots = std::collections::HashSet::new();
    for root in &config.terminal.allowed_cwd_roots {
        let path = std::fs::canonicalize(root).map_err(|error| {
            crate::error::AppError::BadRequest(format!(
                "terminal workspace root '{root}' is invalid: {error}"
            ))
        })?;
        if path.parent().is_none() {
            return Err(crate::error::AppError::BadRequest(
                "filesystem roots cannot be used as terminal workspace roots".to_string(),
            ));
        }
        if home
            .as_ref()
            .is_some_and(|home| crate::fsutil::paths_equal(home, &path))
        {
            return Err(crate::error::AppError::BadRequest(
                "the entire user home directory cannot be used as a terminal workspace root"
                    .to_string(),
            ));
        }
        if !canonical_roots.insert(crate::fsutil::path_identity_key(&path)) {
            return Err(crate::error::AppError::BadRequest(
                "terminal workspace roots must be unique".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_tools_overrides(
    overrides: &crate::domain::AgentToolsOverrides,
    defaults: &crate::domain::AgentToolsConfig,
) -> AppResult<()> {
    if overrides.version != 1 {
        return Err(crate::error::AppError::BadRequest(format!(
            "unsupported tools.json version {}",
            overrides.version
        )));
    }
    validate_tools_config(&overrides.apply_to(defaults))
}
