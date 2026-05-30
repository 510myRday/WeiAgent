use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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
    storage::JsonStore,
};

fn request_options(settings: &crate::domain::AppSettings) -> RequestOptions {
    RequestOptions {
        timeout: std::time::Duration::from_secs(settings.request_timeout_seconds.max(1)),
        retry_attempts: settings.request_retry_attempts.max(1),
    }
}

const STREAM_SAVE_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_SAVE_CHARS: usize = 240;

#[derive(Default)]
struct StreamCleanupState {
    assistant_persisted: bool,
    completed: bool,
    persisted_content: String,
}

struct StreamCleanup {
    store: Arc<JsonStore>,
    conversation_id: Uuid,
    assistant_message_id: Uuid,
    state: Arc<Mutex<StreamCleanupState>>,
}

impl Drop for StreamCleanup {
    fn drop(&mut self) {
        let Ok(state) = self.state.lock() else {
            return;
        };
        if state.completed || !state.assistant_persisted {
            return;
        }

        let store = self.store.clone();
        let conversation_id = self.conversation_id;
        let assistant_message_id = self.assistant_message_id;
        let persisted_content = state.persisted_content.clone();
        tokio::spawn(async move {
            let _ = store
                .update_conversation_message(
                    conversation_id,
                    assistant_message_id,
                    persisted_content,
                    ChatMessageStatus::Failed,
                )
                .await;
        });
    }
}

fn update_cleanup_state(
    state: &Arc<Mutex<StreamCleanupState>>,
    assistant_persisted: bool,
    completed: bool,
    persisted_content: String,
) {
    if let Ok(mut state) = state.lock() {
        state.assistant_persisted = assistant_persisted;
        state.completed = completed;
        state.persisted_content = persisted_content;
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

    let store = state.store.clone();
    let agent = prepared.agent;
    let provider = prepared.provider;
    let messages_for_llm = prepared.messages_for_llm;
    let system_prompt = prepared.system_prompt;
    let user_message = prepared.user_message;
    let options = request_options(&settings);
    let stream = async_stream::stream! {
        let mut assistant_content = String::new();
        let mut persisted_assistant_content = String::new();
        let mut pending_content = String::new();
        let assistant_message_id = Uuid::new_v4();
        let cleanup_state = Arc::new(Mutex::new(StreamCleanupState::default()));
        let _cleanup = StreamCleanup {
            store: store.clone(),
            conversation_id,
            assistant_message_id,
            state: cleanup_state.clone(),
        };
        let mut assistant_persisted = false;
        let mut last_save = Instant::now();
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
            .append_conversation_messages(conversation_id, std::slice::from_ref(&user_message))
            .await
        {
            yield Ok(Event::default().event("error").data(error.to_string()));
            return;
        }

        yield Ok(Event::default().event("user").json_data(&user_message).unwrap_or_else(|_| {
            Event::default().event("error").data("failed to serialize user message")
        }));

        let mut llm_stream = match client
            .stream(
                &agent,
                &provider,
                &messages_for_llm,
                &system_prompt,
                options,
            )
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                yield Ok(Event::default().event("error").data(error.to_string()));
                return;
            }
        };

        while let Some(chunk) = llm_stream.next().await {
            match chunk {
                Ok(content) => {
                    assistant_content.push_str(&content);
                    pending_content.push_str(&content);

                    if pending_content.chars().count() >= STREAM_SAVE_CHARS || last_save.elapsed() >= STREAM_SAVE_INTERVAL {
                        let was_assistant_persisted = assistant_persisted;
                        let save_result = if assistant_persisted {
                            store.update_conversation_message(
                                conversation_id,
                                assistant_message_id,
                                assistant_content.clone(),
                                ChatMessageStatus::Streaming,
                            )
                            .await
                        } else {
                            assistant_message.content = assistant_content.clone();
                            store.append_conversation_messages(conversation_id, std::slice::from_ref(&assistant_message)).await
                        };

                        if let Err(error) = save_result {
                            yield Ok(Event::default().event("error").data(error.to_string()));
                            return;
                        }

                        persisted_assistant_content = assistant_content.clone();
                        if !assistant_persisted {
                            assistant_persisted = true;
                            yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                                Event::default().event("error").data("failed to serialize assistant message")
                            }));
                        }
                        update_cleanup_state(
                            &cleanup_state,
                            assistant_persisted,
                            false,
                            persisted_assistant_content.clone(),
                        );

                        let persisted_content = std::mem::take(&mut pending_content);
                        last_save = Instant::now();
                        if was_assistant_persisted {
                            yield Ok(Event::default().event("delta").data(persisted_content));
                        }
                    }
                }
                Err(error) => {
                    let _ = if assistant_persisted {
                        store
                            .update_conversation_message(
                                conversation_id,
                                assistant_message_id,
                                persisted_assistant_content.clone(),
                                ChatMessageStatus::Failed,
                            )
                            .await
                    } else {
                        Ok(())
                    };
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            }
        }

        assistant_message.content = assistant_content;
        assistant_message.status = ChatMessageStatus::Done;
        let save_result = if assistant_persisted {
            store.update_conversation_message(
                conversation_id,
                assistant_message_id,
                assistant_message.content.clone(),
                ChatMessageStatus::Done,
            )
            .await
        } else if assistant_message.content.is_empty() {
            Ok(())
        } else {
            store.append_conversation_messages(conversation_id, std::slice::from_ref(&assistant_message)).await
        };

        match save_result
        {
            Ok(()) => {
                update_cleanup_state(
                    &cleanup_state,
                    !assistant_message.content.is_empty(),
                    true,
                    assistant_message.content.clone(),
                );
                if !assistant_persisted && !assistant_message.content.is_empty() {
                    yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                        Event::default().event("error").data("failed to serialize assistant message")
                    }));
                } else {
                    if !pending_content.is_empty() {
                        yield Ok(Event::default().event("delta").data(std::mem::take(&mut pending_content)));
                    }
                    yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                        Event::default().event("error").data("failed to serialize assistant message")
                    }));
                }
                yield Ok(Event::default().event("done").data("ok"));
            }
            Err(error) => {
                yield Ok(Event::default().event("error").data(error.to_string()));
            }
        }
    };

    Ok(Sse::new(stream))
}
