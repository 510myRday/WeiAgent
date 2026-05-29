use std::convert::Infallible;

use axum::{
    Json,
    extract::{Path, State},
    response::sse::{Event, Sse},
};
use chrono::Utc;
use futures_util::{Stream, StreamExt};
use uuid::Uuid;

use crate::{
    app::{AppState, chat::prepare_chat},
    domain::{
        ChatMessage, ChatMessageStatus, ChatRole, Conversation, CreateConversationRequest,
        SendMessageRequest, SendMessageResponse,
    },
    error::AppResult,
    llm::client::RequestOptions,
};

fn request_options(settings: &crate::domain::AppSettings) -> RequestOptions {
    RequestOptions {
        timeout: std::time::Duration::from_secs(settings.request_timeout_seconds.max(1)),
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
    let client = state.llm_registry.client_for(&prepared.provider)?;
    let assistant_content = client
        .complete(
            &prepared.agent,
            &prepared.provider,
            &prepared.messages_for_llm,
            &prepared.system_prompt,
            request_options(&settings),
        )
        .await?;
    let assistant_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: prepared.agent.id,
        conversation_id,
        role: ChatRole::Assistant,
        content: assistant_content,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
    };
    state
        .store
        .append_conversation_messages(
            conversation_id,
            &[prepared.user_message.clone(), assistant_message.clone()],
        )
        .await?;

    Ok(Json(SendMessageResponse {
        user: prepared.user_message,
        assistant: assistant_message,
    }))
}

pub async fn send_message_stream(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let prepared = prepare_chat(&state, conversation_id, &request.content).await?;
    let settings = state.store.read_settings().await?;
    let client = state.llm_registry.client_for(&prepared.provider)?;
    let llm_stream = client
        .stream(
            &prepared.agent,
            &prepared.provider,
            &prepared.messages_for_llm,
            &prepared.system_prompt,
            request_options(&settings),
        )
        .await?;

    let store = state.store.clone();
    let user_message = prepared.user_message;
    let stream = async_stream::stream! {
        let mut assistant_content = String::new();
        let mut llm_stream = llm_stream;
        let assistant_message_id = Uuid::new_v4();
        let mut assistant_message = ChatMessage {
            id: assistant_message_id,
            agent_id: user_message.agent_id,
            conversation_id,
            role: ChatRole::Assistant,
            content: String::new(),
            status: ChatMessageStatus::Streaming,
            created_at: Utc::now(),
        };

        if let Err(error) = store
            .append_conversation_messages(conversation_id, &[user_message.clone(), assistant_message.clone()])
            .await
        {
            yield Ok(Event::default().event("error").data(error.to_string()));
            return;
        }

        yield Ok(Event::default().event("user").json_data(&user_message).unwrap_or_else(|_| {
            Event::default().event("error").data("failed to serialize user message")
        }));
        yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
            Event::default().event("error").data("failed to serialize assistant message")
        }));

        while let Some(chunk) = llm_stream.next().await {
            match chunk {
                Ok(content) => {
                    assistant_content.push_str(&content);
                    if let Err(error) = store
                        .update_conversation_message(
                            conversation_id,
                            assistant_message_id,
                            assistant_content.clone(),
                            ChatMessageStatus::Streaming,
                        )
                        .await
                    {
                        yield Ok(Event::default().event("error").data(error.to_string()));
                        return;
                    }
                    yield Ok(Event::default().event("delta").data(content));
                }
                Err(error) => {
                    let failed_content = if assistant_content.is_empty() {
                        format!("[stream failed: {error}]")
                    } else {
                        assistant_content.clone()
                    };
                    let _ = store
                        .update_conversation_message(
                            conversation_id,
                            assistant_message_id,
                            failed_content,
                            ChatMessageStatus::Failed,
                        )
                        .await;
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            }
        }

        assistant_message.content = assistant_content;
        assistant_message.status = ChatMessageStatus::Done;
        match store
            .update_conversation_message(
                conversation_id,
                assistant_message_id,
                assistant_message.content.clone(),
                ChatMessageStatus::Done,
            )
            .await
        {
            Ok(()) => {
                yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                    Event::default().event("error").data("failed to serialize assistant message")
                }));
                yield Ok(Event::default().event("done").data("ok"));
            }
            Err(error) => {
                yield Ok(Event::default().event("error").data(error.to_string()));
            }
        }
    };

    Ok(Sse::new(stream))
}
