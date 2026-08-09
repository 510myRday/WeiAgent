use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    app::{AppState, agent_executor::AgentRunEvent},
    domain::{
        Agent, ChatMessage, ChatMessageStatus, ChatRole, ImageAttachment, ImageSource,
        ModelProvider, ModelUsage, TokenUsage,
    },
    error::{AppError, AppResult},
    llm::{client::RequestOptions, image::ImageLogContext},
};

const MAX_PROMPT_BYTES: usize = 100 * 1024;
const MAX_REFERENCE_IMAGES: usize = 8;
const MAX_REFERENCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_GENERATED_TOTAL_BYTES: usize = 128 * 1024 * 1024;

#[allow(clippy::too_many_arguments)]
pub async fn run_image(
    state: AppState,
    conversation_id: Uuid,
    user_message_id: Uuid,
    agent: Agent,
    provider: ModelProvider,
    content: String,
    attachments: Vec<ImageAttachment>,
    restart_from_message_id: Option<Uuid>,
    options: RequestOptions,
    events: Option<mpsc::Sender<AgentRunEvent>>,
) -> AppResult<()> {
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
    validate_references(&attachments, options.timeout).await?;
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
    let generated = image_client
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
    let image_count = generated.images.len() as u32;
    let usage = image_usage(&provider, &agent.model_id, generated.usage, image_count);

    let mut total_generated_bytes = 0_usize;
    for image in &generated.images {
        if image.bytes.len() > MAX_GENERATED_IMAGE_BYTES {
            return Err(AppError::Llm(format!(
                "generated image exceeds {MAX_GENERATED_IMAGE_BYTES} bytes"
            )));
        }
        total_generated_bytes = total_generated_bytes.saturating_add(image.bytes.len());
    }
    if total_generated_bytes > MAX_GENERATED_TOTAL_BYTES {
        return Err(AppError::Llm(format!(
            "generated images exceed {MAX_GENERATED_TOTAL_BYTES} total bytes"
        )));
    }

    let generated_dir = state
        .store
        .attachments_dir
        .join("generated")
        .join(conversation_id.to_string());
    tokio::fs::create_dir_all(&generated_dir).await?;
    let mut generated_attachments = Vec::with_capacity(generated.images.len());
    let mut written_paths = Vec::with_capacity(generated.images.len());
    for image in generated.images {
        let id = Uuid::new_v4().to_string();
        let extension = extension_for_media_type(&image.media_type);
        let path = generated_dir.join(format!("{id}.{extension}"));
        if let Err(error) = crate::fsutil::write_atomic(&path, &image.bytes).await {
            cleanup_generated_paths(&written_paths).await;
            return Err(error);
        }
        written_paths.push(path.clone());
        let path = match tokio::fs::canonicalize(&path).await {
            Ok(path) => path,
            Err(error) => {
                cleanup_generated_paths(&written_paths).await;
                return Err(error.into());
            }
        };
        generated_attachments.push(ImageAttachment {
            id: id.clone(),
            name: format!("generated-{id}.{extension}"),
            media_type: image.media_type,
            is_mask: false,
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
        usage: Some(usage),
    };
    state
        .store
        .upsert_conversation_message(conversation_id, assistant.clone())
        .await?;
    emit(&events, AgentRunEvent::Assistant(assistant)).await?;
    emit(&events, AgentRunEvent::Done).await
}

async fn validate_references(
    attachments: &[ImageAttachment],
    timeout: std::time::Duration,
) -> AppResult<()> {
    if attachments.len() > MAX_REFERENCE_IMAGES {
        return Err(AppError::BadRequest(format!(
            "a request can contain at most {MAX_REFERENCE_IMAGES} reference images"
        )));
    }
    validate_reference_roles(attachments)?;
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
        } else if let ImageSource::Url { url } = &attachment.source {
            crate::llm::image::validate_remote_image_url(url, timeout).await?;
        } else if matches!(attachment.source, ImageSource::Local { .. }) {
            return Err(AppError::BadRequest(
                "local generated images cannot be used as reference uploads".to_string(),
            ));
        }
    }
    if total > MAX_REFERENCE_BYTES {
        return Err(AppError::BadRequest(
            "reference images exceed the 32 MB total limit".to_string(),
        ));
    }
    Ok(())
}

fn validate_reference_roles(attachments: &[ImageAttachment]) -> AppResult<()> {
    let masks = attachments
        .iter()
        .filter(|attachment| attachment.is_mask)
        .collect::<Vec<_>>();
    if masks.len() > 1 {
        return Err(AppError::BadRequest(
            "a request can contain at most one mask image".to_string(),
        ));
    }
    let Some(mask) = masks.first() else {
        return Ok(());
    };
    if mask.media_type != "image/png" {
        return Err(AppError::BadRequest(
            "the mask image must be a PNG".to_string(),
        ));
    }
    if !attachments.iter().any(|attachment| !attachment.is_mask) {
        return Err(AppError::BadRequest(
            "a mask image requires at least one source image".to_string(),
        ));
    }
    Ok(())
}

async fn cleanup_generated_paths(paths: &[std::path::PathBuf]) {
    for path in paths {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn image_usage(
    provider: &crate::domain::ModelProvider,
    model_id: &str,
    usage: Option<TokenUsage>,
    image_count: u32,
) -> ModelUsage {
    let has_usage = usage.is_some();
    let tokens = usage.unwrap_or_default().normalized();
    let config = provider
        .model_configs
        .iter()
        .find(|config| config.model_id == model_id);
    let cost_usd = has_usage.then(|| {
        config.map_or(0.0, |config| {
            tokens.input_tokens as f64 * config.input_price.unwrap_or(0.0) / 1_000_000.0
                + tokens.output_tokens as f64 * config.output_price.unwrap_or(0.0) / 1_000_000.0
        })
    });
    ModelUsage {
        total_tokens: tokens.total_tokens(),
        tokens,
        cost_usd,
        image_count: Some(image_count),
        provider_id: provider.id,
        model_id: model_id.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use crate::domain::{ImageAttachment, ImageSource};

    use super::validate_reference_roles;

    fn attachment(id: &str, media_type: &str, is_mask: bool) -> ImageAttachment {
        ImageAttachment {
            id: id.to_string(),
            name: format!("{id}.png"),
            media_type: media_type.to_string(),
            is_mask,
            source: ImageSource::Base64 {
                data: "AA==".to_string(),
            },
        }
    }

    #[test]
    fn validates_mask_reference_roles() {
        assert!(validate_reference_roles(&[attachment("mask", "image/png", true)]).is_err());
        assert!(
            validate_reference_roles(&[
                attachment("source", "image/png", false),
                attachment("mask-a", "image/png", true),
                attachment("mask-b", "image/png", true),
            ])
            .is_err()
        );
        assert!(
            validate_reference_roles(&[
                attachment("source", "image/png", false),
                attachment("mask", "image/jpeg", true),
            ])
            .is_err()
        );
        assert!(
            validate_reference_roles(&[
                attachment("source", "image/png", false),
                attachment("mask", "image/png", true),
            ])
            .is_ok()
        );
    }
}
