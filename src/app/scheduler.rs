use std::{str::FromStr, time::Duration};

use chrono::{DateTime, FixedOffset, Local, Utc};
use cron::Schedule;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::{
    app::{AppState, handlers::conversations::start_message_run},
    domain::{
        ConversationHeartbeat, ConversationHeartbeatExecution, CreateConversationRequest,
        GenerationMode, ScheduledTask, ScheduledTaskExecution, SendMessageRequest, ThinkingLevel,
    },
    error::{AppError, AppResult},
};

const TICK_SECONDS: u64 = 1;
const MAX_TASKS_PER_TICK: usize = 32;

pub fn start(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(TICK_SECONDS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(error) = tick(&state).await {
                tracing::warn!(%error, "scheduler tick failed");
            }
        }
    })
}

pub async fn tick(state: &AppState) -> AppResult<()> {
    process_heartbeats(state).await?;
    process_cron_tasks(state).await?;
    Ok(())
}

pub fn normalize_schedule(expression: &str) -> AppResult<String> {
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(AppError::BadRequest(
            "schedule must use exactly five cron fields: minute hour day-of-month month day-of-week".to_string(),
        ));
    }
    let normalized = format!("0 {}", fields.join(" "));
    Schedule::from_str(&normalized).map_err(|error| {
        AppError::BadRequest(format!("invalid cron schedule '{expression}': {error}"))
    })?;
    Ok(expression.trim().to_string())
}

pub fn local_timezone_offset() -> String {
    let seconds = Local::now().offset().local_minus_utc();
    format_offset(seconds)
}

pub fn normalize_timezone_offset(value: Option<&str>) -> AppResult<String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(local_timezone_offset);
    parse_timezone_offset(&value)?;
    Ok(value)
}

pub fn next_run_at(
    expression: &str,
    timezone_offset: &str,
    after: DateTime<Utc>,
) -> AppResult<DateTime<Utc>> {
    let normalized = normalize_schedule(expression)?;
    let offset = parse_timezone_offset(timezone_offset)?;
    let schedule = Schedule::from_str(&format!("0 {normalized}"))
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    let local_after = after.with_timezone(&offset);
    schedule
        .after(&local_after)
        .next()
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(|| AppError::BadRequest("cron schedule has no future occurrence".to_string()))
}

fn parse_timezone_offset(value: &str) -> AppResult<FixedOffset> {
    if value.len() != 6
        || !matches!(value.as_bytes().first(), Some(b'+') | Some(b'-'))
        || &value[3..4] != ":"
    {
        return Err(AppError::BadRequest(
            "timezone_offset must use ±HH:MM format".to_string(),
        ));
    }
    let hours = value[1..3]
        .parse::<i32>()
        .map_err(|_| AppError::BadRequest("invalid timezone hour".to_string()))?;
    let minutes = value[4..6]
        .parse::<i32>()
        .map_err(|_| AppError::BadRequest("invalid timezone minute".to_string()))?;
    if hours > 23 || minutes > 59 {
        return Err(AppError::BadRequest(
            "timezone_offset is out of range".to_string(),
        ));
    }
    let seconds = (hours * 3600 + minutes * 60) * if &value[0..1] == "-" { -1 } else { 1 };
    FixedOffset::east_opt(seconds)
        .ok_or_else(|| AppError::BadRequest("timezone_offset is invalid".to_string()))
}

fn format_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let absolute = seconds.unsigned_abs();
    format!("{sign}{:02}:{:02}", absolute / 3600, (absolute % 3600) / 60)
}

async fn process_heartbeats(state: &AppState) -> AppResult<()> {
    let due = state.store.claim_due_heartbeats(Utc::now()).await?;
    for heartbeat in due {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = execute_heartbeat(&state, heartbeat).await {
                tracing::warn!(%error, "heartbeat execution failed");
            }
        });
    }
    Ok(())
}

