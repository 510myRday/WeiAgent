use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use image::{ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType};
use std::io::Cursor;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        Agent, AgentToolsConfig, ChatMessage, ChatMessageStatus, ChatRole, ImageAttachment,
        ImageSource, ModelProvider, SkillManifest, ThinkingLevel,
    },
    error::{AppError, AppResult},
    fsutil,
    storage::BUILTIN_WEIAGENT_CLI_SKILL_NAME,
};

const MAX_USER_MESSAGE_BYTES: usize = 100 * 1024;
const MAX_SYSTEM_PROMPT_BYTES: usize = 512 * 1024;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 512;
const MAX_IMAGE_ATTACHMENTS: usize = 8;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 32 * 1024 * 1024;
// Aggregate model input is governed by context compaction; this remains a
// per-image outbound guard after resizing and JPEG compression.
const MAX_LLM_IMAGE_BYTES: usize = 1_500_000;
const MAX_LLM_IMAGE_DIMENSION: u32 = 1536;
const LLM_IMAGE_JPEG_QUALITY: u8 = 78;
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

pub struct PreparedChat {
    pub agent: Agent,
    pub provider: ModelProvider,
    pub user_message: ChatMessage,
    pub messages_for_llm: Vec<ChatMessage>,
    pub system_prompt: String,
    pub tools_config: AgentToolsConfig,
    pub skill_snapshot: Vec<SkillManifest>,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Agent tools config loaded from the persisted defaults and agent overrides.
pub(crate) async fn effective_tools_config(
    state: &AppState,
    agent_id: Uuid,
) -> AppResult<AgentToolsConfig> {
    state.store.read_agent_tools_config(agent_id).await
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
    state.mcp_manager.prepare_agent(agent.id).await?;
    let skill_settings = state.skill_manager.settings(Some(agent.id)).await?;
    let mut tools_config = tools_config;
    for skill in &skill_settings.effective {
        let root = std::path::Path::new(&skill.root_dir);
        if let Ok(canonical) = std::fs::canonicalize(root) {
            let value = fsutil::display_path(&canonical);
            if !tools_config
                .terminal
                .allowed_cwd_roots
                .iter()
                .any(|item| item == &value)
            {
                tools_config.terminal.allowed_cwd_roots.push(value);
            }
        }
    }
    let mut system_prompt = state
        .store
        .build_agent_system_prompt(&agent, &tools_config)
        .await?;
    if !skill_settings.effective.is_empty() {
        system_prompt.push_str("\n\n## Available Skills (catalog)\n\n");
        system_prompt.push_str(
            "The entries below are Skill metadata, not loaded Skill instructions. When a Skill is relevant, use skill_view to load its instructions for the current task.\n\n<available_skills>\n",
        );
        for skill in &skill_settings.effective {
            let description = compact_skill_description(&skill.description);
            if let Some(group) = &skill.group {
                system_prompt.push_str(&format!("- {} [{}]: {}\n", skill.name, group, description));
            } else {
                system_prompt.push_str(&format!("- {}: {}\n", skill.name, description));
            }
        }
        system_prompt.push_str("</available_skills>\n");
    }
    if skill_settings
        .effective
        .iter()
        .any(|skill| skill.name == BUILTIN_WEIAGENT_CLI_SKILL_NAME && skill.is_builtin)
    {
        system_prompt.push_str("\n\n## WeiAgent Service Context\n\n");
        let service_context = format!(
            "The current runtime is the `weiagent` intelligent agent service. When the user describes an operation without naming another target, interpret it as an operation on this running service. Current agent ID: {}. Current conversation ID: {}. For detailed commands and workflows, load the `{BUILTIN_WEIAGENT_CLI_SKILL_NAME}` Skill with skill_view when it is relevant; its full body is task-specific guidance, not a permanent system instruction. Use a service-first installation workflow: when the user asks to install or download a Skill, run `weiagent skill install SOURCE`, then verify the Skill in this running service. When the user asks to install or add an MCP server, configure it in this running service with `weiagent mcp add --file FILE` or the matching CLI command, then run the relevant test or reload and verify the server. Treat downloaded Skill sources, MCP repositories, and configuration files as preparation; completion means the resource is installed and configured in the current WeiAgent service.",
            agent.id, conversation_id
        );
        system_prompt.push_str(&service_context);
    }
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
    // A previous run may have persisted an assistant tool call before the
    // result was written. Complete that protocol pair before building context.
    state
        .store
        .repair_incomplete_tool_calls(conversation_id)
        .await?;
    let history = state
        .store
        .read_conversation_messages(conversation_id)
        .await?;
    let history = normalize_history_for_llm(
        history,
        model_config.is_some_and(|config| config.multimodal),
    )
    .await?;
    let checkpoint = state.store.read_context_checkpoint(conversation_id).await?;
    let active_task_plan = state.store.read_task_plan(conversation_id).await?;
    let active_task_plan_context = active_task_plan
        .as_ref()
        .map(crate::app::context_compress::ActiveTaskPlanContext::from_task_plan);
    let history_for_llm = crate::app::context_compress::build_llm_messages(
        &history,
        checkpoint.as_ref(),
        active_task_plan_context.as_ref(),
    );
    let user_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::User,
        content: content.to_string(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: attachments.to_vec(),
        usage: None,
    };
    let mut llm_attachments = attachments.to_vec();
    normalize_attachments_for_llm(&mut llm_attachments).await?;
    let mut llm_user_message = user_message.clone();
    llm_user_message.attachments = llm_attachments;
    let mut messages_for_llm = history_for_llm;
    if let Some((skill_name, task)) = parse_skill_command(content)
        && skill_settings
            .defaults
            .iter()
            .any(|skill| skill.name == skill_name)
    {
        let selected = skill_settings
            .effective
            .iter()
            .find(|skill| skill.name == skill_name)
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "Skill '{skill_name}' is not enabled for this Agent"
                ))
            })?;
        let skill_content = state
            .skill_manager
            .content_from_manifest(selected, None)
            .await?
            .content;
        messages_for_llm.push(ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id,
            role: ChatRole::User,
            content: format!("The user explicitly selected the '{skill_name}' Skill. Follow this Skill for the current task only:\n\n{skill_content}\n\nCurrent task:\n{task}"),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        });
    }
    messages_for_llm.push(llm_user_message);

    Ok(PreparedChat {
        agent,
        provider,
        user_message,
        messages_for_llm,
        system_prompt,
        tools_config,
        skill_snapshot: skill_settings.effective,
        thinking_level,
    })
}

