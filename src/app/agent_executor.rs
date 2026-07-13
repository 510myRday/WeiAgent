use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use chrono::Utc;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    app::{AppState, chat::PreparedChat},
    domain::{ChatMessage, ChatMessageStatus, ChatRole, SendMessageResponse, ToolResult},
    error::{AppError, AppResult},
    llm::{
        client::{DynChatClient, RequestOptions},
        types::{ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction},
    },
    tools::registry::{ToolExecutionContext, ToolExecutionEvent},
};

const STREAM_SAVE_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_SAVE_CHARS: usize = 240;
const MAX_TOOL_ITERATIONS: usize = 10;

#[derive(Debug, Clone)]
pub enum AgentRunEvent {
    User(ChatMessage),
    Assistant(ChatMessage),
    Delta(String),
    ToolCall {
        call_id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        result: ToolResult,
    },
    ApprovalRequired {
        call_id: String,
        command: String,
        approval_id: String,
    },
    Done,
    Error(String),
}

pub async fn run_agent(
    state: AppState,
    prepared: PreparedChat,
    options: RequestOptions,
    use_streaming_llm: bool,
    events: Option<mpsc::UnboundedSender<AgentRunEvent>>,
) -> AppResult<SendMessageResponse> {
    let client = state.llm_registry.client_for(&prepared.provider)?;
    let tools = state.tool_registry.definitions(&prepared.tools_config);
    let mut messages = prepared.messages_for_llm.clone();
    let user_message = prepared.user_message.clone();
    state
        .store
        .append_conversation_messages(
            user_message.conversation_id,
            std::slice::from_ref(&user_message),
        )
        .await?;
    emit(&events, AgentRunEvent::User(user_message.clone()))?;

    for _iteration in 0..MAX_TOOL_ITERATIONS {
        let output = if use_streaming_llm {
            stream_llm_iteration(
                &state,
                &client,
                &prepared,
                &messages,
                options,
                tools.clone(),
                &events,
            )
            .await?
        } else {
            IterationOutput {
                message_id: Uuid::new_v4(),
                created_at: Utc::now(),
                output: client
                    .complete(
                        &prepared.agent,
                        &prepared.provider,
                        &messages,
                        &prepared.system_prompt,
                        options,
                        tools.clone(),
                    )
                    .await?,
            }
        };

        let assistant_message = ChatMessage {
            id: output.message_id,
            agent_id: prepared.agent.id,
            conversation_id: user_message.conversation_id,
            role: ChatRole::Assistant,
            content: output.output.content.clone(),
            status: ChatMessageStatus::Done,
            created_at: output.created_at,
            tool_calls: (!output.output.tool_calls.is_empty()).then(|| {
                serde_json::to_value(&output.output.tool_calls).unwrap_or(Value::Array(Vec::new()))
            }),
            tool_call_id: None,
        };
        state
            .store
            .upsert_conversation_message(user_message.conversation_id, assistant_message.clone())
            .await?;

        if output.output.tool_calls.is_empty() {
            emit(&events, AgentRunEvent::Assistant(assistant_message.clone()))?;
            emit(&events, AgentRunEvent::Done)?;
            return Ok(SendMessageResponse {
                user: user_message,
                assistant: assistant_message,
            });
        }

        if !assistant_message.content.is_empty() {
            emit(&events, AgentRunEvent::Assistant(assistant_message.clone()))?;
        }
        messages.push(assistant_message);

        for call in output.output.tool_calls {
            let arguments = match serde_json::from_str::<Value>(&call.function.arguments) {
                Ok(Value::Object(map)) => Value::Object(map),
                Ok(_) => Value::Object(Default::default()),
                Err(error) => {
                    let result = ToolResult::Error {
                        message: format!("Invalid tool arguments: {error}"),
                    };
                    persist_tool_result(&state, &prepared, &mut messages, &call.id, &result)
                        .await?;
                    emit(
                        &events,
                        AgentRunEvent::ToolResult {
                            call_id: call.id,
                            result,
                        },
                    )?;
                    continue;
                }
            };
            emit(
                &events,
                AgentRunEvent::ToolCall {
                    call_id: call.id.clone(),
                    name: call.function.name.clone(),
                    arguments: arguments.clone(),
                },
            )?;

            let context = ToolExecutionContext {
                agent_id: prepared.agent.id,
                conversation_id: user_message.conversation_id,
                call_id: call.id.clone(),
                tools_config: prepared.tools_config.clone(),
                store: state.store.clone(),
                process_manager: state.process_manager.clone(),
                approval_manager: state.approval_manager.clone(),
                event_sender: None,
            };
            let result =
                execute_tool_with_events(&state, &call.function.name, arguments, context, &events)
                    .await;
            emit(
                &events,
                AgentRunEvent::ToolResult {
                    call_id: call.id.clone(),
                    result: result.clone(),
                },
            )?;
            persist_tool_result(&state, &prepared, &mut messages, &call.id, &result).await?;
        }
    }

    Err(AppError::Llm(format!(
        "tool execution exceeded the maximum of {MAX_TOOL_ITERATIONS} LLM iterations"
    )))
}

struct IterationOutput {
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    output: ChatCompletionOutput,
}