async fn execute_heartbeat(state: &AppState, heartbeat: ConversationHeartbeat) -> AppResult<()> {
    let execution_id = Uuid::new_v4();
    let scheduled_at =
        heartbeat.next_fire_at - chrono::Duration::seconds(heartbeat.interval_seconds as i64);
    let started_at = Utc::now();
    let conversation = state
        .store
        .get_conversation(heartbeat.conversation_id)
        .await?;
    let agent = state.store.get_agent(conversation.agent_id).await?;
    let provider = state
        .store
        .get_model_provider(agent.model_provider_id)
        .await?;
    state
        .store
        .insert_conversation_heartbeat_execution(ConversationHeartbeatExecution {
            id: execution_id,
            conversation_id: heartbeat.conversation_id,
            scheduled_at,
            started_at: Some(started_at),
            finished_at: None,
            status: "running".to_string(),
            error: None,
            trigger_type: "scheduled".to_string(),
            agent_name: Some(agent.name),
            provider_name: Some(provider.name),
            model_id: Some(agent.model_id),
            output_preview: None,
        })
        .await?;
    if state
        .run_manager
        .active_for_conversation(heartbeat.conversation_id)
        .is_some()
    {
        state
            .store
            .mark_heartbeat_pending(heartbeat.conversation_id)
            .await?;
        state
            .store
            .finish_conversation_heartbeat_execution(
                execution_id,
                heartbeat.conversation_id,
                "pending",
                None,
            )
            .await?;
        return Ok(());
    }
    let request = SendMessageRequest {
        content: heartbeat.prompt.clone(),
        attachments: Vec::new(),
        thinking_level: ThinkingLevel::default(),
        stream: false,
        generation_mode: GenerationMode::Chat,
        restart_from_message_id: None,
        shortcut_actions: Vec::new(),
    };
    let result = start_message_run(state.clone(), heartbeat.conversation_id, request).await;
    match result {
        Ok(run) => {
            let terminal = state.run_manager.wait_for_terminal(run.id).await?;
            state
                .store
                .finish_conversation_heartbeat_execution(
                    execution_id,
                    heartbeat.conversation_id,
                    &status_name(terminal.status),
                    terminal.error,
                )
                .await?;
        }
        Err(AppError::Conflict(_)) => {
            state
                .store
                .mark_heartbeat_pending(heartbeat.conversation_id)
                .await?;
            state
                .store
                .finish_conversation_heartbeat_execution(
                    execution_id,
                    heartbeat.conversation_id,
                    "pending",
                    None,
                )
                .await?;
        }
        Err(error) => {
            let message = error.to_string();
            state
                .store
                .finish_conversation_heartbeat_execution(
                    execution_id,
                    heartbeat.conversation_id,
                    "failed",
                    Some(message),
                )
                .await?;
        }
    }
    Ok(())
}

async fn process_cron_tasks(state: &AppState) -> AppResult<()> {
    for _ in 0..MAX_TASKS_PER_TICK {
        let execution_id = Uuid::new_v4();
        let Some(task) = state
            .store
            .claim_scheduled_task(Utc::now(), execution_id)
            .await?
        else {
            break;
        };
        let state = state.clone();
        let scheduled_at = task.next_run_at;
        tokio::spawn(async move {
            if let Err(error) =
                execute_cron_task(&state, task, execution_id, "scheduled", scheduled_at).await
            {
                tracing::warn!(%error, "cron execution failed");
            }
        });
    }
    Ok(())
}

pub async fn run_cron_task_now(state: AppState, task_id: Uuid) -> AppResult<ScheduledTask> {
    let now = Utc::now();
    let execution_id = Uuid::new_v4();
    let task = state
        .store
        .claim_scheduled_task_now(task_id, now, execution_id)
        .await?
        .ok_or_else(|| AppError::Conflict("scheduled task is already running".to_string()))?;
    let background_state = state.clone();
    let background_task = task.clone();
    tokio::spawn(async move {
        if let Err(error) = execute_cron_task(
            &background_state,
            background_task,
            execution_id,
            "manual",
            now,
        )
        .await
        {
            tracing::warn!(%error, "manual cron execution failed");
        }
    });
    Ok(task)
}

