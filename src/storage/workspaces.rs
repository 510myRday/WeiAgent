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

    pub async fn build_agent_system_prompt(&self, agent: &Agent) -> AppResult<String> {
        let workspace = self.read_agent_workspace(agent.id).await?;
        Ok(join_prompt_sections([
            ("Personality", workspace.personality.as_str()),
            ("Memory", workspace.memory.as_str()),
            ("Instructions", workspace.instructions.as_str()),
        ]))
    }

    pub(super) async fn ensure_builtin_agents(&self) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut agents = self.read_agents_unlocked().await?;
        let now = Utc::now();

        let existing_builtin = agents.iter().position(|agent| agent.is_builtin);

        if let Some(index) = existing_builtin {
            agents[index].name = BUILTIN_MASTER_NAME.to_string();
            self.ensure_agent_workspace_unlocked(&agents[index]).await?;
        } else {
            let agent = Agent {
                id: Uuid::new_v4(),
                name: BUILTIN_MASTER_NAME.to_string(),
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
        ensure_text_file(&workspace.join(WORKSPACE_TOOLS), "[]\n".to_string()).await?;
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
        fs::write(workspace.join(file_name), content).await?;
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
