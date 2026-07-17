use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, Query, State},
    response::sse::{Event, Sse},
};
use futures_util::Stream;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    app::{
        AppState,
        agent_executor::{AgentRunEvent, run_agent},
        chat::prepare_chat,
        run_manager::SequencedRunEvent,
    },
    domain::{
        AgentRun, AgentRunStatus, ChatMessage, Conversation, ConversationMessagePage,
        CreateConversationRequest, SendMessageRequest, SendMessageResponse,
        UpdateConversationRequest,
    },
    error::AppResult,
    llm::client::RequestOptions,
};

const MAX_AGENT_RUN_DURATION: Duration = Duration::from_secs(60 * 60);

struct ProcessCleanupGuard {
    terminal_manager: std::sync::Arc<crate::tools::terminal::TerminalManager>,
    conversation_id: Uuid,
    armed: bool,
}

impl ProcessCleanupGuard {
    fn new(
        terminal_manager: std::sync::Arc<crate::tools::terminal::TerminalManager>,
        conversation_id: Uuid,
    ) -> Self {
        Self {
            terminal_manager,
            conversation_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProcessCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let terminal_manager = self.terminal_manager.clone();
        let conversation_id = self.conversation_id;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                terminal_manager
                    .stop_for_conversation(conversation_id)
                    .await;
            });
        }
    }
}

fn request_options(settings: &crate::domain::AppSettings) -> RequestOptions {
    RequestOptions {
        timeout: Duration::from_secs(settings.request_timeout_seconds.max(1)),
        retry_attempts: settings.request_retry_attempts.max(1),
    }
}

