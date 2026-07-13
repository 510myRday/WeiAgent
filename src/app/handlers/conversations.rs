use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    response::sse::{Event, Sse},
};
use futures_util::Stream;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    app::{
        AppState,
        agent_executor::{AgentRunEvent, run_agent},
        chat::prepare_chat,
    },
    domain::{
        ChatMessage, Conversation, CreateConversationRequest, SendMessageRequest,
        SendMessageResponse,
    },
    error::AppResult,
    llm::client::RequestOptions,
};

const MAX_AGENT_RUN_DURATION: Duration = Duration::from_secs(15 * 60);

struct ProcessCleanupGuard {
    process_manager: std::sync::Arc<crate::tools::process_manager::ProcessManager>,
    conversation_id: Uuid,
    armed: bool,
}

impl ProcessCleanupGuard {
    fn new(
        process_manager: std::sync::Arc<crate::tools::process_manager::ProcessManager>,
        conversation_id: Uuid,
    ) -> Self {
        Self {
            process_manager,
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
        let process_manager = self.process_manager.clone();
        let conversation_id = self.conversation_id;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                process_manager.stop_for_conversation(conversation_id).await;
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
    state.store.delete_conversation(conversation_id).await
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

pub async fn send_message_complete(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Json<SendMessageResponse>> {
    let _run_guard = state.run_manager.try_start(conversation_id)?;
    let prepared = prepare_chat(&state, conversation_id, &request.content).await?;
    let settings = state.store.read_settings().await?;
    let process_manager = state.process_manager.clone();
    let mut cleanup_guard = ProcessCleanupGuard::new(process_manager.clone(), conversation_id);
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
            process_manager.stop_for_conversation(conversation_id).await;
            cleanup_guard.disarm();
            Err(crate::error::AppError::Llm(
                "agent run exceeded the 15 minute deadline".to_string(),
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
    let prepared = prepare_chat(&state, conversation_id, &request.content).await?;
    let settings = state.store.read_settings().await?;
    let options = request_options(&settings);
    let use_streaming_llm = request.stream;
    let (sender, mut receiver) = mpsc::channel(256);
    let error_sender = sender.clone();
    let process_manager = state.process_manager.clone();
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
                process_manager.stop_for_conversation(conversation_id).await;
                let _ = error_sender
                    .send(AgentRunEvent::Error(
                        "agent run exceeded the 15 minute deadline".to_string(),
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
    Ok(Sse::new(stream))
}

fn agent_event_to_sse(event: AgentRunEvent) -> Event {
    match event {
        AgentRunEvent::User(message) => json_event("user", &message),
        AgentRunEvent::Assistant(message) => json_event("assistant", &message),
        AgentRunEvent::Delta(delta) => Event::default().event("delta").data(delta),
        AgentRunEvent::ToolCall {
            call_id,
            name,
            arguments,
        } => json_event(
            "tool_call",
            &serde_json::json!({
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            }),
        ),
        AgentRunEvent::ToolResult { call_id, result } => json_event(
            "tool_result",
            &serde_json::json!({ "call_id": call_id, "result": result }),
        ),
        AgentRunEvent::ApprovalRequired {
            call_id,
            command,
            approval_id,
            cwd,
            runtime,
            network_enabled,
            write_roots,
            background,
        } => json_event(
            "approval_required",
            &serde_json::json!({
                "call_id": call_id,
                "command": command,
                "approval_id": approval_id,
                "cwd": cwd,
                "runtime": runtime,
                "network_enabled": network_enabled,
                "write_roots": write_roots,
                "background": background,
            }),
        ),
        AgentRunEvent::Done => Event::default().event("done").data("ok"),
        AgentRunEvent::Error(error) => Event::default().event("error").data(error),
    }
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
