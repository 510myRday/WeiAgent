use chrono::Utc;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{Agent, AgentToolsConfig, ChatMessage, ChatMessageStatus, ChatRole, ModelProvider},
    error::{AppError, AppResult},
};

const MAX_USER_MESSAGE_BYTES: usize = 100 * 1024;
const MAX_SYSTEM_PROMPT_BYTES: usize = 512 * 1024;

pub struct PreparedChat {
    pub agent: Agent,
    pub provider: ModelProvider,
    pub user_message: ChatMessage,
    pub messages_for_llm: Vec<ChatMessage>,
    pub system_prompt: String,
    pub tools_config: AgentToolsConfig,
}

pub async fn prepare_chat(
    state: &AppState,
    conversation_id: Uuid,
    content: &str,
) -> AppResult<PreparedChat> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::BadRequest(
            "message content cannot be empty".to_string(),
        ));
    }
    if content.len() > MAX_USER_MESSAGE_BYTES {
        return Err(AppError::BadRequest(format!(
            "message content exceeds {MAX_USER_MESSAGE_BYTES} bytes"
        )));
    }

    let conversation = state.store.get_conversation(conversation_id).await?;
    let agent = state.store.get_agent(conversation.agent_id).await?;
    let provider = state
        .store
        .get_model_provider(agent.model_provider_id)
        .await?;
    let history = state
        .store
        .read_conversation_messages(conversation_id)
        .await?;
    let user_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::User,
        content: content.to_string(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
    };
    let mut messages_for_llm = history;
    messages_for_llm.push(user_message.clone());
    let mut tools_config = state.store.read_agent_tools_config(agent.id).await?;
    if tools_config.terminal.enabled
        && tools_config.terminal.permission_mode
            == crate::domain::TerminalPermissionMode::RequestApproval
        && !state.sandbox_runtime.is_available_cached()
    {
        tools_config.terminal.enabled = false;
    }
    let system_prompt = state
        .store
        .build_agent_system_prompt(&agent, &tools_config)
        .await?;
    if system_prompt.len() > MAX_SYSTEM_PROMPT_BYTES {
        return Err(AppError::BadRequest(format!(
            "agent system prompt exceeds {MAX_SYSTEM_PROMPT_BYTES} bytes"
        )));
    }

    Ok(PreparedChat {
        agent,
        provider,
        user_message,
        messages_for_llm,
        system_prompt,
        tools_config,
    })
}
