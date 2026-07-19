use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};

use chrono::Utc;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    app::{AppState, chat::PreparedChat},
    domain::{
        ChatMessage, ChatMessageStatus, ChatRole, ModelUsage, SendMessageResponse,
        TerminalPermissionMode, TokenUsage, ToolResult,
    },
    error::{AppError, AppResult},
    llm::{
        client::{DynChatClient, RequestOptions},
        types::{ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition},
    },
    tools::registry::{ToolExecutionContext, ToolExecutionEvent},
};

const STREAM_SAVE_INTERVAL: Duration = Duration::from_secs(2);
const STREAM_SAVE_BYTES: usize = 32 * 1024;
const MAX_TOOL_CALLS_PER_ITERATION: usize = 8;
const MAX_TOTAL_TOOL_CALLS: usize = 256;
const MAX_TOOL_CALL_ID_BYTES: usize = 256;
const MAX_TOOL_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_STREAM_CONTENT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunPhase {
    WaitingForModel,
    ExecutingTool,
    WaitingForApproval,
}

#[derive(Debug, Clone)]
pub enum AgentRunEvent {
    User(ChatMessage),
    Assistant(ChatMessage),
    Delta(String),
    ReasoningDelta(String),
    Phase {
        phase: AgentRunPhase,
        iteration: usize,
        call_id: Option<String>,
    },
    /// LLM HTTP request is about to be retried after a timeout/error.
    Retry {
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: Value,
        permission_mode: TerminalPermissionMode,
    },
    ToolResult {
        call_id: String,
        result: ToolResult,
    },
    ApprovalRequired {
        call_id: String,
        agent_id: Uuid,
        command: String,
        approval_id: String,
        cwd: Option<String>,
        runtime: String,
        permission: String,
        justification: Option<String>,
        background: bool,
    },
    Done,
    Cancelled,
    Error(String),
}

struct AgentRuntime<'a> {
    state: &'a AppState,
    client: &'a DynChatClient,
    prepared: &'a PreparedChat,
    options: RequestOptions,
    use_streaming_llm: bool,
    events: &'a Option<mpsc::Sender<AgentRunEvent>>,
}

#[derive(Default)]
struct ToolBudget {
    seen_call_ids: HashSet<String>,
    total_calls: usize,
}

