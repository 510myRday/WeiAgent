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
    let prepared = prepare_chat(&state, conversation_id, &request.content).await?;
    let settings = state.store.read_settings().await?;
    Ok(Json(
        run_agent(state, prepared, request_options(&settings), false, None).await?,
    ))
}

pub async fn send_message_stream(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let prepared = prepare_chat(&state, conversation_id, &request.content).await?;
    let settings = state.store.read_settings().await?;
    let options = request_options(&settings);
    let use_streaming_llm = request.stream;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let error_sender = sender.clone();
    tokio::spawn(async move {
        if let Err(error) =
            run_agent(state, prepared, options, use_streaming_llm, Some(sender)).await
        {
            let _ = error_sender.send(AgentRunEvent::Error(error.to_string()));
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
        } => json_event(
            "approval_required",
            &serde_json::json!({
                "call_id": call_id,
                "command": command,
                "approval_id": approval_id,
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
