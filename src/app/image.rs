use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    app::{AppState, agent_executor::AgentRunEvent},
    domain::{ChatMessage, ChatMessageStatus, ChatRole, ImageAttachment, ImageSource},
    error::{AppError, AppResult},
    llm::{client::RequestOptions, image::ImageLogContext},
};

const MAX_PROMPT_BYTES: usize = 100 * 1024;
const MAX_REFERENCE_IMAGES: usize = 8;
const MAX_REFERENCE_BYTES: usize = 32 * 1024 * 1024;

#[allow(clippy::too_many_arguments)]
pub async fn run_image(
    state: AppState,
    conversation_id: Uuid,
    user_message_id: Uuid,
    content: String,
    attachments: Vec<ImageAttachment>,
    restart_from_message_id: Option<Uuid>,
    options: RequestOptions,
    events: Option<mpsc::Sender<AgentRunEvent>>,
) -> AppResult<()> {
    let conversation = state.store.get_conversation(conversation_id).await?;
    let agent = state.store.get_agent(conversation.agent_id).await?;
    let provider = state
        .store
        .get_model_provider(agent.model_provider_id)
        .await?;
    if provider.provider_type != crate::domain::ProviderType::OpenAiImages {
        return Err(AppError::BadRequest(
            "image mode requires an OpenAI Images provider".to_string(),
        ));
    }
    let prompt = content.trim().to_string();
    if prompt.is_empty() {
        return Err(AppError::BadRequest(
            "image prompt cannot be empty".to_string(),
        ));
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(AppError::BadRequest(
            "image prompt is too large".to_string(),
        ));
    }
    validate_references(&attachments)?;
    if let Some(message_id) = restart_from_message_id {
        state
            .store
            .truncate_conversation_from_message(conversation_id, message_id)
            .await?;
    }

    let user_message = ChatMessage {
        id: user_message_id,
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::User,
        content: prompt.clone(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: attachments.clone(),
        usage: None,
    };
    state
        .store
        .append_conversation_messages(conversation_id, std::slice::from_ref(&user_message))
        .await?;
    emit(&events, AgentRunEvent::User(user_message)).await?;

    let image_client = state.llm_registry.image_client_for(&provider)?;
    let image_options = provider
        .model_configs
        .iter()
        .find(|config| config.model_id == agent.model_id)
        .and_then(|config| config.image_options.as_ref())
        .map(crate::llm::image::ImageRequestOptions::from)
        .unwrap_or_default();
    let images = image_client
        .generate(
            &provider,
            &agent.model_id,
            &prompt,
            &attachments,
            options,
            image_options,
            Some(ImageLogContext {
                agent_name: agent.name.clone(),
                agent_id: Some(agent.id.to_string()),
                conversation_id: Some(conversation_id.to_string()),
                model_id: agent.model_id.clone(),
                provider_name: provider.name.clone(),
            }),
        )
        .await?;

    let generated_dir = state
        .store
        .attachments_dir
        .join("generated")
        .join(conversation_id.to_string());
    tokio::fs::create_dir_all(&generated_dir).await?;
    let mut generated_attachments = Vec::with_capacity(images.len());
    for image in images {
        let id = Uuid::new_v4().to_string();
        let extension = extension_for_media_type(&image.media_type);
        let path = generated_dir.join(format!("{id}.{extension}"));
        tokio::fs::write(&path, image.bytes).await?;
        let path = tokio::fs::canonicalize(&path).await?;
        generated_attachments.push(ImageAttachment {
            id: id.clone(),
            name: format!("generated-{id}.{extension}"),
            media_type: image.media_type,
            source: ImageSource::Local {
                path: path.to_string_lossy().into_owned(),
            },
        });
    }

    let assistant = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id,
        role: ChatRole::Assistant,
        content: String::new(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: generated_attachments,
        usage: None,
    };
    state
        .store
        .upsert_conversation_message(conversation_id, assistant.clone())
        .await?;
    emit(&events, AgentRunEvent::Assistant(assistant)).await?;
    emit(&events, AgentRunEvent::Done).await
}

fn validate_references(attachments: &[ImageAttachment]) -> AppResult<()> {
    if attachments.len() > MAX_REFERENCE_IMAGES {
        return Err(AppError::BadRequest(format!(
            "a request can contain at most {MAX_REFERENCE_IMAGES} reference images"
        )));
    }
    let mut total = 0_usize;
    for attachment in attachments {
        if !matches!(
            attachment.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            return Err(AppError::BadRequest(format!(
                "unsupported reference image type {}",
                attachment.media_type
            )));
        }
        if let ImageSource::Base64 { data } = &attachment.source {
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
                .map_err(|_| {
                    AppError::BadRequest("reference image contains invalid Base64".to_string())
                })?;
            total = total.saturating_add(bytes.len());
        }
    }
    if total > MAX_REFERENCE_BYTES {
        return Err(AppError::BadRequest(
            "reference images exceed the 32 MB total limit".to_string(),
        ));
    }
    Ok(())
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

async fn emit(events: &Option<mpsc::Sender<AgentRunEvent>>, event: AgentRunEvent) -> AppResult<()> {
    if let Some(events) = events {
        events
            .send(event)
            .await
            .map_err(|_| AppError::Llm("image run client disconnected".to_string()))?;
    }
    Ok(())
}