pub async fn run_agent(
    state: AppState,
    prepared: PreparedChat,
    options: RequestOptions,
    use_streaming_llm: bool,
    events: Option<mpsc::Sender<AgentRunEvent>>,
) -> AppResult<SendMessageResponse> {
    let client = state.llm_registry.client_for(&prepared.provider)?;
    let tools = state.tool_registry.definitions(&prepared.tools_config);
    let max_llm_iterations = prepared.tools_config.max_llm_iterations();
    let mut messages = prepared.messages_for_llm.clone();
    let user_message = prepared.user_message.clone();
    let mut tool_budget = ToolBudget::default();
    state
        .store
        .append_conversation_messages(
            user_message.conversation_id,
            std::slice::from_ref(&user_message),
        )
        .await?;
    emit(&events, AgentRunEvent::User(user_message.clone())).await?;
    let mut options = options;
    if let Some(sender) = events.clone() {
        options.on_retry = Some(std::sync::Arc::new(move |attempt, max_attempts, reason| {
            let _ = sender.try_send(AgentRunEvent::Retry {
                attempt,
                max_attempts,
                reason: reason.to_string(),
            });
        }));
    }
    let runtime = AgentRuntime {
        state: &state,
        client: &client,
        prepared: &prepared,
        options,
        use_streaming_llm,
        events: &events,
    };

    for iteration in 0..max_llm_iterations {
        emit(
            &events,
            AgentRunEvent::Phase {
                phase: AgentRunPhase::WaitingForModel,
                iteration,
                call_id: None,
            },
        )
        .await?;
        let output = match run_llm_iteration(&runtime, &messages, tools.clone()).await {
            Ok(output) => output,
            Err(error) => {
                if events.as_ref().is_some_and(|sender| sender.is_closed()) {
                    state
                        .terminal_manager
                        .stop_for_conversation(user_message.conversation_id)
                        .await;
                }
                persist_run_failure(&runtime, &error).await;
                return Err(error);
            }
        };

        let assistant_message = ChatMessage {
            id: output.message_id,
            agent_id: prepared.agent.id,
            conversation_id: user_message.conversation_id,
            role: ChatRole::Assistant,
            content: output.output.content.clone(),
            reasoning_content: (!output.output.reasoning_content.is_empty())
                .then_some(output.output.reasoning_content.clone()),
            status: ChatMessageStatus::Done,
            created_at: output.created_at,
            tool_calls: (!output.output.tool_calls.is_empty()).then(|| {
                tool_calls_with_permission_mode(
                    &output.output.tool_calls,
                    &prepared.tools_config.terminal.permission_mode,
                )
            }),
            tool_call_id: None,
            attachments: Vec::new(),
            usage: Some(model_usage(&prepared, &output.output.usage)),
        };
        state
            .store
            .upsert_conversation_message(user_message.conversation_id, assistant_message.clone())
            .await?;

        if output.output.tool_calls.is_empty() {
            emit(&events, AgentRunEvent::Assistant(assistant_message.clone())).await?;
            emit(&events, AgentRunEvent::Done).await?;
            return Ok(SendMessageResponse {
                user: user_message,
                assistant: assistant_message,
            });
        }

        if !assistant_message.content.is_empty() || assistant_message.reasoning_content.is_some() {
            emit(&events, AgentRunEvent::Assistant(assistant_message.clone())).await?;
        }
        messages.push(assistant_message);
        if let Err(error) = execute_tool_calls(
            &state,
            &prepared,
            &mut messages,
            output.output.tool_calls,
            &mut tool_budget,
            &events,
            iteration,
        )
        .await
        {
            persist_run_failure(&runtime, &error).await;
            return Err(error);
        }

        // Final reserved turn after the last tool-capable iteration.
        if iteration + 1 == max_llm_iterations {
            break;
        }
    }

    let summary = match finalize_after_budget(&runtime, &mut messages, max_llm_iterations).await {
        Ok(summary) => summary,
        Err(error) => {
            persist_run_failure(&runtime, &error).await;
            return Err(error);
        }
    };
    emit(&events, AgentRunEvent::Assistant(summary.clone())).await?;
    emit(&events, AgentRunEvent::Done).await?;
    Ok(SendMessageResponse {
        user: user_message,
        assistant: summary,
    })
}

async fn persist_run_failure(runtime: &AgentRuntime<'_>, error: &AppError) {
    let assistant_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: runtime.prepared.agent.id,
        conversation_id: runtime.prepared.user_message.conversation_id,
        role: ChatRole::Assistant,
        content: format!("Agent run stopped: {error}"),
        reasoning_content: None,
        status: ChatMessageStatus::Failed,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
    };
    let _ = runtime
        .state
        .store
        .upsert_conversation_message(assistant_message.conversation_id, assistant_message.clone())
        .await;
    let _ = emit(runtime.events, AgentRunEvent::Assistant(assistant_message)).await;
}

async fn finalize_after_budget(
    runtime: &AgentRuntime<'_>,
    messages: &mut Vec<ChatMessage>,
    max_llm_iterations: usize,
) -> AppResult<ChatMessage> {
    let budget_notice = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: runtime.prepared.agent.id,
        conversation_id: runtime.prepared.user_message.conversation_id,
        role: ChatRole::User,
        content: format!(
            "System notice: tool-capable iterations reached the configured limit of {max_llm_iterations}. Do not call tools. Summarize completed work, failures, and remaining steps."
        ),
        reasoning_content: None,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
    };
    // Keep the budget notice in the in-memory LLM context only.
    messages.push(budget_notice);

    emit(
        runtime.events,
        AgentRunEvent::Phase {
            phase: AgentRunPhase::WaitingForModel,
            iteration: max_llm_iterations,
            call_id: None,
        },
    )
    .await?;
    let output = run_llm_iteration(runtime, messages, None).await?;

    let mut content = output.output.content;
    if content.trim().is_empty() {
        content = format!(
            "Tool execution stopped after {max_llm_iterations} tool-capable LLM iterations. Review the tool results above for completed work and remaining steps."
        );
    }

    let assistant_message = ChatMessage {
        id: output.message_id,
        agent_id: runtime.prepared.agent.id,
        conversation_id: runtime.prepared.user_message.conversation_id,
        role: ChatRole::Assistant,
        content,
        reasoning_content: (!output.output.reasoning_content.is_empty())
            .then_some(output.output.reasoning_content),
        status: ChatMessageStatus::Done,
        created_at: output.created_at,
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: Some(model_usage(runtime.prepared, &output.output.usage)),
    };
    runtime
        .state
        .store
        .upsert_conversation_message(
            runtime.prepared.user_message.conversation_id,
            assistant_message.clone(),
        )
        .await?;
    Ok(assistant_message)
}

