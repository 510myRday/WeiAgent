use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    app::{AppState, scheduler},
    domain::{
        ConversationHeartbeat, CreateScheduledTaskRequest, HeartbeatRequest, ScheduledTask,
        ScheduledTaskExecution, UpdateScheduledTaskRequest,
    },
    error::{AppError, AppResult},
};

pub async fn list_cron_tasks(State(state): State<AppState>) -> AppResult<Json<Vec<ScheduledTask>>> {
    Ok(Json(state.store.list_scheduled_tasks().await?))
}

pub async fn create_cron_task(
    State(state): State<AppState>,
    Json(request): Json<CreateScheduledTaskRequest>,
) -> AppResult<Json<ScheduledTask>> {
    let schedule = scheduler::normalize_schedule(&request.schedule)?;
    let timezone_offset = scheduler::normalize_timezone_offset(request.timezone_offset.as_deref())?;
    let prompt = request.prompt.trim().to_string();
    if prompt.chars().count() > 100_000 {
        return Err(AppError::BadRequest("cron prompt is too long".to_string()));
    }
    if prompt.is_empty() {
        return Err(AppError::BadRequest(
            "cron prompt cannot be empty".to_string(),
        ));
    }
    if let Some(conversation_id) = request.source_conversation_id {
        let conversation = state.store.get_conversation(conversation_id).await?;
        if conversation.agent_id != request.agent_id {
            return Err(AppError::BadRequest(
                "source conversation belongs to a different agent".to_string(),
            ));
        }
    }
    let now = Utc::now();
    let task = ScheduledTask {
        id: Uuid::new_v4(),
        agent_id: request.agent_id,
        source_conversation_id: request.source_conversation_id,
        schedule_expr: schedule,
        timezone_offset,
        prompt,
        enabled: true,
        next_run_at: scheduler::next_run_at(
            &request.schedule,
            &scheduler::normalize_timezone_offset(request.timezone_offset.as_deref())?,
            now,
        )?,
        last_run_at: None,
        last_status: None,
        last_error: None,
        active_execution_id: None,
        latest_execution_conversation_id: None,
        created_at: now,
        updated_at: now,
    };
    Ok(Json(state.store.insert_scheduled_task(task).await?))
}

pub async fn read_cron_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> AppResult<Json<ScheduledTask>> {
    Ok(Json(state.store.get_scheduled_task(task_id).await?))
}

pub async fn update_cron_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    Json(request): Json<UpdateScheduledTaskRequest>,
) -> AppResult<Json<ScheduledTask>> {
    let current = state.store.get_scheduled_task(task_id).await?;
    if let Some(agent_id) = request.agent_id {
        state.store.get_agent(agent_id).await?;
    }
    let schedule = request
        .schedule
        .as_deref()
        .map(scheduler::normalize_schedule)
        .transpose()?;
    let timezone = request
        .timezone_offset
        .as_deref()
        .map(|value| scheduler::normalize_timezone_offset(Some(value)))
        .transpose()?;
    let prompt = request
        .prompt
        .map(|value| {
            let value = value.trim().to_string();
            if value.chars().count() > 100_000 {
                return Err(AppError::BadRequest("cron prompt is too long".to_string()));
            }
            if value.is_empty() {
                Err(AppError::BadRequest(
                    "cron prompt cannot be empty".to_string(),
                ))
            } else {
                Ok(value)
            }
        })
        .transpose()?;
    let schedule_changed = schedule.is_some() || timezone.is_some();
    let effective_schedule = schedule.as_deref().unwrap_or(&current.schedule_expr);
    let effective_timezone = timezone.as_deref().unwrap_or(&current.timezone_offset);
    let next = schedule_changed
        .then(|| scheduler::next_run_at(effective_schedule, effective_timezone, Utc::now()))
        .transpose()?;
    Ok(Json(
        state
            .store
            .update_scheduled_task(task_id, request.agent_id, schedule, timezone, prompt, next)
            .await?,
    ))
}