fn parse_skill_command(content: &str) -> Option<(String, String)> {
    let content = content.trim();
    let rest = content.strip_prefix('/')?;
    let boundary = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = rest[..boundary].trim().to_ascii_lowercase();
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return None;
    }
    let task = rest[boundary..].trim().to_string();
    Some((name, task))
}

fn compact_skill_description(description: &str) -> String {
    let compact = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let truncated = chars
        .by_ref()
        .take(MAX_SKILL_DESCRIPTION_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{truncated} [truncated]")
    } else {
        truncated
    }
}

async fn normalize_history_for_llm(
    history: Vec<ChatMessage>,
    supports_multimodal: bool,
) -> AppResult<Vec<ChatMessage>> {
    if history
        .iter()
        .any(|message| !message.attachments.is_empty())
        && !supports_multimodal
    {
        return Err(AppError::BadRequest(
            "the selected model is not configured for multimodal image context".to_string(),
        ));
    }

    let mut normalized = Vec::with_capacity(history.len());
    for mut message in history {
        if message.role == ChatRole::Tool {
            message.content = normalize_stored_tool_result_for_llm(&message.content);
        }
        normalize_attachments_for_llm(&mut message.attachments).await?;

        if message.role == ChatRole::Assistant
            && !message.attachments.is_empty()
            && message.tool_calls.is_none()
        {
            let attachments = std::mem::take(&mut message.attachments);
            let has_assistant_content = !message.content.is_empty()
                || message
                    .reasoning_content
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                || !message.openai_response_items.is_empty();
            if has_assistant_content {
                normalized.push(message.clone());
            }
            normalized.push(ChatMessage {
                // Keep the persisted assistant id so context checkpoints remain stable
                // across turns after the image is represented as a user input.
                id: message.id,
                agent_id: message.agent_id,
                conversation_id: message.conversation_id,
                role: ChatRole::User,
                content: String::new(),
                reasoning_content: None,
                openai_response_items: Vec::new(),
                status: ChatMessageStatus::Done,
                created_at: message.created_at,
                tool_calls: None,
                tool_call_id: None,
                attachments,
                usage: None,
            });
        } else {
            normalized.push(message);
        }
    }
    Ok(normalized)
}