async fn execute_tool_calls(
    state: &AppState,
    prepared: &PreparedChat,
    messages: &mut Vec<ChatMessage>,
    tool_calls: Vec<ToolCall>,
    budget: &mut ToolBudget,
    events: &Option<mpsc::Sender<AgentRunEvent>>,
    iteration: usize,
) -> AppResult<()> {
    for (index, call) in tool_calls.into_iter().enumerate() {
        let parsed_arguments = serde_json::from_str::<Value>(&call.function.arguments);
        let display_arguments = parsed_arguments
            .as_ref()
            .ok()
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        emit(
            events,
            AgentRunEvent::Phase {
                phase: AgentRunPhase::ExecutingTool,
                iteration,
                call_id: Some(call.id.clone()),
            },
        )
        .await?;
        emit(
            events,
            AgentRunEvent::ToolCall {
                call_id: call.id.clone(),
                name: call.function.name.clone(),
                arguments: display_arguments,
                permission_mode: prepared.tools_config.terminal.permission_mode.clone(),
            },
        )
        .await?;

        let validation_error = if call.id.is_empty() {
            Some("tool call id cannot be empty".to_string())
        } else if call.id.len() > MAX_TOOL_CALL_ID_BYTES {
            Some(format!(
                "tool call id exceeds {MAX_TOOL_CALL_ID_BYTES} bytes"
            ))
        } else if !budget.seen_call_ids.insert(call.id.clone()) {
            Some("duplicate tool call id in this run".to_string())
        } else if index >= MAX_TOOL_CALLS_PER_ITERATION {
            Some(format!(
                "tool call limit per iteration ({MAX_TOOL_CALLS_PER_ITERATION}) exceeded"
            ))
        } else if budget.total_calls >= MAX_TOTAL_TOOL_CALLS {
            Some(format!(
                "total tool call limit ({MAX_TOTAL_TOOL_CALLS}) exceeded"
            ))
        } else {
            budget.total_calls += 1;
            None
        };
        if let Some(message) = validation_error {
            let result = ToolResult::Error { message };
            persist_tool_result(state, prepared, messages, &call.id, &result).await?;
            emit(
                events,
                AgentRunEvent::ToolResult {
                    call_id: call.id,
                    result,
                },
            )
            .await?;
            continue;
        }
        let arguments = match parsed_arguments {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(other) => {
                let result = ToolResult::Error {
                    message: format!("Invalid tool arguments: expected a JSON object, got {other}"),
                };
                persist_tool_result(state, prepared, messages, &call.id, &result).await?;
                emit(
                    events,
                    AgentRunEvent::ToolResult {
                        call_id: call.id,
                        result,
                    },
                )
                .await?;
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
                )
                .await?;
                continue;
            }
        };

        let context = ToolExecutionContext {
            agent_id: prepared.agent.id,
            conversation_id: prepared.user_message.conversation_id,
            call_id: call.id.clone(),
            tools_config: prepared.tools_config.clone(),
            store: state.store.clone(),
            terminal_manager: state.terminal_manager.clone(),
            approval_manager: state.approval_manager.clone(),
            event_sender: None,
        };
        let result = execute_tool_with_events(
            state,
            &call.function.name,
            arguments,
            context,
            events,
            iteration,
        )
        .await;
        emit(
            events,
            AgentRunEvent::ToolResult {
                call_id: call.id.clone(),
                result: result.clone(),
            },
        )
        .await?;
        persist_tool_result(state, prepared, messages, &call.id, &result).await?;
    }
    Ok(())
}

fn tool_calls_with_permission_mode(
    tool_calls: &[ToolCall],
    permission_mode: &TerminalPermissionMode,
) -> Value {
    let mut value = serde_json::to_value(tool_calls).unwrap_or(Value::Array(Vec::new()));
    if let Value::Array(calls) = &mut value {
        for call in calls {
            if let Value::Object(call) = call {
                call.insert(
                    "permission_mode".to_string(),
                    serde_json::to_value(permission_mode).unwrap_or(Value::Null),
                );
            }
        }
    }
    value
}

