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
        types::{ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition},
    },
    tools::registry::{ToolExecutionContext, ToolExecutionEvent},
};

const STREAM_SAVE_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_SAVE_CHARS: usize = 240;

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
    let max_llm_iterations = prepared.tools_config.max_llm_iterations();
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

    for iteration in 0..max_llm_iterations {
        let output = run_llm_iteration(
            &state,
            &client,
            &prepared,
            &messages,
            options,
            tools.clone(),
            use_streaming_llm,
            &events,
        )
        .await?;

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
        execute_tool_calls(
            &state,
            &prepared,
            &user_message,
            &mut messages,
            output.output.tool_calls,
            &events,
        )
        .await?;

        // Final reserved turn after the last tool-capable iteration.
        if iteration + 1 == max_llm_iterations {
            break;
        }
    }

    let summary = finalize_after_budget(
        &state,
        &client,
        &prepared,
        &mut messages,
        options,
        use_streaming_llm,
        max_llm_iterations,
        &events,
    )
    .await?;
    emit(&events, AgentRunEvent::Assistant(summary.clone()))?;
    emit(&events, AgentRunEvent::Done)?;
    Ok(SendMessageResponse {
        user: user_message,
        assistant: summary,
    })
}

async fn finalize_after_budget(
    state: &AppState,
    client: &DynChatClient,
    prepared: &PreparedChat,
    messages: &mut Vec<ChatMessage>,
    options: RequestOptions,
    use_streaming_llm: bool,
    max_llm_iterations: usize,
    events: &Option<mpsc::UnboundedSender<AgentRunEvent>>,
) -> AppResult<ChatMessage> {
    let budget_notice = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: prepared.agent.id,
        conversation_id: prepared.user_message.conversation_id,
        role: ChatRole::User,
        content: format!(
            "System notice: tool-capable iterations reached the configured limit of {max_llm_iterations}. Do not call tools. Summarize completed work, failures, and remaining steps."
        ),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
    };
    // Keep the budget notice in the in-memory LLM context only.
    messages.push(budget_notice);

    let output = run_llm_iteration(
        state,
        client,
        prepared,
        messages,
        options,
        None,
        use_streaming_llm,
        events,
    )
    .await?;

    let mut content = output.output.content;
    if content.trim().is_empty() {
        content = format!(
            "Tool execution stopped after {max_llm_iterations} tool-capable LLM iterations. Review the tool results above for completed work and remaining steps."
        );
    }

    let assistant_message = ChatMessage {
        id: output.message_id,
        agent_id: prepared.agent.id,
        conversation_id: prepared.user_message.conversation_id,
        role: ChatRole::Assistant,
        content,
        status: ChatMessageStatus::Done,
        created_at: output.created_at,
        tool_calls: None,
        tool_call_id: None,
    };
    state
        .store
        .upsert_conversation_message(
            prepared.user_message.conversation_id,
            assistant_message.clone(),
        )
        .await?;
    Ok(assistant_message)
}

async fn execute_tool_calls(
    state: &AppState,
    prepared: &PreparedChat,
    user_message: &ChatMessage,
    messages: &mut Vec<ChatMessage>,
    tool_calls: Vec<ToolCall>,
    events: &Option<mpsc::UnboundedSender<AgentRunEvent>>,
) -> AppResult<()> {
    for call in tool_calls {
        let arguments = match serde_json::from_str::<Value>(&call.function.arguments) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(other) => {
                let result = ToolResult::Error {
                    message: format!(
                        "Invalid tool arguments: expected a JSON object, got {other}"
                    ),
                };
                persist_tool_result(state, prepared, messages, &call.id, &result).await?;
                emit(
                    events,
                    AgentRunEvent::ToolResult {
                        call_id: call.id,
                        result,
                    },
                )?;
                continue;
            }
            Err(error) => {
                let result = ToolResult::Error {
                    message: format!("Invalid tool arguments: {error}"),
                };
                persist_tool_result(state, prepared, messages, &call.id, &result).await?;
                emit(
                    events,
                    AgentRunEvent::ToolResult {
                        call_id: call.id,
                        result,
                    },
                )?;
                continue;
            }
        };
        emit(
            events,
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
            execute_tool_with_events(state, &call.function.name, arguments, context, events).await;
        emit(
            events,
            AgentRunEvent::ToolResult {
                call_id: call.id.clone(),
                result: result.clone(),
            },
        )?;
        persist_tool_result(state, prepared, messages, &call.id, &result).await?;
    }
    Ok(())
}

struct IterationOutput {
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    output: ChatCompletionOutput,
}

async fn run_llm_iteration(
    state: &AppState,
    client: &DynChatClient,
    prepared: &PreparedChat,
    messages: &[ChatMessage],
    options: RequestOptions,
    tools: Option<Vec<ToolDefinition>>,
    use_streaming_llm: bool,
    events: &Option<mpsc::UnboundedSender<AgentRunEvent>>,
) -> AppResult<IterationOutput> {
    if use_streaming_llm {
        stream_llm_iteration(state, client, prepared, messages, options, tools, events).await
    } else {
        Ok(IterationOutput {
            message_id: Uuid::new_v4(),
            created_at: Utc::now(),
            output: client
                .complete(
                    &prepared.agent,
                    &prepared.provider,
                    messages,
                    &prepared.system_prompt,
                    options,
                    tools,
                )
                .await?,
        })
    }
}

async fn stream_llm_iteration(
    state: &AppState,
    client: &DynChatClient,
    prepared: &PreparedChat,
    messages: &[ChatMessage],
    options: RequestOptions,
    tools: Option<Vec<ToolDefinition>>,
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
