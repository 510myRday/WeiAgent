use uuid::Uuid;

use crate::{
    domain::{Agent, ModelProvider},
    error::{AppError, AppResult},
    storage::{DEFAULT_AGENT_COLOR, DEFAULT_AGENT_ICON},
};

pub(super) fn validate_non_empty(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{field} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

pub(super) fn normalize_models(models: Vec<String>) -> AppResult<Vec<String>> {
    let mut models = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(AppError::BadRequest(
            "models must contain at least one model".to_string(),
        ));
    }
    Ok(models)
}

pub(super) fn validate_agent_text(name: &str, model_id: &str) -> AppResult<()> {
    validate_non_empty("agent name", name)?;
    validate_non_empty("model_id", model_id)?;
    Ok(())
}

pub(super) fn normalize_avatar_icon(value: Option<&str>) -> AppResult<String> {
    let icon = value.unwrap_or(DEFAULT_AGENT_ICON).trim();
    if icon.is_empty() {
        return Err(AppError::BadRequest(
            "avatar_icon cannot be empty".to_string(),
        ));
    }
    Ok(icon.to_string())
}

pub(super) fn normalize_avatar_color(value: Option<&str>) -> AppResult<String> {
    let color = value.unwrap_or(DEFAULT_AGENT_COLOR).trim();
    if !is_hex_color(color) {
        return Err(AppError::BadRequest(
            "avatar_color must be a hex color like #22c55e".to_string(),
        ));
    }
    Ok(color.to_ascii_lowercase())
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.chars().skip(1).all(|ch| ch.is_ascii_hexdigit())
}

pub(super) fn validate_unique_avatar(
    agents: &[Agent],
    current_id: Option<Uuid>,
    icon: &str,
    color: &str,
) -> AppResult<()> {
    if agents.iter().any(|agent| {
        Some(agent.id) != current_id
            && agent.avatar_icon == icon
            && agent.avatar_color.eq_ignore_ascii_case(color)
    }) {
        return Err(AppError::BadRequest(
            "another agent already uses this avatar icon and color".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_provider_model(
    providers: &[ModelProvider],
    provider_id: Uuid,
    model_id: &str,
) -> AppResult<()> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err(AppError::BadRequest("model_id cannot be empty".to_string()));
    }
    let provider = providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| AppError::BadRequest("model provider does not exist".to_string()))?;
    if !provider.models.iter().any(|model| model == model_id) {
        return Err(AppError::BadRequest(
            "model does not exist on selected provider".to_string(),
        ));
    }
    Ok(())
}