struct IterationOutput {
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    output: ChatCompletionOutput,
}

async fn run_llm_iteration(
    runtime: &AgentRuntime<'_>,
    messages: &[ChatMessage],
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<IterationOutput> {
    if runtime.use_streaming_llm {
        stream_llm_iteration(runtime, messages, tools).await
    } else {
        let completion = runtime.client.complete(
            &runtime.prepared.agent,
            &runtime.prepared.provider,
            messages,
            &runtime.prepared.system_prompt,
            runtime.options.clone(),
            tools,
        );
        let output = if let Some(sender) = runtime.events {
            tokio::select! {
                result = completion => result?,
                _ = sender.closed() => {
                    return Err(AppError::Llm("stream client disconnected".to_string()));
                }
            }
        } else {
            completion.await?
        };
        Ok(IterationOutput {
            message_id: Uuid::new_v4(),
            created_at: Utc::now(),
            output,
        })
    }
}

async fn stream_llm_iteration(
    runtime: &AgentRuntime<'_>,
    messages: &[ChatMessage],
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<IterationOutput> {
    let message_id = Uuid::new_v4();
    let created_at = Utc::now();
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut persisted = false;
    let mut persisted_bytes = 0_usize;
    let mut last_save = Instant::now();
    let mut calls: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();
    let mut finished_with_tool_calls = None;
    let mut usage = TokenUsage::default();
    let mut stream = runtime
        .client
        .stream(
            &runtime.prepared.agent,
            &runtime.prepared.provider,
            messages,
            &runtime.prepared.system_prompt,
            runtime.options.clone(),
            tools,
        )
        .await?;

    loop {
        let next = if let Some(sender) = runtime.events {
            tokio::select! {
                event = stream.next() => event,
                _ = sender.closed() => {
                    mark_interrupted(
                        runtime.state,
                        runtime.prepared,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                        persisted,
                    )
                        .await;
                    return Err(AppError::Llm("stream client disconnected".to_string()));
                }
            }
        } else {
            stream.next().await
        };
        let Some(event) = next else {
            break;
        };
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                return Err(fail_stream_iteration(
                    runtime,
                    message_id,
                    created_at,
                    &content,
                    &reasoning_content,
                    persisted,
                    error,
                )
                .await);
            }
        };
        match event {
            StreamEvent::TextDelta(delta) => {
                if content.len().saturating_add(delta.len()) > MAX_STREAM_CONTENT_BYTES {
                    return Err(fail_stream_iteration(
                        runtime,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                        persisted,
                        AppError::Llm(format!(
                            "streamed response exceeded {MAX_STREAM_CONTENT_BYTES} bytes"
                        )),
                    )
                    .await);
                }
                content.push_str(&delta);
                if emit(runtime.events, AgentRunEvent::Delta(delta))
                    .await
                    .is_err()
                {
                    mark_interrupted(
                        runtime.state,
                        runtime.prepared,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                        persisted,
                    )
                    .await;
                    return Err(AppError::Llm("stream client disconnected".to_string()));
                }
                let streamed_bytes = content.len().saturating_add(reasoning_content.len());
                if !persisted
                    || streamed_bytes.saturating_sub(persisted_bytes) >= STREAM_SAVE_BYTES
                    || last_save.elapsed() >= STREAM_SAVE_INTERVAL
                {
                    let message = partial_assistant(
                        runtime.prepared,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                    );
                    if let Err(error) = runtime
                        .state
                        .store
                        .upsert_conversation_message(message.conversation_id, message)
                        .await
                    {
                        return Err(fail_stream_iteration(
                            runtime,
                            message_id,
                            created_at,
                            &content,
                            &reasoning_content,
                            persisted,
                            error,
                        )
                        .await);
                    }
                    persisted = true;
                    persisted_bytes = streamed_bytes;
                    last_save = Instant::now();
                }
            }
            StreamEvent::ReasoningDelta(delta) => {
                if reasoning_content.len().saturating_add(delta.len()) > MAX_STREAM_CONTENT_BYTES {
                    return Err(fail_stream_iteration(
                        runtime,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                        persisted,
                        AppError::Llm(format!(
                            "streamed reasoning exceeded {MAX_STREAM_CONTENT_BYTES} bytes"
                        )),
                    )
                    .await);
                }
                reasoning_content.push_str(&delta);
                if emit(runtime.events, AgentRunEvent::ReasoningDelta(delta))
                    .await
                    .is_err()
                {
                    mark_interrupted(
                        runtime.state,
                        runtime.prepared,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                        persisted,
                    )
                    .await;
                    return Err(AppError::Llm("stream client disconnected".to_string()));
                }
                let streamed_bytes = content.len().saturating_add(reasoning_content.len());
                if !persisted
                    || streamed_bytes.saturating_sub(persisted_bytes) >= STREAM_SAVE_BYTES
                    || last_save.elapsed() >= STREAM_SAVE_INTERVAL
                {
                    let message = partial_assistant(
                        runtime.prepared,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                    );
                    if let Err(error) = runtime
                        .state
                        .store
                        .upsert_conversation_message(message.conversation_id, message)
                        .await
                    {
                        return Err(fail_stream_iteration(
                            runtime,
                            message_id,
                            created_at,
                            &content,
                            &reasoning_content,
                            persisted,
                            error,
                        )
                        .await);
                    }
                    persisted = true;
                    persisted_bytes = streamed_bytes;
                    last_save = Instant::now();
                }
            }
            StreamEvent::ToolCallStart { index, id, name } => {
                let builder = calls.entry(index).or_default();
                if !id.is_empty() {
                    builder.id = id;
                }
                if !name.is_empty() {
                    builder.name = name;
                }
            }
            StreamEvent::ToolCallArgsDelta { index, arguments } => {
                let builder = calls.entry(index).or_default();
                if builder.arguments.len().saturating_add(arguments.len()) > MAX_TOOL_ARGUMENT_BYTES
                {
                    return Err(fail_stream_iteration(
                        runtime,
                        message_id,
                        created_at,
                        &content,
                        &reasoning_content,
                        persisted,
                        AppError::Llm(format!(
                            "tool arguments exceeded {MAX_TOOL_ARGUMENT_BYTES} bytes"
                        )),
                    )
                    .await);
                }
                builder.arguments.push_str(&arguments);
            }
            StreamEvent::Usage(value) => {
                usage = value;
            }
            StreamEvent::FinishedWithToolCalls => {
                finished_with_tool_calls = Some(true);
            }
            StreamEvent::Finished => {
                finished_with_tool_calls.get_or_insert(false);
            }
        }
    }

    let Some(finished_with_tool_calls) = finished_with_tool_calls else {
        mark_interrupted(
            runtime.state,
            runtime.prepared,
            message_id,
            created_at,
            &content,
            &reasoning_content,
            persisted,
        )
        .await;
        return Err(AppError::Llm(
            "LLM stream ended before a completion event".to_string(),
        ));
    };
    let tool_calls = calls
        .into_values()
        .map(|builder| {
            if builder.id.is_empty() || builder.name.is_empty() {
                return Err(AppError::Llm(
                    "LLM stream ended with an incomplete tool call".to_string(),
                ));
            }
            Ok(ToolCall {
                id: builder.id,
                kind: "function".to_string(),
                function: ToolCallFunction {
                    name: builder.name,
                    arguments: builder.arguments,
                },
            })
        })
        .collect::<AppResult<Vec<_>>>();
    let tool_calls = match tool_calls {
        Ok(tool_calls) => tool_calls,
        Err(error) => {
            return Err(fail_stream_iteration(
                runtime,
                message_id,
                created_at,
                &content,
                &reasoning_content,
                persisted,
                error,
            )
            .await);
        }
    };
    if finished_with_tool_calls && tool_calls.is_empty() {
        return Err(fail_stream_iteration(
            runtime,
            message_id,
            created_at,
            &content,
            &reasoning_content,
            persisted,
            AppError::Llm(
                "LLM reported tool calls but did not provide a complete tool call".to_string(),
            ),
        )
        .await);
    }

    Ok(IterationOutput {
        message_id,
        created_at,
        output: ChatCompletionOutput {
            content,
            reasoning_content,
            tool_calls,
            usage,
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
    events: &Option<mpsc::Sender<AgentRunEvent>>,
    iteration: usize,
) -> ToolResult {
    let Some(run_sender) = events.clone() else {
        return state.tool_registry.execute(name, arguments, context).await;
    };
    let (tool_sender, mut tool_receiver) = mpsc::channel(16);
    let terminal_manager = context.terminal_manager.clone();
    let conversation_id = context.conversation_id;
    context.event_sender = Some(tool_sender);
    let execution = state.tool_registry.execute(name, arguments, context);
    tokio::pin!(execution);
    loop {
        tokio::select! {
            result = &mut execution => return result,
            _ = run_sender.closed() => {
                terminal_manager.stop_for_conversation(conversation_id).await;
                return ToolResult::Rejected {
                    reason: "Approval client disconnected".to_string(),
                };
            }
            event = tool_receiver.recv() => {
                let Some(event) = event else { continue; };
                let mapped = match event {
                    ToolExecutionEvent::ApprovalRequired {
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
                        if run_sender
                            .send(AgentRunEvent::Phase {
                                phase: AgentRunPhase::WaitingForApproval,
                                iteration,
                                call_id: Some(call_id.clone()),
                            })
                            .await
                            .is_err()
                        {
                            return ToolResult::Rejected {
                                reason: "Approval client disconnected".to_string(),
                            };
                        }
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
                        }
                    }
                };
                if run_sender.send(mapped).await.is_err() {
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
        reasoning_content: None,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: Some(call_id.to_string()),
        attachments: Vec::new(),
        usage: None,
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
    reasoning_content: &str,
) -> ChatMessage {
    ChatMessage {
        id: message_id,
        agent_id: prepared.agent.id,
        conversation_id: prepared.user_message.conversation_id,
        role: ChatRole::Assistant,
        content: content.to_string(),
        reasoning_content: (!reasoning_content.is_empty()).then(|| reasoning_content.to_string()),
        status: ChatMessageStatus::Streaming,
        created_at,
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
    }
}

fn model_usage(prepared: &PreparedChat, tokens: &TokenUsage) -> ModelUsage {
    let tokens = tokens.clone().normalized();
    let config = prepared
        .provider
        .model_configs
        .iter()
        .find(|config| config.model_id == prepared.agent.model_id);
    // Missing unit prices are treated as 0 so partial configs still bill what is known.
    let cost_usd = Some(match config {
        Some(config) => {
            tokens.input_tokens as f64 * config.input_price.unwrap_or(0.0) / 1_000_000.0
                + tokens.output_tokens as f64 * config.output_price.unwrap_or(0.0) / 1_000_000.0
                + tokens.cache_write_tokens as f64 * config.cache_write_price.unwrap_or(0.0) / 1_000_000.0
                + tokens.cache_read_tokens as f64 * config.cache_read_price.unwrap_or(0.0) / 1_000_000.0
        }
        None => 0.0,
    });
    let total_tokens = tokens.total_tokens();
    ModelUsage {
        tokens,
        total_tokens,
        cost_usd,
        provider_id: prepared.provider.id,
        model_id: prepared.agent.model_id.clone(),
    }
}

async fn mark_interrupted(
    state: &AppState,
    prepared: &PreparedChat,
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    content: &str,
    reasoning_content: &str,
    persisted: bool,
) {
    if !persisted && content.is_empty() && reasoning_content.is_empty() {
        return;
    }
    let mut message = partial_assistant(
        prepared,
        message_id,
        created_at,
        content,
        reasoning_content,
    );
    message.status = ChatMessageStatus::Failed;
    let _ = state
        .store
        .upsert_conversation_message(message.conversation_id, message)
        .await;
}

async fn fail_stream_iteration(
    runtime: &AgentRuntime<'_>,
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    content: &str,
    reasoning_content: &str,
    persisted: bool,
    error: AppError,
) -> AppError {
    mark_interrupted(
        runtime.state,
        runtime.prepared,
        message_id,
        created_at,
        content,
        reasoning_content,
        persisted,
    )
    .await;
    error
}

async fn emit(events: &Option<mpsc::Sender<AgentRunEvent>>, event: AgentRunEvent) -> AppResult<()> {
    if let Some(events) = events {
        events
            .send(event)
            .await
            .map_err(|_| AppError::Llm("stream client disconnected".to_string()))?;
    }
    Ok(())
}