async fn execute_cron_task(
    state: &AppState,
    task: ScheduledTask,
    execution_id: Uuid,
    trigger_type: &str,
    scheduled_at: DateTime<Utc>,
) -> AppResult<()> {
    let now = Utc::now();
    let next_run_at = next_run_at(&task.schedule_expr, &task.timezone_offset, now)
        .unwrap_or_else(|_| now + chrono::Duration::minutes(1));
    let agent = match state.store.get_agent(task.agent_id).await {
        Ok(agent) => agent,
        Err(error) => {
            state
                .store
                .fail_claimed_scheduled_task(task.id, execution_id, next_run_at, error.to_string())
                .await?;
            return Err(error);
        }
    };
    let provider = match state
        .store
        .get_model_provider(agent.model_provider_id)
        .await
    {
        Ok(provider) => provider,
        Err(error) => {
            state
                .store
                .fail_claimed_scheduled_task(task.id, execution_id, next_run_at, error.to_string())
                .await?;
            return Err(error);
        }
    };
    let title = format!(
        "[Cron] {}",
        task.prompt.chars().take(42).collect::<String>()
    );
    let conversation = match state
        .store
        .create_conversation(
            task.agent_id,
            CreateConversationRequest { title: Some(title) },
        )
        .await
    {
        Ok(conversation) => conversation,
        Err(error) => {
            state
                .store
                .fail_claimed_scheduled_task(task.id, execution_id, next_run_at, error.to_string())
                .await?;
            return Err(error);
        }
    };
    let execution = ScheduledTaskExecution {
        id: execution_id,
        task_id: task.id,
        conversation_id: conversation.id,
        scheduled_at,
        started_at: Some(now),
        finished_at: None,
        status: "running".to_string(),
        error: None,
        trigger_type: trigger_type.to_string(),
        agent_name: Some(agent.name),
        provider_name: Some(provider.name),
        model_id: Some(agent.model_id),
        output_preview: None,
    };
    if let Err(error) = state.store.insert_scheduled_task_execution(execution).await {
        state
            .store
            .fail_claimed_scheduled_task(task.id, execution_id, next_run_at, error.to_string())
            .await?;
        return Err(error);
    }
    let request = SendMessageRequest {
        content: task.prompt.clone(),
        attachments: Vec::new(),
        thinking_level: ThinkingLevel::default(),
        stream: false,
        generation_mode: GenerationMode::Chat,
        restart_from_message_id: None,
        shortcut_actions: Vec::new(),
    };
    let run = match start_message_run(state.clone(), conversation.id, request).await {
        Ok(run) => run,
        Err(error) => {
            state
                .store
                .finish_scheduled_task_execution(
                    execution_id,
                    task.id,
                    conversation.id,
                    next_run_at,
                    "failed",
                    Some(error.to_string()),
                )
                .await?;
            return Err(error);
        }
    };
    let terminal = state.run_manager.wait_for_terminal(run.id).await?;
    state
        .store
        .finish_scheduled_task_execution(
            execution_id,
            task.id,
            conversation.id,
            next_run_at,
            &status_name(terminal.status),
            terminal.error,
        )
        .await?;
    Ok(())
}

fn status_name(status: crate::domain::AgentRunStatus) -> String {
    match status {
        crate::domain::AgentRunStatus::Running => "running",
        crate::domain::AgentRunStatus::Completed => "completed",
        crate::domain::AgentRunStatus::Failed => "failed",
        crate::domain::AgentRunStatus::Cancelled => "cancelled",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_five_field_cron_and_rejects_six_field_cron() {
        assert_eq!(normalize_schedule("0 9 * * *").unwrap(), "0 9 * * *");
        assert!(normalize_schedule("0 0 9 * * *").is_err());
    }

    #[test]
    fn computes_next_run_with_fixed_offset() {
        let after = DateTime::parse_from_rfc3339("2026-08-22T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let next = next_run_at("0 9 * * *", "+08:00", after).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-08-22T01:00:00+00:00");
    }
}
