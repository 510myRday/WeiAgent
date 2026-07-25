use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        Agent, AgentToolsConfig, ChatMessage, ChatMessageStatus, ChatRole, ImageAttachment,
        ImageSource, ModelProvider, ThinkingLevel,
    },
    error::{AppError, AppResult},
};

const MAX_USER_MESSAGE_BYTES: usize = 100 * 1024;
const MAX_SYSTEM_PROMPT_BYTES: usize = 512 * 1024;
const MAX_IMAGE_ATTACHMENTS: usize = 8;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

pub struct PreparedChat {
    pub agent: Agent,
    pub provider: ModelProvider,
    pub user_message: ChatMessage,
    pub messages_for_llm: Vec<ChatMessage>,
    pub system_prompt: String,
    pub tools_config: AgentToolsConfig,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Agent tools config with runtime availability applied: the terminal tool is
/// forced off while the codex runtime is not installed/reachable.
pub(crate) async fn effective_tools_config(
    state: &AppState,
    agent_id: Uuid,
) -> AppResult<AgentToolsConfig> {
    let mut config = state.store.read_agent_tools_config(agent_id).await?;
    if config.terminal.enabled && !state.codex_runtime.is_available_cached() {
        config.terminal.enabled = false;
    }
    Ok(config)
}

pub async fn prepare_chat(
    state: &AppState,
    conversation_id: Uuid,
    content: &str,
    attachments: &[ImageAttachment],
    thinking_level: ThinkingLevel,
    restart_from_message_id: Option<Uuid>,
) -> AppResult<PreparedChat> {
    let content = content.trim();
    if content.is_empty() && attachments.is_empty() {
        return Err(AppError::BadRequest(
            "message content and attachments cannot both be empty".to_string(),
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
    validate_attachments(attachments)?;
    let model_config = provider
        .model_configs
        .iter()
        .find(|config| config.model_id == agent.model_id);
    if !attachments.is_empty() && !model_config.is_some_and(|config| config.multimodal) {
        return Err(AppError::BadRequest(
            "the selected model is not configured for multimodal image input".to_string(),
        ));
    }
    // Off means "do not enable thinking": omit all thinking params, same as models
    // that do not support thinking. Only non-Off levels are forwarded.
    let thinking_level = if model_config.is_some_and(|config| config.thinking)
        && thinking_level != ThinkingLevel::Off
    {
        Some(thinking_level)
    } else {
        None
    };
    let tools_config = effective_tools_config(state, agent.id).await?;
    let system_prompt = state
        .store
        .build_agent_system_prompt(&agent, &tools_config)
        .await?;
    if system_prompt.len() > MAX_SYSTEM_PROMPT_BYTES {
        return Err(AppError::BadRequest(format!(
            "agent system prompt exceeds {MAX_SYSTEM_PROMPT_BYTES} bytes"
        )));
    }
    if let Some(message_id) = restart_from_message_id {
        state
            .store
            .truncate_conversation_from_message(conversation_id, message_id)
            .await?;
    }
    let history = state
        .store
        .read_conversation_messages(conversation_id)
        .await?;
    let checkpoint = state.store.read_context_checkpoint(conversation_id).await?;
    let history_for_llm =
        crate::app::context_compress::build_llm_messages(&history, checkpoint.as_ref());
    let user_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::User,
        content: content.to_string(),
        reasoning_content: None,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: attachments.to_vec(),
        usage: None,
    };
    let mut messages_for_llm = history_for_llm;
    messages_for_llm.push(user_message.clone());

    Ok(PreparedChat {
        agent,
        provider,
        user_message,
        messages_for_llm,
        system_prompt,
        tools_config,
        thinking_level,
    })
}

fn validate_attachments(attachments: &[ImageAttachment]) -> AppResult<()> {
    if attachments.len() > MAX_IMAGE_ATTACHMENTS {
        return Err(AppError::BadRequest(format!(
            "a message can contain at most {MAX_IMAGE_ATTACHMENTS} images"
        )));
    }
    let mut total_bytes = 0_usize;
    for attachment in attachments {
        if !SUPPORTED_IMAGE_TYPES.contains(&attachment.media_type.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unsupported image type {}",
                attachment.media_type
            )));
        }
        match &attachment.source {
            ImageSource::Base64 { data } => {
                let bytes = STANDARD.decode(data).map_err(|_| {
                    AppError::BadRequest(format!(
                        "image attachment {} contains invalid base64 data",
                        attachment.name
                    ))
                })?;
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(AppError::BadRequest(format!(
                        "image attachment {} exceeds {MAX_IMAGE_BYTES} bytes",
                        attachment.name
                    )));
                }
                total_bytes = total_bytes.saturating_add(bytes.len());
            }
            ImageSource::Url { url } => {
                let parsed = reqwest::Url::parse(url).map_err(|_| {
                    AppError::BadRequest(format!(
                        "image attachment {} contains an invalid URL",
                        attachment.name
                    ))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(AppError::BadRequest(
                        "image URLs must use http or https".to_string(),
                    ));
                }
            }
        }
    }
    if total_bytes > MAX_TOTAL_IMAGE_BYTES {
        return Err(AppError::BadRequest(format!(
            "image attachments exceed {MAX_TOTAL_IMAGE_BYTES} total bytes"
        )));
    }
    Ok(())
}
