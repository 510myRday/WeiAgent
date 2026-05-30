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
        ApprovalDecision, ChatMessage, ChatMessageStatus, ChatRole, Conversation,
        CreateConversationRequest, PendingApproval, SendMessageRequest, SendMessageResponse,
        ToolResult,
    },
    error::AppResult,
    llm::{
        client::RequestOptions,
        types::{StreamEvent, ToolDefinition},
    },
    storage::JsonStore,
    tools::execute::{ExecuteTerminalRequest, execute_terminal},
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
    let assistant_output = client
        .complete(
            &prepared.agent,
            &prepared.provider,
            &prepared.messages_for_llm,
            &prepared.system_prompt,
            request_options(&settings),
            None,
        )
        .await?;
    let assistant_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: prepared.agent.id,
        conversation_id,
        role: ChatRole::Assistant,
        content: assistant_output.content,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
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

    let store = state.store.clone();
    let process_manager = state.process_manager.clone();
    let approval_manager = state.approval_manager.clone();
    let agent = prepared.agent;
    let provider = prepared.provider;
    let mut messages_for_llm = prepared.messages_for_llm;
    let system_prompt = prepared.system_prompt;
    let user_message = prepared.user_message;
    let options = request_options(&settings);
    let stream_response = request.stream;
    let tools = if prepared.tools_config.terminal.enabled {
        Some(vec![ToolDefinition::execute_terminal()])
    } else {
        None
    };
    let tools_config = prepared.tools_config;
    let client = state.llm_registry.client_for(&provider).unwrap();
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
            tool_calls: None,
            tool_call_id: None,
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

        if !stream_response {
            let output = match client
                .complete(
                    &agent,
                    &provider,
                    &messages_for_llm,
                    &system_prompt,
                    options,
                    tools.clone(),
                )
                .await
            {
                Ok(output) => output,
                Err(error) => {
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            };

            if output.tool_calls.is_empty() {
                assistant_message.content = output.content;
                assistant_message.status = ChatMessageStatus::Done;
                match store
                    .append_conversation_messages(conversation_id, std::slice::from_ref(&assistant_message))
                    .await
                {
                    Ok(()) => {
                        update_cleanup_state(
                            &cleanup_state,
                            true,
                            true,
                            assistant_message.content.clone(),
                        );
                        yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                            Event::default().event("error").data("failed to serialize assistant message")
                        }));
                        yield Ok(Event::default().event("done").data("ok"));
                    }
                    Err(error) => yield Ok(Event::default().event("error").data(error.to_string())),
                }
                return;
            }

            let tool_calls_value = serde_json::to_value(&output.tool_calls)
                .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
            let tool_call_message = ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Assistant,
                content: output.content,
                status: ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: Some(tool_calls_value),
                tool_call_id: None,
            };
            messages_for_llm.push(tool_call_message.clone());
            if let Err(error) = store
                .append_conversation_messages(conversation_id, std::slice::from_ref(&tool_call_message))
                .await
            {
                yield Ok(Event::default().event("error").data(error.to_string()));
                return;
            }

            for call in &output.tool_calls {
                if call.function.name != "execute_terminal" {
                    continue;
                }

                let call_id = call.id.clone();
                let args_value: serde_json::Value = match serde_json::from_str(&call.function.arguments) {
                    Ok(value) => value,
                    Err(error) => {
                        let result = ToolResult::Error {
                            message: format!("Invalid tool arguments: {error}"),
                        };
                        let result_json = serde_json::json!({"call_id": call_id, "result": result.clone()});
                        yield Ok(Event::default().event("tool_result").json_data(&result_json).unwrap_or_else(|_| {
                            Event::default().event("error").data("failed to serialize tool_result")
                        }));
                        let tool_message = ChatMessage {
                            id: Uuid::new_v4(),
                            agent_id: agent.id,
                            conversation_id,
                            role: ChatRole::Tool,
                            content: serde_json::to_string(&result).unwrap_or_default(),
                            status: ChatMessageStatus::Done,
                            created_at: Utc::now(),
                            tool_calls: None,
                            tool_call_id: Some(call_id.clone()),
                        };
                        messages_for_llm.push(tool_message.clone());
                        let _ = store.append_conversation_messages(conversation_id, std::slice::from_ref(&tool_message)).await;
                        continue;
                    }
                };

                let tc_data = serde_json::json!({ "call_id": call_id, "name": call.function.name, "arguments": args_value });
                yield Ok(Event::default().event("tool_call").json_data(&tc_data).unwrap_or_else(|_| {
                    Event::default().event("error").data("failed to serialize tool_call")
                }));

                let command = args_value.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cwd = args_value.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());
                let mut result = execute_terminal(
                    &ExecuteTerminalRequest {
                        command: command.clone(),
                        cwd: cwd.clone(),
                        timeout_ms: None,
                        background: None,
                        approved: false,
                    },
                    &tools_config.terminal,
                    &process_manager,
                )
                .await;

                if let ToolResult::PendingApproval { approval_id, .. } = &result {
                    let approval_id = approval_id.clone();
                    let decision_rx = approval_manager.register(PendingApproval {
                        approval_id: approval_id.clone(),
                        command: command.clone(),
                        cwd: cwd.clone(),
                        agent_id: agent.id,
                        created_at: std::time::Instant::now(),
                    });
                    let approval_data = serde_json::json!({"call_id": call_id, "command": command, "approval_id": approval_id});
                    yield Ok(Event::default().event("approval_required").json_data(&approval_data).unwrap_or_else(|_| {
                        Event::default().event("error").data("failed to serialize approval_required")
                    }));

                    match tokio::time::timeout(std::time::Duration::from_secs(600), decision_rx).await {
                        Ok(Ok(ApprovalDecision::Approve | ApprovalDecision::AlwaysApprove)) => {
                            result = execute_terminal(
                                &ExecuteTerminalRequest {
                                    command: command.clone(),
                                    cwd: cwd.clone(),
                                    timeout_ms: None,
                                    background: None,
                                    approved: true,
                                },
                                &tools_config.terminal,
                                &process_manager,
                            )
                            .await;
                        }
                        Ok(Ok(ApprovalDecision::Reject)) => {
                            result = ToolResult::Rejected {
                                reason: "Command rejected by user".to_string(),
                            };
                        }
                        Ok(Err(_)) => {
                            result = ToolResult::Rejected {
                                reason: "Approval channel closed".to_string(),
                            };
                        }
                        Err(_) => {
                            result = ToolResult::Rejected {
                                reason: "Approval timed out".to_string(),
                            };
                        }
                    }
                }

                let result_json = serde_json::json!({"call_id": call_id, "result": result.clone()});
                yield Ok(Event::default().event("tool_result").json_data(&result_json).unwrap_or_else(|_| {
                    Event::default().event("error").data("failed to serialize tool_result")
                }));
                let tool_message = ChatMessage {
                    id: Uuid::new_v4(),
                    agent_id: agent.id,
                    conversation_id,
                    role: ChatRole::Tool,
                    content: serde_json::to_string(&result).unwrap_or_default(),
                    status: ChatMessageStatus::Done,
                    created_at: Utc::now(),
                    tool_calls: None,
                    tool_call_id: Some(call_id.clone()),
                };
                messages_for_llm.push(tool_message.clone());
                if let Err(error) = store
                    .append_conversation_messages(conversation_id, std::slice::from_ref(&tool_message))
                    .await
                {
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            }

            let follow_up = match client
                .complete(&agent, &provider, &messages_for_llm, &system_prompt, options, tools.clone())
                .await
            {
                Ok(follow_up) => follow_up,
                Err(error) => {
                    yield Ok(Event::default().event("error").data(format!("Follow-up LLM call failed: {}", error)));
                    return;
                }
            };
            assistant_message = ChatMessage {
                id: Uuid::new_v4(),
                agent_id: agent.id,
                conversation_id,
                role: ChatRole::Assistant,
                content: follow_up.content,
                status: ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: None,
                tool_call_id: None,
            };
            match store
                .append_conversation_messages(conversation_id, std::slice::from_ref(&assistant_message))
                .await
            {
                Ok(()) => {
                    update_cleanup_state(
                        &cleanup_state,
                        true,
                        true,
                        assistant_message.content.clone(),
                    );
                    yield Ok(Event::default().event("assistant").json_data(&assistant_message).unwrap_or_else(|_| {
                        Event::default().event("error").data("failed to serialize assistant message")
                    }));
                    yield Ok(Event::default().event("done").data("ok"));
                }
                Err(error) => yield Ok(Event::default().event("error").data(error.to_string())),
            }
            return;
        }

        // Accumulate tool calls across stream chunks
        let mut pending_tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args_accumulated)
        let mut has_tool_calls = false;

        let mut llm_stream = match client
            .stream(
                &agent,
                &provider,
                &messages_for_llm,
                &system_prompt,
                options,
                tools.clone(),
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
                Ok(event) => match event {
                    StreamEvent::TextDelta(content) => {
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
                    StreamEvent::ToolCallStart { index, id, name } => {
                        // Ensure vec is large enough
                        while pending_tool_calls.len() <= index {
                            pending_tool_calls.push((String::new(), String::new(), String::new()));
                        }
                        pending_tool_calls[index] = (id, name, String::new());
                    }
                    StreamEvent::ToolCallArgsDelta { index, arguments } => {
                        if let Some(tc) = pending_tool_calls.get_mut(index) {
                            tc.2.push_str(&arguments);
                        }
                    }
                    StreamEvent::FinishedWithToolCalls => {
                        has_tool_calls = true;
                    }
                    StreamEvent::Finished => {}
                },
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

        // Process tool calls if any
        if has_tool_calls && !pending_tool_calls.is_empty() {
            let assistant_tool_calls = pending_tool_calls
                .iter()
                .filter(|(_, name, _)| name == "execute_terminal")
                .map(|(id, name, arguments)| serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments,
                    }
                }))
                .collect::<Vec<_>>();

            if !assistant_tool_calls.is_empty() {
                if assistant_persisted {
                    if let Err(error) = store
                        .update_conversation_message(
                            conversation_id,
                            assistant_message_id,
                            assistant_content.clone(),
                            ChatMessageStatus::Done,
                        )
                        .await
                    {
                        yield Ok(Event::default().event("error").data(error.to_string()));
                        return;
                    }
                }
                let tool_call_message = ChatMessage {
                    id: Uuid::new_v4(),
                    agent_id: agent.id,
                    conversation_id,
                    role: ChatRole::Assistant,
                    content: assistant_content.clone(),
                    status: ChatMessageStatus::Done,
                    created_at: Utc::now(),
                    tool_calls: Some(serde_json::Value::Array(assistant_tool_calls)),
                    tool_call_id: None,
                };
                messages_for_llm.push(tool_call_message.clone());
                if let Err(error) = store
                    .append_conversation_messages(conversation_id, std::slice::from_ref(&tool_call_message))
                    .await
                {
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            }

            for (tc_id, tc_name, tc_args) in &pending_tool_calls {
                if tc_name != "execute_terminal" { continue; }

                let call_id = tc_id.clone();
                let args_value: serde_json::Value = match serde_json::from_str(tc_args) {
                    Ok(value) => value,
                    Err(error) => {
                        let result = ToolResult::Error {
                            message: format!("Invalid tool arguments: {error}"),
                        };
                        let result_json = serde_json::json!({"call_id": call_id, "result": result.clone()});
                        yield Ok(Event::default().event("tool_result").json_data(&result_json).unwrap_or_else(|_| {
                            Event::default().event("error").data("failed to serialize tool_result")
                        }));
                        let tool_message = ChatMessage {
                            id: Uuid::new_v4(),
                            agent_id: agent.id,
                            conversation_id,
                            role: ChatRole::Tool,
                            content: serde_json::to_string(&result).unwrap_or_default(),
                            status: ChatMessageStatus::Done,
                            created_at: Utc::now(),
                            tool_calls: None,
                            tool_call_id: Some(call_id.clone()),
                        };
                        messages_for_llm.push(tool_message.clone());
                        let _ = store.append_conversation_messages(conversation_id, std::slice::from_ref(&tool_message)).await;
                        continue;
                    }
                };

                // Emit tool_call event to frontend
                let tc_data = serde_json::json!({ "call_id": call_id, "name": tc_name, "arguments": args_value });
                yield Ok(Event::default().event("tool_call").json_data(&tc_data).unwrap_or_else(|_| {
                    Event::default().event("error").data("failed to serialize tool_call")
                }));

                let command = args_value.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cwd = args_value.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());

                let mut result = execute_terminal(
                    &ExecuteTerminalRequest {
                        command: command.clone(),
                        cwd: cwd.clone(),
                        timeout_ms: None,
                        background: None,
                        approved: false,
                    },
                    &tools_config.terminal,
                    &process_manager,
                )
                .await;

                if let ToolResult::PendingApproval { approval_id, .. } = &result {
                    let approval_id = approval_id.clone();
                    let decision_rx = approval_manager.register(PendingApproval {
                        approval_id: approval_id.clone(),
                        command: command.clone(),
                        cwd: cwd.clone(),
                        agent_id: agent.id,
                        created_at: std::time::Instant::now(),
                    });
                    let approval_data = serde_json::json!({"call_id": call_id, "command": command, "approval_id": approval_id});
                    yield Ok(Event::default().event("approval_required").json_data(&approval_data).unwrap_or_else(|_| {
                        Event::default().event("error").data("failed to serialize approval_required")
                    }));

                    match tokio::time::timeout(std::time::Duration::from_secs(600), decision_rx).await {
                        Ok(Ok(ApprovalDecision::Approve | ApprovalDecision::AlwaysApprove)) => {
                            result = execute_terminal(
                                &ExecuteTerminalRequest {
                                    command: command.clone(),
                                    cwd: cwd.clone(),
                                    timeout_ms: None,
                                    background: None,
                                    approved: true,
                                },
                                &tools_config.terminal,
                                &process_manager,
                            )
                            .await;
                        }
                        Ok(Ok(ApprovalDecision::Reject)) => {
                            result = ToolResult::Rejected {
                                reason: "Command rejected by user".to_string(),
                            };
                        }
                        Ok(Err(_)) => {
                            result = ToolResult::Rejected {
                                reason: "Approval channel closed".to_string(),
                            };
                        }
                        Err(_) => {
                            result = ToolResult::Rejected {
                                reason: "Approval timed out".to_string(),
                            };
                        }
                    }
                }

                let result_json = serde_json::json!({"call_id": call_id, "result": result.clone()});
                yield Ok(Event::default().event("tool_result").json_data(&result_json).unwrap_or_else(|_| {
                    Event::default().event("error").data("failed to serialize tool_result")
                }));
                let tool_message = ChatMessage {
                    id: Uuid::new_v4(),
                    agent_id: agent.id,
                    conversation_id,
                    role: ChatRole::Tool,
                    content: serde_json::to_string(&result).unwrap_or_default(),
                    status: ChatMessageStatus::Done,
                    created_at: Utc::now(),
                    tool_calls: None,
                    tool_call_id: Some(call_id.clone()),
                };
                messages_for_llm.push(tool_message.clone());
                if let Err(error) = store
                    .append_conversation_messages(conversation_id, std::slice::from_ref(&tool_message))
                    .await
                {
                    yield Ok(Event::default().event("error").data(error.to_string()));
                    return;
                }
            }

            // Second LLM call with tool results (non-streaming for simplicity)
            match client.complete(&agent, &provider, &messages_for_llm, &system_prompt, options, tools.clone()).await {
                Ok(follow_up) => {
                    assistant_content = follow_up.content;
                    assistant_message = ChatMessage {
                        id: Uuid::new_v4(),
                        agent_id: agent.id,
                        conversation_id,
                        role: ChatRole::Assistant,
                        content: assistant_content.clone(),
                        status: ChatMessageStatus::Done,
                        created_at: Utc::now(),
                        tool_calls: None,
                        tool_call_id: None,
                    };
                    assistant_persisted = false;
                    persisted_assistant_content.clear();
                    pending_content.clear();
                }
                Err(e) => {
                    yield Ok(Event::default().event("error").data(format!("Follow-up LLM call failed: {}", e)));
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
