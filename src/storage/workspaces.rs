use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{Agent, AgentWorkspace, UpdateAgentWorkspaceRequest},
    error::AppResult,
    storage::{
        BUILTIN_CEO_COLOR, BUILTIN_CEO_ICON, WORKSPACE_INSTRUCTIONS, WORKSPACE_MEMORY,
        WORKSPACE_PERSONALITY, files::ensure_text_file, json_store::JsonStore,
    },
};

const BUILTIN_MASTER_NAME: &str = "Master";
const BUILTIN_MASTER_DESCRIPTION: &str =
    "Coordinates tasks, delegates work, and drives complex jobs to completion.";
const LEGACY_AGENT_DESCRIPTION: &str = "Custom AI agent";
const WEB_SEARCH_POLICY_V1: &str = "## Web Search Policy V1\n\
Use web search when the user requests browsing, current information, source links, precise attribution, or information likely to have changed.\n\n\
Use fast for simple lookups, balanced for current facts and comparisons, and deep for comprehensive research, conflicting claims, or explicit deep investigation. For research, issue complementary query rewrites in one call, retry empty searches with materially different wording, and use multiple independent source groups.\n\n\
Use web_search_sources before custom source selection. Search APIs return evidence candidates. AI research syntheses are useful hypotheses and query directions, but are unverified and never count as independent evidence.\n\n\
Open the most relevant original pages with web_extract, prefer primary sources, and verify important or disputed claims with independent evidence. Cite the original page using a Markdown link placed next to the supported claim.";

impl JsonStore {
    pub async fn read_agent_workspace(&self, id: Uuid) -> AppResult<AgentWorkspace> {
        let _guard = self.lock.lock().await;
        if !self
            .read_agents_unlocked()
            .await?
            .iter()
            .any(|agent| agent.id == id)
        {
            return Err(crate::error::AppError::NotFound);
        }
        self.read_agent_workspace_unlocked(id).await
    }

    pub async fn update_agent_workspace(
        &self,
        id: Uuid,
        request: UpdateAgentWorkspaceRequest,
    ) -> AppResult<AgentWorkspace> {
        let _guard = self.lock.lock().await;
        if !self
            .read_agents_unlocked()
            .await?
            .iter()
            .any(|agent| agent.id == id)
        {
            return Err(crate::error::AppError::NotFound);
        }
        let workspace = AgentWorkspace {
            personality: request.personality,
            memory: request.memory,
            instructions: request.instructions,
        };
        self.write_agent_workspace_unlocked(id, &workspace).await?;
        self.read_agent_workspace_unlocked(id).await
    }

    pub async fn build_agent_system_prompt(
        &self,
        agent: &Agent,
        tools_config: &crate::domain::AgentToolsConfig,
    ) -> AppResult<String> {
        let workspace = self.read_agent_workspace(agent.id).await?;
        let environment = crate::tools::prompt::build_environment_prefix(
            &self.data_dir,
            &self.terminal_workspace_dir,
            &self.attachments_dir,
        );
        // Runtime identity: which agent is handling this turn.
        let agent_identity = format!(
            "## Agent\n\
当前处理本轮对话的智能体（接收用户消息、调用工具并回复）：\n\
- Name: {}\n\
- ID: {}",
            agent.name, agent.id
        );
        // Calendar date only (local TZ). Stable within a day for prompt cache;
        // intentionally not a timestamp. Worded as authoritative "today" so models
        // do not fall back to older years from training/knowledge cutoffs.
        let runtime_date = format!(
            "## Runtime\n\
本机本地时区的权威日历日（仅日期，不含时刻）。此为当前对话的“今天”，必须以此为准；不得沿用训练数据、知识截止日期或旧语境中更早的年份/日期假设。\n\
- Date: {}\n\
- Trust: absolute. If any prior belief conflicts with this date, discard that belief.",
            crate::tools::prompt::today_date()
        );
        let agent_sections = join_prompt_sections([
            ("Personality", workspace.personality.as_str()),
            ("Memory", workspace.memory.as_str()),
            ("Instructions", workspace.instructions.as_str()),
        ]);
        let web_search_policy = if tools_config.web_search.enabled {
            WEB_SEARCH_POLICY_V1
        } else {
            ""
        };
        Ok([
            environment.as_str(),
            agent_identity.as_str(),
            runtime_date.as_str(),
            web_search_policy,
            agent_sections.as_str(),
        ]
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n"))
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

    pub(super) async fn write_agent_workspace_unlocked(
        &self,
        id: Uuid,
        workspace: &AgentWorkspace,
    ) -> AppResult<()> {
        self.write_workspace_text_unlocked(id, WORKSPACE_PERSONALITY, &workspace.personality)
            .await?;
        self.write_workspace_text_unlocked(id, WORKSPACE_MEMORY, &workspace.memory)
            .await?;
        self.write_workspace_text_unlocked(id, WORKSPACE_INSTRUCTIONS, &workspace.instructions)
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