pub async fn list_conversations(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<Vec<Conversation>>> {
    Ok(Json(state.store.list_conversations(agent_id).await?))
}

pub async fn list_active_agent_runs(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<Vec<AgentRun>>> {
    let ids = state.store.list_conversation_ids(agent_id).await?;
    Ok(Json(state.run_manager.active_for_conversations(&ids)))
}

pub async fn create_conversation(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(request): Json<CreateConversationRequest>,
) -> AppResult<Json<Conversation>> {
    Ok(Json(
        state.store.create_conversation(agent_id, request).await?,
    ))
}

pub async fn delete_conversation(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<()> {
    state
        .run_manager
        .cancel_conversation_and_wait(conversation_id)
        .await?;
    state
        .terminal_manager
        .stop_all_for_conversation(conversation_id)
        .await;
    state.store.delete_conversation(conversation_id).await
}

pub async fn update_conversation(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<UpdateConversationRequest>,
) -> AppResult<Json<Conversation>> {
    Ok(Json(
        state
            .store
            .update_conversation(conversation_id, request)
            .await?,
    ))
}

pub async fn read_conversation_messages(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> AppResult<Json<Vec<ChatMessage>>> {
    Ok(Json(
        state
            .store
            .read_conversation_messages(conversation_id)
            .await?,
    ))
}

pub async fn read_conversation_message_page(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Query(query): Query<ConversationMessagesQuery>,
) -> AppResult<Json<ConversationMessagePage>> {
    Ok(Json(
        state
            .store
            .read_conversation_message_page(
                conversation_id,
                query.before,
                query.limit.unwrap_or(80).clamp(20, 200),
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct ConversationMessagesQuery {
    before: Option<Uuid>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct RunEventsQuery {
    #[serde(default)]
    after: u64,
}

pub async fn create_message_run(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Json<AgentRun>> {
    let settings = state.store.read_settings().await?;
    let registration = state.run_manager.create(conversation_id)?;
    let run_id = registration.info.id;
    let prepared = match prepare_chat(
        &state,
        conversation_id,
        &request.content,
        request.restart_from_message_id,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            state
                .run_manager
                .finish(run_id, AgentRunStatus::Failed, Some(error.to_string()));
            return Err(error);
        }
    };
    state
        .run_manager
        .set_user_message(run_id, prepared.user_message.id);
    let info = state.run_manager.get(run_id)?;
    let options = request_options(&settings);
    let use_streaming_llm = request.stream;
    let manager = state.run_manager.clone();
    let terminal_manager = state.terminal_manager.clone();
    let store = state.store.clone();
    let mut cancel = registration.cancel.clone();
    let (event_sender, mut event_receiver) = mpsc::channel(256);
    let event_manager = manager.clone();
    let forwarding = tokio::spawn(async move {
        let mut pending_delta = String::new();
        let mut flush_interval = tokio::time::interval(Duration::from_millis(32));
        flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = flush_interval.tick(), if !pending_delta.is_empty() => {
                    event_manager.record_event(
                        run_id,
                        AgentRunEvent::Delta(std::mem::take(&mut pending_delta)),
                    );
                }
                event = event_receiver.recv() => {
                    let Some(event) = event else {
                        if !pending_delta.is_empty() {
                            event_manager.record_event(
                                run_id,
                                AgentRunEvent::Delta(std::mem::take(&mut pending_delta)),
                            );
                        }
                        break;
                    };
                    match event {
                        AgentRunEvent::Delta(delta) => {
                            pending_delta.push_str(&delta);
                            if pending_delta.len() >= 8 * 1024 {
                                event_manager.record_event(
                                    run_id,
                                    AgentRunEvent::Delta(std::mem::take(&mut pending_delta)),
                                );
                            }
                        }
                        event => {
                            if !pending_delta.is_empty() {
                                event_manager.record_event(
                                    run_id,
                                    AgentRunEvent::Delta(std::mem::take(&mut pending_delta)),
                                );
                            }
                            event_manager.record_event(run_id, event);
                        }
                    }
                }
            }
        }
    });

    tokio::spawn(async move {
        let _registration = registration;
        let run = run_agent(
            state,
            prepared,
            options,
            use_streaming_llm,
            Some(event_sender.clone()),
        );
        let outcome = tokio::select! {
            result = tokio::time::timeout(MAX_AGENT_RUN_DURATION, run) => {
                match result {
                    Ok(Ok(_)) => (AgentRunStatus::Completed, None, None),
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        (AgentRunStatus::Failed, Some(message.clone()), Some(AgentRunEvent::Error(message)))
                    }
                    Err(_) => {
                        terminal_manager.stop_for_conversation(conversation_id).await;
                        let message = "agent run exceeded the 60 minute deadline".to_string();
                        (AgentRunStatus::Failed, Some(message.clone()), Some(AgentRunEvent::Error(message)))
                    }
                }
            }
            _ = cancel.changed() => {
                terminal_manager.stop_for_conversation(conversation_id).await;
                let _ = store.fail_streaming_messages(conversation_id).await;
                (AgentRunStatus::Cancelled, None, Some(AgentRunEvent::Cancelled))
            }
        };
        if let Some(event) = outcome.2 {
            let _ = event_sender.send(event).await;
        }
        drop(event_sender);
        let _ = forwarding.await;
        manager.finish(run_id, outcome.0, outcome.1);
    });

    Ok(Json(info))
}

pub async fn active_conversation_run(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> Json<Option<AgentRun>> {
    Json(state.run_manager.active_for_conversation(conversation_id))
}

pub async fn cancel_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> AppResult<Json<AgentRun>> {
    Ok(Json(state.run_manager.cancel_and_wait(run_id).await?))
}

pub async fn read_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> AppResult<Json<AgentRun>> {
    Ok(Json(state.run_manager.get(run_id)?))
}

pub async fn stream_run_events(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<RunEventsQuery>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let manager = state.run_manager.clone();
    let subscription = manager.subscribe(run_id, query.after)?;
    let stream = async_stream::stream! {
        let mut cursor = query.after;
        let mut replay = subscription.replay;
        let mut receiver = subscription.receiver;
        let mut terminal = subscription.terminal;
        loop {
            for event in replay.drain(..) {
                if event.sequence <= cursor {
                    continue;
                }
                cursor = event.sequence;
                let done = run_event_is_terminal(&event.event);
                yield Ok(sequenced_event_to_sse(event));
                if done {
                    return;
                }
            }
            if terminal {
                return;
            }
            match receiver.recv().await {
                Ok(event) => {
                    if event.sequence <= cursor {
                        continue;
                    }
                    cursor = event.sequence;
                    let done = run_event_is_terminal(&event.event);
                    yield Ok(sequenced_event_to_sse(event));
                    if done {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    match manager.subscribe(run_id, cursor) {
                        Ok(next) => {
                            replay = next.replay;
                            receiver = next.receiver;
                            terminal = next.terminal;
                        }
                        Err(_) => return,
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

pub async fn send_message_complete(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Json<SendMessageResponse>> {
    let _run_guard = state.run_manager.try_start(conversation_id)?;
    let prepared = prepare_chat(
        &state,
        conversation_id,
        &request.content,
        request.restart_from_message_id,
    )
    .await?;
    let settings = state.store.read_settings().await?;
    let terminal_manager = state.terminal_manager.clone();
    let mut cleanup_guard = ProcessCleanupGuard::new(terminal_manager.clone(), conversation_id);
    let result = tokio::time::timeout(
        MAX_AGENT_RUN_DURATION,
        run_agent(state, prepared, request_options(&settings), false, None),
    )
    .await;
    match result {
        Ok(Ok(result)) => {
            cleanup_guard.disarm();
            Ok(Json(result))
        }
        Ok(Err(error)) => Err(error),
        Err(_) => {
            terminal_manager
                .stop_for_conversation(conversation_id)
                .await;
            cleanup_guard.disarm();
            Err(crate::error::AppError::Llm(
                "agent run exceeded the 60 minute deadline".to_string(),
            ))
        }
    }
}

pub async fn send_message_stream(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let run_guard = state.run_manager.try_start(conversation_id)?;
    let prepared = prepare_chat(
        &state,
        conversation_id,
        &request.content,
        request.restart_from_message_id,
    )
    .await?;
    let settings = state.store.read_settings().await?;
    let options = request_options(&settings);
    let use_streaming_llm = request.stream;
    let (sender, mut receiver) = mpsc::channel(256);
    let error_sender = sender.clone();
    let terminal_manager = state.terminal_manager.clone();
    tokio::spawn(async move {
        let _run_guard = run_guard;
        match tokio::time::timeout(
            MAX_AGENT_RUN_DURATION,
            run_agent(state, prepared, options, use_streaming_llm, Some(sender)),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                let _ = error_sender
                    .send(AgentRunEvent::Error(error.to_string()))
                    .await;
            }
            Err(_) => {
                terminal_manager
                    .stop_for_conversation(conversation_id)
                    .await;
                let _ = error_sender
                    .send(AgentRunEvent::Error(
                        "agent run exceeded the 60 minute deadline".to_string(),
                    ))
                    .await;
            }
        }
    });

    let stream = async_stream::stream! {
        while let Some(event) = receiver.recv().await {
            yield Ok(agent_event_to_sse(event));
        }
    };
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

fn agent_event_to_sse(event: AgentRunEvent) -> Event {
    match event {
        AgentRunEvent::User(message) => json_event("user", &message),
        AgentRunEvent::Assistant(message) => json_event("assistant", &message),
        AgentRunEvent::Delta(delta) => Event::default().event("delta").data(delta),
        AgentRunEvent::Phase {
            phase,
            iteration,
            call_id,
        } => {
            let mut data = serde_json::json!({
                "phase": phase,
                "iteration": iteration,
            });
            if let Some(call_id) = call_id {
                data["call_id"] = serde_json::Value::String(call_id);
            }
            json_event("phase", &data)
        }
        AgentRunEvent::ToolCall {
            call_id,
            name,
            arguments,
            permission_mode,
        } => json_event(
            "tool_call",
            &serde_json::json!({
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
                "permission_mode": permission_mode,
            }),
        ),
        AgentRunEvent::ToolResult { call_id, result } => json_event(
            "tool_result",
            &serde_json::json!({ "call_id": call_id, "result": result }),
        ),
        AgentRunEvent::ApprovalRequired {
            call_id,
            agent_id,
            command,
            approval_id,
            cwd,
            runtime,
            permission,
            justification,
            background,
        } => {
            let mut data = serde_json::json!({
                "call_id": call_id,
                "agent_id": agent_id,
                "command": command,
                "approval_id": approval_id,
                "runtime": runtime,
                "permission": permission,
                "background": background,
            });
            if let Some(cwd) = cwd {
                data["cwd"] = serde_json::Value::String(cwd);
            }
            if let Some(justification) = justification {
                data["justification"] = serde_json::Value::String(justification);
            }
            json_event("approval_required", &data)
        }
        AgentRunEvent::Done => Event::default().event("done").data("ok"),
        AgentRunEvent::Cancelled => Event::default().event("cancelled").data("ok"),
        AgentRunEvent::Error(error) => Event::default().event("error").data(error),
    }
}

fn run_event_is_terminal(event: &AgentRunEvent) -> bool {
    matches!(
        event,
        AgentRunEvent::Done | AgentRunEvent::Cancelled | AgentRunEvent::Error(_)
    )
}

fn sequenced_event_to_sse(event: SequencedRunEvent) -> Event {
    agent_event_to_sse(event.event).id(event.sequence.to_string())
}

fn json_event<T: serde::Serialize>(name: &'static str, value: &T) -> Event {
    Event::default()
        .event(name)
        .json_data(value)
        .unwrap_or_else(|error| {
            Event::default()
                .event("error")
                .data(format!("failed to serialize {name} event: {error}"))
        })
}