/// Normalize tool messages written by older runs before they enter a new
/// request. This keeps legacy volatile metadata from breaking the reusable
/// prompt prefix after a restart.
fn normalize_stored_tool_result_for_llm(content: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(content) else {
        return content.to_string();
    };
    let Some(object) = value.as_object_mut() else {
        return content.to_string();
    };
    match object.get("status").and_then(serde_json::Value::as_str) {
        Some("completed") => {
            object.remove("duration_ms");
            object.remove("output_cursor");
        }
        Some("running") => {
            object.remove("duration_ms");
        }
        Some("pending_approval") => {
            object.remove("approval_id");
        }
        _ => {}
    }
    serde_json::to_string(&value).unwrap_or_else(|_| content.to_string())
}

#[cfg(test)]
mod tool_result_tests {
    use super::normalize_stored_tool_result_for_llm;

    #[test]
    fn legacy_completed_metadata_is_normalized() {
        let normalized = normalize_stored_tool_result_for_llm(
            r#"{"status":"completed","stdout":"ok","duration_ms":17,"output_cursor":2}"#,
        );
        assert_eq!(normalized, r#"{"status":"completed","stdout":"ok"}"#);
    }

    #[test]
    fn legacy_pending_approval_id_is_normalized() {
        let normalized = normalize_stored_tool_result_for_llm(
            r#"{"status":"pending_approval","command":"COMMAND","approval_id":"APPROVAL"}"#,
        );
        let value: serde_json::Value = serde_json::from_str(&normalized).unwrap();
        assert_eq!(value["status"], "pending_approval");
        assert_eq!(value["command"], "COMMAND");
        assert!(value.get("approval_id").is_none());
    }
}

#[cfg(test)]
mod skill_command_tests {
    use super::parse_skill_command;

    #[test]
    fn parses_explicit_skill_commands_only_at_message_start() {
        assert_eq!(
            parse_skill_command("/rust-review check this"),
            Some(("rust-review".to_string(), "check this".to_string()))
        );
        assert_eq!(
            parse_skill_command("/rust-review"),
            Some(("rust-review".to_string(), String::new()))
        );
        assert_eq!(parse_skill_command("please /rust-review check"), None);
        assert_eq!(
            parse_skill_command("/rust-reviewer check"),
            Some(("rust-reviewer".to_string(), "check".to_string()))
        );
    }
}

#[cfg(test)]
mod skill_prompt_tests {
    use super::compact_skill_description;

    #[test]
    fn compacts_and_bounds_skill_descriptions() {
        let description = "first line\nsecond line";
        assert_eq!(
            compact_skill_description(description),
            "first line second line"
        );

        let long = "x".repeat(600);
        let compact = compact_skill_description(&long);
        assert!(compact.ends_with(" [truncated]"));
        assert!(compact.len() < long.len());
    }
}

async fn normalize_attachments_for_llm(attachments: &mut [ImageAttachment]) -> AppResult<()> {
    for attachment in attachments {
        let bytes = match &attachment.source {
            ImageSource::Base64 { data } => STANDARD.decode(data).map_err(|_| {
                AppError::BadRequest(format!(
                    "image attachment {} contains invalid Base64 data",
                    attachment.name
                ))
            })?,
            ImageSource::Local { path } => tokio::fs::read(path).await.map_err(|error| {
                AppError::BadRequest(format!(
                    "historical image {} could not be read: {error}",
                    attachment.name
                ))
            })?,
            ImageSource::Url { .. } => continue,
        };
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(AppError::BadRequest(format!(
                "image attachment {} exceeds {MAX_IMAGE_BYTES} bytes",
                attachment.name
            )));
        }
        let (media_type, data) = compact_image_for_llm(&bytes, &attachment.name)?;
        attachment.media_type = media_type;
        attachment.source = ImageSource::Base64 { data };
    }
    Ok(())
}