pub async fn pause_cron_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> AppResult<Json<ScheduledTask>> {
    Ok(Json(
        state
            .store
            .set_scheduled_task_enabled(task_id, false)
            .await?,
    ))
}

pub async fn resume_cron_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> AppResult<Json<ScheduledTask>> {
    Ok(Json(
        state
            .store
            .set_scheduled_task_enabled(task_id, true)
            .await?,
    ))
}

pub async fn run_cron_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> AppResult<Json<ScheduledTask>> {
    Ok(Json(scheduler::run_cron_task_now(state, task_id).await?))
}

pub async fn delete_cron_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> AppResult<()> {
    state.store.delete_scheduled_task(task_id).await
}

pub async fn list_cron_executions(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> AppResult<Json<Vec<ScheduledTaskExecution>>> {
    Ok(Json(
        state.store.list_scheduled_task_executions(task_id).await?,
    ))
}

pub async fn list_heartbeat_executions(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<Vec<crate::domain::ConversationHeartbeatExecution>>> {
    Ok(Json(
        state
            .store
            .list_conversation_heartbeat_executions(conversation_id)
            .await?,
    ))
}

pub async fn read_heartbeat(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<ConversationHeartbeat>> {
    Ok(Json(
        state
            .store
            .get_conversation_heartbeat(conversation_id)
            .await?,
    ))
}

pub async fn list_heartbeats(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ConversationHeartbeat>>> {
    Ok(Json(state.store.list_conversation_heartbeats().await?))
}

pub async fn set_heartbeat(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<HeartbeatRequest>,
) -> AppResult<Json<ConversationHeartbeat>> {
    state.store.get_conversation(conversation_id).await?;
    let target_conversation_id = request.conversation_id.unwrap_or(conversation_id);
    let target_conversation = state.store.get_conversation(target_conversation_id).await?;
    let prompt = request.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(AppError::BadRequest(
            "heartbeat prompt cannot be empty".to_string(),
        ));
    }
    crate::storage::validate_heartbeat_interval(request.interval_seconds)?;
    if target_conversation_id != conversation_id {
        if state
            .store
            .get_conversation_heartbeat(target_conversation_id)
            .await
            .is_ok()
        {
            return Err(AppError::Conflict(
                "target conversation already has a heartbeat".to_string(),
            ));
        }
    }
    let heartbeat = ConversationHeartbeat {
        conversation_id: target_conversation_id,
        agent_id: target_conversation.agent_id,
        interval_seconds: request.interval_seconds,
        prompt,
        enabled: true,
        next_fire_at: Utc::now() + chrono::Duration::seconds(request.interval_seconds as i64),
        pending: false,
        last_run_at: None,
        last_status: None,
        last_error: None,
    };
    Ok(Json(
        state
            .store
            .move_conversation_heartbeat(conversation_id, heartbeat)
            .await?,
    ))
}

pub async fn pause_heartbeat(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<ConversationHeartbeat>> {
    Ok(Json(
        state
            .store
            .set_conversation_heartbeat_enabled(conversation_id, false)
            .await?,
    ))
}

pub async fn resume_heartbeat(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<ConversationHeartbeat>> {
    Ok(Json(
        state
            .store
            .set_conversation_heartbeat_enabled(conversation_id, true)
            .await?,
    ))
}

pub async fn run_heartbeat(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<ConversationHeartbeat>> {
    let mut heartbeat = state
        .store
        .get_conversation_heartbeat(conversation_id)
        .await?;
    heartbeat.next_fire_at = Utc::now();
    heartbeat.pending = false;
    Ok(Json(
        state.store.upsert_conversation_heartbeat(heartbeat).await?,
    ))
}

pub async fn delete_heartbeat(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<()> {
    state
        .store
        .delete_conversation_heartbeat(conversation_id)
        .await
}