async fn stream_llm_iteration(
    state: &AppState,
    client: &DynChatClient,
    prepared: &PreparedChat,
    messages: &[ChatMessage],
    options: RequestOptions,
    tools: Option<Vec<crate::llm::types::ToolDefinition>>,
    events: &Option<mpsc::UnboundedSender<AgentRunEvent>>,
) -> AppResult<IterationOutput> {
    let message_id = Uuid::new_v4();
    let created_at = Utc::now();
    let mut content = String::new();
    let mut persisted = false;
    let mut persisted_chars = 0_usize;
    let mut last_save = Instant::now();
    let mut calls: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();
    let mut stream = client
        .stream(
            &prepared.agent,
            &prepared.provider,
            messages,
            &prepared.system_prompt,
            options,
            tools,
        )
        .await?;

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta(delta) => {
                content.push_str(&delta);
                if emit(events, AgentRunEvent::Delta(delta)).is_err() {
                    mark_interrupted(state, prepared, message_id, created_at, &content, persisted)
                        .await;
                    return Err(AppError::Llm("stream client disconnected".to_string()));
                }
                if !persisted
                    || content.chars().count().saturating_sub(persisted_chars) >= STREAM_SAVE_CHARS
                    || last_save.elapsed() >= STREAM_SAVE_INTERVAL
                {
                    let message = partial_assistant(prepared, message_id, created_at, &content);
                    state
                        .store
                        .upsert_conversation_message(message.conversation_id, message)
                        .await?;
                    persisted = true;
                    persisted_chars = content.chars().count();
                    last_save = Instant::now();
                }
            }
            StreamEvent::ToolCallStart { index, id, name } => {
                let builder = calls.entry(index).or_default();
                builder.id = id;
                builder.name = name;
            }
            StreamEvent::ToolCallArgsDelta { index, arguments } => {
                calls
                    .entry(index)
                    .or_default()
                    .arguments
                    .push_str(&arguments);
            }
            StreamEvent::FinishedWithToolCalls | StreamEvent::Finished => {}
        }
    }

    Ok(IterationOutput {
        message_id,
        created_at,
        output: ChatCompletionOutput {
            content,
            tool_calls: calls
                .into_values()
                .filter(|builder| !builder.id.is_empty() && !builder.name.is_empty())
                .map(|builder| ToolCall {
                    id: builder.id,
                    kind: "function".to_string(),
                    function: ToolCallFunction {
                        name: builder.name,
                        arguments: builder.arguments,
                    },
                })
                .collect(),
        },
    })
}

#[derive(Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

async fn execute_tool_with_events(
    state: &AppState,
    name: &str,
    arguments: Value,
    mut context: ToolExecutionContext,
    events: &Option<mpsc::UnboundedSender<AgentRunEvent>>,
) -> ToolResult {
    let Some(run_sender) = events.clone() else {
        return state.tool_registry.execute(name, arguments, context).await;
    };
    let (tool_sender, mut tool_receiver) = mpsc::unbounded_channel();
    context.event_sender = Some(tool_sender);
    let execution = state.tool_registry.execute(name, arguments, context);
    tokio::pin!(execution);
    loop {
        tokio::select! {
            result = &mut execution => return result,
            _ = run_sender.closed() => {
                return ToolResult::Rejected {
                    reason: "Approval client disconnected".to_string(),
                };
            }
            event = tool_receiver.recv() => {
                let Some(event) = event else { continue; };
                let mapped = match event {
                    ToolExecutionEvent::ApprovalRequired { call_id, command, approval_id } => {
                        AgentRunEvent::ApprovalRequired { call_id, command, approval_id }
                    }
                };
                if run_sender.send(mapped).is_err() {
                    return ToolResult::Rejected {
                        reason: "Approval client disconnected".to_string(),
                    };
                }
            }
        }
    }
}

async fn persist_tool_result(
    state: &AppState,
    prepared: &PreparedChat,
    messages: &mut Vec<ChatMessage>,
    call_id: &str,
    result: &ToolResult,
) -> AppResult<()> {
    let tool_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: prepared.agent.id,
        conversation_id: prepared.user_message.conversation_id,
        role: ChatRole::Tool,
        content: serde_json::to_string(result)?,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: Some(call_id.to_string()),
    };
    state
        .store
        .append_conversation_messages(
            tool_message.conversation_id,
            std::slice::from_ref(&tool_message),
        )
        .await?;
    messages.push(tool_message);
    Ok(())
}

fn partial_assistant(
    prepared: &PreparedChat,
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    content: &str,
) -> ChatMessage {
    ChatMessage {
        id: message_id,
        agent_id: prepared.agent.id,
        conversation_id: prepared.user_message.conversation_id,
        role: ChatRole::Assistant,
        content: content.to_string(),
        status: ChatMessageStatus::Streaming,
        created_at,
        tool_calls: None,
        tool_call_id: None,
    }
}

async fn mark_interrupted(
    state: &AppState,
    prepared: &PreparedChat,
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    content: &str,
    persisted: bool,
) {
    if !persisted && content.is_empty() {
        return;
    }
    let mut message = partial_assistant(prepared, message_id, created_at, content);
    message.status = ChatMessageStatus::Failed;
    let _ = state
        .store
        .upsert_conversation_message(message.conversation_id, message)
        .await;
}

fn emit(
    events: &Option<mpsc::UnboundedSender<AgentRunEvent>>,
    event: AgentRunEvent,
) -> AppResult<()> {
    if let Some(events) = events {
        events
            .send(event)
            .map_err(|_| AppError::Llm("stream client disconnected".to_string()))?;
    }
    Ok(())
}