fn compact_image_for_llm(bytes: &[u8], name: &str) -> AppResult<(String, String)> {
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| {
            AppError::BadRequest(format!("image attachment {name} format error: {error}"))
        })?
        .decode()
        .map_err(|error| {
            AppError::BadRequest(format!("image attachment {name} decode failed: {error}"))
        })?;
    if bytes.len() <= MAX_LLM_IMAGE_BYTES
        && image.width() <= MAX_LLM_IMAGE_DIMENSION
        && image.height() <= MAX_LLM_IMAGE_DIMENSION
    {
        return Ok((
            detect_image_media_type(bytes).unwrap_or_else(|| "image/png".to_string()),
            STANDARD.encode(bytes),
        ));
    }

    let mut resized = image.thumbnail(MAX_LLM_IMAGE_DIMENSION, MAX_LLM_IMAGE_DIMENSION);
    loop {
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, LLM_IMAGE_JPEG_QUALITY)
            .encode_image(&resized)
            .map_err(|error| {
                AppError::BadRequest(format!("image attachment {name} encode failed: {error}"))
            })?;
        if encoded.len() <= MAX_LLM_IMAGE_BYTES {
            return Ok(("image/jpeg".to_string(), STANDARD.encode(encoded)));
        }
        let next_width = (resized.width() * 3 / 4).max(1);
        let next_height = (resized.height() * 3 / 4).max(1);
        if next_width == resized.width() && next_height == resized.height() {
            return Err(AppError::BadRequest(format!(
                "image attachment {name} remains too large after optimization"
            )));
        }
        resized = resized.resize(next_width, next_height, FilterType::Triangle);
    }
}

fn detect_image_media_type(bytes: &[u8]) -> Option<String> {
    crate::image_util::detect_image_media_type(bytes).map(str::to_string)
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
            ImageSource::Local { .. } => {
                return Err(AppError::BadRequest(
                    "local generated images cannot be used as user attachments".to_string(),
                ));
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

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;

    fn assistant_with_attachment(attachment: ImageAttachment) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            role: ChatRole::Assistant,
            content: String::new(),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: vec![attachment],
            usage: None,
        }
    }

    fn image_attachment(source: ImageSource) -> ImageAttachment {
        ImageAttachment {
            id: "generated-image".to_string(),
            name: "generated.png".to_string(),
            media_type: "image/png".to_string(),
            is_mask: false,
            source,
        }
    }

    fn png_data() -> String {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("test PNG should encode");
        STANDARD.encode(bytes.into_inner())
    }

    #[tokio::test]
    async fn normalizes_generated_assistant_image_as_user_context() {
        let message =
            assistant_with_attachment(image_attachment(ImageSource::Base64 { data: png_data() }));
        let message_id = message.id;

        let normalized = normalize_history_for_llm(vec![message], true)
            .await
            .expect("multimodal history should be accepted");

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].id, message_id);
        assert_eq!(normalized[0].role, ChatRole::User);
        assert!(normalized[0].content.is_empty());
        assert_eq!(normalized[0].attachments.len(), 1);
    }

    #[tokio::test]
    async fn rejects_image_history_for_text_only_model() {
        let history = vec![assistant_with_attachment(image_attachment(
            ImageSource::Base64 { data: png_data() },
        ))];

        let error = normalize_history_for_llm(history, false)
            .await
            .expect_err("text-only model should reject image context");

        assert!(error.to_string().contains("multimodal image context"));
    }
}
