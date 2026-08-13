use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        CreateSkillGroupRequest, RenameSkillGroupRequest, SkillAgentOverridesRequest,
        SkillGroupAssignmentRequest, SkillGroupOrderRequest, SkillGroupSkillOrderRequest,
        SkillImportTask, SkillManifest, SkillOverrides, SkillSettings,
    },
    error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct SkillContentQuery {
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SkillChangeQuery {
    #[serde(default)]
    pub since: u64,
}

#[derive(Debug, Deserialize)]
pub struct SkillEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct SkillIconRequest {
    pub icon_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SkillUpdateRequest {
    pub source_url: String,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct SkillImportRequest {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub source_directory: Option<String>,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
}

pub async fn list_skills(State(state): State<AppState>) -> AppResult<Json<SkillSettings>> {
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn skill_changes(
    State(state): State<AppState>,
    Query(query): Query<SkillChangeQuery>,
) -> AppResult<Json<crate::domain::SkillChangeFeed>> {
    Ok(Json(state.skill_manager.changes_since(query.since).await))
}

pub async fn import_skill(
    State(state): State<AppState>,
    Json(request): Json<SkillImportRequest>,
) -> AppResult<Json<SkillImportTask>> {
    let source = request
        .source
        .or(request.source_directory)
        .ok_or_else(|| AppError::BadRequest("Skill path or URL is required".to_string()))?;
    let proxy_url = if let Some(proxy_id) = request.proxy_id {
        let proxy = state
            .store
            .list_ip_proxies()
            .await?
            .into_iter()
            .find(|proxy| proxy.id == proxy_id)
            .ok_or_else(|| {
                AppError::BadRequest("selected Skill download proxy was not found".to_string())
            })?;
        if !proxy.enabled {
            return Err(AppError::BadRequest(
                "selected Skill download proxy is disabled".to_string(),
            ));
        }
        Some(proxy.url)
    } else {
        None
    };
    let kind = request.kind.unwrap_or_else(|| {
        if source.contains("://") {
            "network"
        } else {
            "local"
        }
        .to_string()
    });
    let task = state
        .skill_manager
        .create_import_task(&source, &kind, request.group_id)
        .await?;
    let manager = state.skill_manager.clone();
    let group_id = task.group_id.clone();
    tokio::spawn({
        let source = source.clone();
        let proxy_url = proxy_url.clone();
        let task_id = task.id;
        async move {
            manager
            .run_import_task(task_id, source, proxy_url, group_id)
            .await;
        }
    });
    Ok(Json(task))
}

pub async fn list_import_tasks(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<SkillImportTask>>> {
    Ok(Json(state.skill_manager.list_import_tasks().await))
}

pub async fn read_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<SkillManifest>> {
    let settings = state.skill_manager.settings(None).await?;
    settings
        .defaults
        .into_iter()
        .find(|skill| skill.name == name)
        .map(Json)
        .ok_or(AppError::NotFound)
}

pub async fn read_skill_content(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<SkillContentQuery>,
) -> AppResult<Json<crate::domain::SkillContent>> {
    Ok(Json(
        state
            .skill_manager
            .inspect_content(&name, query.path.as_deref())
            .await?,
    ))
}

pub async fn update_skill_enabled(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<SkillEnabledRequest>,
) -> AppResult<Json<SkillSettings>> {
    state
        .store
        .update_skill_global_override(&name, Some(request.enabled))
        .await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn update_skill_icon(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<SkillIconRequest>,
) -> AppResult<Json<SkillSettings>> {
    let name = name.trim().to_ascii_lowercase();
    let settings = state.skill_manager.settings(None).await?;
    if !settings.defaults.iter().any(|skill| skill.name == name) {
        return Err(AppError::NotFound);
    }
    let icon_url = request.icon_url.map(|value| value.trim().to_string());
    if icon_url.as_deref().is_some_and(|value| value.len() > 2048) {
        return Err(AppError::BadRequest(
            "Skill icon address is too long".to_string(),
        ));
    }
    state.store.update_skill_icon(&name, icon_url).await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn update_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<SkillUpdateRequest>,
) -> AppResult<Json<SkillImportTask>> {
    let name = name.trim().to_ascii_lowercase();
    let settings = state.skill_manager.settings(None).await?;
    let skill = settings
        .defaults
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or(AppError::NotFound)?;
    let source = request.source_url.trim().to_string();
    if source.is_empty() || !source.contains("://") {
        return Err(AppError::BadRequest(
            "Skill source URL is required".to_string(),
        ));
    }
    let Some(stored_source) = skill.source_url else {
        return Err(AppError::BadRequest(
            "This Skill was imported from a local path".to_string(),
        ));
    };
    if stored_source != source {
        return Err(AppError::BadRequest(
            "Skill source URL does not match the saved source".to_string(),
        ));
    }
    let proxy_url = if let Some(proxy_id) = request.proxy_id {
        let proxy = state
            .store
            .list_ip_proxies()
            .await?
            .into_iter()
            .find(|proxy| proxy.id == proxy_id)
            .ok_or_else(|| {
                AppError::BadRequest("selected Skill download proxy was not found".to_string())
            })?;
        if !proxy.enabled {
            return Err(AppError::BadRequest(
                "selected Skill download proxy is disabled".to_string(),
            ));
        }
        Some(proxy.url)
    } else {
        None
    };
    let task = state
        .skill_manager
        .create_import_task(&source, "network", None)
        .await?;
    let manager = state.skill_manager.clone();
    tokio::spawn(async move {
        manager.run_import_task(task.id, source, proxy_url, None).await;
    });
    Ok(Json(task))
}

pub async fn delete_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<SkillSettings>> {
    state.skill_manager.delete_skill(&name).await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn update_skill_group_enabled(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<SkillEnabledRequest>,
) -> AppResult<Json<SkillSettings>> {
    let group = state.store.read_skill_group_name(&group_id).await?;
    state
        .store
        .update_skill_global_group_override(&group, Some(request.enabled))
        .await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn create_skill_group(
    State(state): State<AppState>,
    Json(request): Json<CreateSkillGroupRequest>,
) -> AppResult<Json<SkillSettings>> {
    state.skill_manager.create_group(&request.name).await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn rename_skill_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<RenameSkillGroupRequest>,
) -> AppResult<Json<SkillSettings>> {
    state.skill_manager.rename_group(&group_id, &request.name).await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn delete_skill_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> AppResult<Json<SkillSettings>> {
    state.skill_manager.delete_group(&group_id).await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn reorder_skill_groups(
    State(state): State<AppState>,
    Json(request): Json<SkillGroupOrderRequest>,
) -> AppResult<Json<SkillSettings>> {
    state.skill_manager.reorder_groups(&request.ids).await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn assign_skill_group(
    State(state): State<AppState>,
    Path(skill_name): Path<String>,
    Json(request): Json<SkillGroupAssignmentRequest>,
) -> AppResult<Json<SkillSettings>> {
    state
        .skill_manager
        .assign_skill_group(&skill_name, request.group_id.as_deref())
        .await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn reorder_skill_group_items(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    Json(request): Json<SkillGroupSkillOrderRequest>,
) -> AppResult<Json<SkillSettings>> {
    let target_group = (group_id != "ungrouped").then_some(group_id.as_str());
    state
        .skill_manager
        .reorder_group_skills(target_group, &request.skill_names)
        .await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn cleanup_orphans(State(state): State<AppState>) -> AppResult<Json<SkillSettings>> {
    let settings = state.skill_manager.settings(None).await?;
    state
        .store
        .cleanup_skill_orphans(&settings.orphan_skills)
        .await?;
    Ok(Json(state.skill_manager.settings(None).await?))
}

pub async fn read_agent_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<SkillSettings>> {
    state.store.get_agent(agent_id).await?;
    Ok(Json(state.skill_manager.settings(Some(agent_id)).await?))
}

pub async fn update_agent_skills(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<SkillAgentOverridesRequest>,
) -> AppResult<Json<SkillSettings>> {
    state.store.get_agent(agent_id).await?;
    state
        .store
        .update_skill_agent_overrides(
            agent_id,
            SkillOverrides {
                skills: request.skills,
                groups: request.groups,
            },
        )
        .await?;
    Ok(Json(state.skill_manager.settings(Some(agent_id)).await?))
}
