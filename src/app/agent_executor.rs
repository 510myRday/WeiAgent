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
        ChatMessage, ChatMessageStatus, ChatRole, ModelUsage, TaskPlan, TaskPlanItemStatus,
        TerminalPermissionMode, TokenUsage, ToolResult,
    },
    error::{AppError, AppResult},
    llm::{
        client::{DynChatClient, RequestOptions},
        types::{ChatCompletionOutput, StreamEvent, ToolCall, ToolCallFunction, ToolDefinition},
    },
    tools::registry::{ToolExecutionContext, ToolExecutionEvent},
    tools::task_planning::TASK_PLANNING_TOOL_NAME,
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
    CompactingContext,
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
    ToolCallStarted {
        call_id: String,
        name: String,
        permission_mode: TerminalPermissionMode,
    },
    ToolCallArgsDelta {
        call_id: String,
        arguments: String,
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
    prepared: &'a mut PreparedChat,
    options: RequestOptions,
    use_streaming_llm: bool,
    events: &'a Option<mpsc::Sender<AgentRunEvent>>,
}

#[derive(Default)]
struct ToolBudget {
    seen_call_ids: HashSet<String>,
    total_calls: usize,
    task_plan_tool_iterations: usize,
    web_search_provenance: crate::tools::web_search::evidence::WebSearchProvenanceCache,
}

impl ToolBudget {
    fn task_plan_progress_due(&mut self, has_active_plan: bool, interval: usize) -> bool {
        if !has_active_plan {
            self.task_plan_tool_iterations = 0;
            return false;
        }
        self.task_plan_tool_iterations += 1;
        self.task_plan_tool_iterations >= interval
    }

    fn reset_task_plan_progress(&mut self) {
        self.task_plan_tool_iterations = 0;
    }
}

/// Per-run compaction inputs, loaded once instead of re-reading settings and
/// the checkpoint from storage on every LLM iteration.
struct CompactionState {
    config: crate::app::context_compress::CompactionConfig,
    checkpoint: Option<crate::domain::ContextCheckpoint>,
    fixed_tokens_with_tools: u64,
    fixed_tokens_without_tools: u64,
}

async fn ensure_messages_within_context(
    runtime: &AgentRuntime<'_>,
    messages: &mut Vec<ChatMessage>,
    tools: Option<&[ToolDefinition]>,
    active_task_plan: Option<&crate::app::context_compress::ActiveTaskPlanContext>,
    compaction: &mut CompactionState,
) -> AppResult<()> {
    crate::app::context_compress::refresh_active_task_plan_anchor(messages, active_task_plan);
    let fixed_tokens = if tools.is_some() {
        compaction.fixed_tokens_with_tools
    } else {
        compaction.fixed_tokens_without_tools
    };
    if !crate::app::context_compress::needs_compaction_with_fixed(
        messages,
        &compaction.config,
        fixed_tokens,
    ) {
        return Ok(());
    }

    emit(
        runtime.events,
        AgentRunEvent::Phase {
            phase: AgentRunPhase::CompactingContext,
            iteration: 0,
            call_id: None,
        },
    )
    .await?;

    let outcome = crate::app::context_compress::ensure_within_threshold(
        runtime.client,
        &runtime.prepared.agent,
        &runtime.prepared.provider,
        &compaction.config,
        &runtime.prepared.system_prompt,
        &runtime.options,
        tools,
        active_task_plan,
        messages.clone(),
        compaction.checkpoint.clone(),
        runtime.events.as_ref(),
    )
    .await?;

    if outcome.did_compact {
        tracing::info!(
            before = outcome.estimated_tokens_before,
            after = outcome.estimated_tokens_after,
            reason = ?outcome.reason,
            "auto-compacted conversation context"
        );
        *messages = outcome.messages;
        if let Some(checkpoint) = outcome.checkpoint {
            runtime
                .state
                .store
                .write_context_checkpoint(
                    runtime.prepared.user_message.conversation_id,
                    checkpoint.clone(),
                )
                .await?;
            compaction.checkpoint = Some(checkpoint);
        }
    }
    Ok(())
}

pub async fn run_agent(
    state: AppState,
    mut prepared: PreparedChat,
    options: RequestOptions,
    use_streaming_llm: bool,
    events: Option<mpsc::Sender<AgentRunEvent>>,
) -> AppResult<()> {
    let client = state.llm_registry.client_for(&prepared.provider)?;
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
    // Tool definitions were computed by prepare_chat and stay frozen for this
    // run so prompt-cache prefixes remain stable across tool iterations.
    // Execution still hot-reloads terminal config per tool call.
    let frozen_tools = state.tool_registry.definitions(&prepared.tools_config);
    let max_llm_iterations = prepared.tools_config.max_llm_iterations();

    // Load compaction inputs once per run; the checkpoint is kept in memory and
    // written back only when compaction produces a new one.
    let settings = state.store.read_settings().await?;
    let checkpoint = state
        .store
        .read_context_checkpoint(user_message.conversation_id)
        .await?;
    let mut compaction = CompactionState {
        config: crate::app::context_compress::CompactionConfig::from_agent_provider_and_threshold(
            &prepared.agent,
            &prepared.provider,
            settings.context_compress_threshold_percent,
            options.thinking_level,
        ),
        checkpoint,
        fixed_tokens_with_tools: crate::app::context_compress::fixed_request_tokens(
            &prepared.system_prompt,
            frozen_tools.as_deref(),
        ),
        fixed_tokens_without_tools: crate::app::context_compress::fixed_request_tokens(
            &prepared.system_prompt,
            None,
        ),
    };

    let runtime = AgentRuntime {
        state: &state,
        client: &client,
        prepared: &mut prepared,
        options,
        use_streaming_llm,
        events: &events,
    };

    let mut iteration = 0_usize;
    loop {
        if iteration >= max_llm_iterations {
            break;
        }
        let active_task_plan = if runtime.prepared.tools_config.task_planning.enabled {
            runtime
                .state
                .store
                .read_task_plan(user_message.conversation_id)
                .await?
        } else {
            None
        };
        let active_task_plan_context = active_task_plan
            .as_ref()
            .map(crate::app::context_compress::ActiveTaskPlanContext::from_task_plan);
        let has_active_task_plan = active_task_plan_context.is_some();
        let tools = frozen_tools.clone();
        ensure_messages_within_context(
            &runtime,
            &mut messages,
            tools.as_deref(),
            active_task_plan_context.as_ref(),
            &mut compaction,
        )
        .await?;
        emit(
            &events,
            AgentRunEvent::Phase {
                phase: AgentRunPhase::WaitingForModel,
                iteration,
                call_id: None,
            },
        )
        .await?;
        let output = match run_llm_iteration(&runtime, &messages, tools).await {
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
            agent_id: runtime.prepared.agent.id,
            conversation_id: user_message.conversation_id,
            role: ChatRole::Assistant,
            content: output.output.content.clone(),
            reasoning_content: (!output.output.reasoning_content.is_empty())
                .then_some(output.output.reasoning_content.clone()),
            openai_response_items: output.output.openai_response_items.clone(),
            status: ChatMessageStatus::Done,
            created_at: output.created_at,
            tool_calls: (!output.output.tool_calls.is_empty()).then(|| {
                tool_calls_with_permission_mode(
                    &output.output.tool_calls,
                    &runtime.prepared.tools_config.terminal.permission_mode,
                )
            }),
            tool_call_id: None,
            attachments: Vec::new(),
            usage: Some(model_usage(runtime.prepared, &output.output.usage)),
        };
        state
            .store
            .upsert_conversation_message(user_message.conversation_id, assistant_message.clone())
            .await?;

        if output.output.tool_calls.is_empty() {
            emit(&events, AgentRunEvent::Assistant(assistant_message.clone())).await?;
            emit(&events, AgentRunEvent::Done).await?;
            return Ok(());
        }

        if !assistant_message.content.is_empty() || assistant_message.reasoning_content.is_some() {
            emit(&events, AgentRunEvent::Assistant(assistant_message.clone())).await?;
        }
        messages.push(assistant_message);
        let append_task_plan_progress = tool_budget.task_plan_progress_due(
            has_active_task_plan,
            runtime
                .prepared
                .tools_config
                .task_planning
                .progress_interval
                .clamp(1, crate::domain::tools::MAX_TASK_PLAN_PROGRESS_INTERVAL)
                as usize,
        );
        if let Err(error) = execute_tool_calls(
            runtime.state,
            runtime.prepared,
            &mut messages,
            output.output.tool_calls,
            &mut tool_budget,
            runtime.events,
            iteration,
            append_task_plan_progress,
        )
        .await
        {
            persist_run_failure(&runtime, &error).await;
            return Err(error);
        }
        if append_task_plan_progress {
            tool_budget.reset_task_plan_progress();
        }

        iteration += 1;
        // Final reserved turn after the last tool-capable iteration.
        if iteration >= max_llm_iterations {
            break;
        }
    }

    let summary =
        match finalize_after_budget(&runtime, &mut messages, max_llm_iterations, &mut compaction)
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                persist_run_failure(&runtime, &error).await;
                return Err(error);
            }
        };
    emit(&events, AgentRunEvent::Assistant(summary.clone())).await?;
    emit(&events, AgentRunEvent::Done).await?;
    Ok(())
}

async fn persist_run_failure(runtime: &AgentRuntime<'_>, error: &AppError) {
    let _ = runtime
        .state
        .store
        .repair_incomplete_tool_calls(runtime.prepared.user_message.conversation_id)
        .await;
    let assistant_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: runtime.prepared.agent.id,
        conversation_id: runtime.prepared.user_message.conversation_id,
        role: ChatRole::Assistant,
        content: format!("Agent run stopped: {error}"),
        reasoning_content: None,
        openai_response_items: Vec::new(),
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
    compaction: &mut CompactionState,
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
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
    };
    // Keep the budget notice in the in-memory LLM context only.
    messages.push(budget_notice);

    let active_task_plan = if runtime.prepared.tools_config.task_planning.enabled {
        runtime
            .state
            .store
            .read_task_plan(runtime.prepared.user_message.conversation_id)
            .await?
    } else {
        None
    };
    let active_task_plan_context = active_task_plan
        .as_ref()
        .map(crate::app::context_compress::ActiveTaskPlanContext::from_task_plan);
    ensure_messages_within_context(
        runtime,
        messages,
        None,
        active_task_plan_context.as_ref(),
        compaction,
    )
    .await?;
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
        openai_response_items: output.output.openai_response_items,
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

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls(
    state: &AppState,
    prepared: &mut PreparedChat,
    messages: &mut Vec<ChatMessage>,
    tool_calls: Vec<ToolCall>,
    budget: &mut ToolBudget,
    events: &Option<mpsc::Sender<AgentRunEvent>>,
    iteration: usize,
    append_task_plan_progress: bool,
) -> AppResult<()> {
    let call_count = tool_calls.len();
    for (index, call) in tool_calls.into_iter().enumerate() {
        refresh_live_terminal_config(state, prepared).await?;
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
        let result = if let Some(message) = validation_error {
            ToolResult::Error { message }
        } else {
            match parsed_arguments {
                Ok(Value::Object(map)) => {
                    let context = ToolExecutionContext {
                        agent_id: prepared.agent.id,
                        conversation_id: prepared.user_message.conversation_id,
                        call_id: call.id.clone(),
                        tools_config: prepared.tools_config.clone(),
                        store: state.store.clone(),
                        terminal_manager: state.terminal_manager.clone(),
                        approval_manager: state.approval_manager.clone(),
                        event_sender: None,
                        web_search_provenance: budget.web_search_provenance.clone(),
                    };
                    execute_tool_with_events(
                        state,
                        &call.function.name,
                        Value::Object(map),
                        context,
                        events,
                        iteration,
                    )
                    .await
                }
                Ok(other) => ToolResult::Error {
                    message: format!("Invalid tool arguments: expected a JSON object, got {other}"),
                },
                Err(error) => ToolResult::Error {
                    message: format!("Invalid tool arguments: {error}"),
                },
            }
        };
        let result = if append_task_plan_progress && index + 1 == call_count {
            append_task_plan_progress_to_result(
                state,
                prepared.user_message.conversation_id,
                result,
            )
            .await?
        } else {
            result
        };
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

async fn append_task_plan_progress_to_result(
    state: &AppState,
    conversation_id: Uuid,
    mut result: ToolResult,
) -> AppResult<ToolResult> {
    let plan = match state.store.read_task_plan(conversation_id).await? {
        Some(plan) => Some(plan),
        None => state
            .store
            .read_task_plans(conversation_id)
            .await?
            .into_iter()
            .next(),
    };
    let Some(plan) = plan else {
        return Ok(result);
    };
    let notice = format_task_plan_progress(&plan);
    match &mut result {
        ToolResult::Completed { stdout, .. } => append_progress_to_stdout(stdout, &notice),
        ToolResult::Running { message, .. } => message.push_str(&notice),
        ToolResult::Blocked { reason, .. } => reason.push_str(&notice),
        ToolResult::Rejected { reason } => reason.push_str(&notice),
        ToolResult::Interrupted { reason } => reason.push_str(&notice),
        ToolResult::Error { message } => message.push_str(&notice),
        ToolResult::PendingApproval { .. } => {}
    }
    Ok(result)
}

fn append_progress_to_stdout(stdout: &mut String, notice: &str) {
    if let Ok(Value::Object(mut object)) = serde_json::from_str::<Value>(stdout.as_str()) {
        object.insert(
            "task_plan_progress".to_string(),
            Value::String(notice.trim().to_string()),
        );
        *stdout = Value::Object(object).to_string();
    } else {
        stdout.push_str(notice);
    }
}

fn format_task_plan_progress(plan: &TaskPlan) -> String {
    let current = plan
        .plan
        .iter()
        .find(|item| item.status == TaskPlanItemStatus::InProgress)
        .map(|item| item.step.as_str())
        .unwrap_or("无");
    let steps = plan
        .plan
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let item_status = match &item.status {
                TaskPlanItemStatus::Pending => "待处理",
                TaskPlanItemStatus::InProgress => "进行中",
                TaskPlanItemStatus::Completed => "已完成",
            };
            format!("{}. [{}] {}", index + 1, item_status, item.step)
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "\n\n【任务规划进度提醒】\n当前进度：\n{steps}\n当前步骤：{current}\n请只判断当前步骤是否完成：如果当前步骤已完成，请立即调用 task_planning 将当前步骤更新为 completed；如果当前步骤尚未完成，请继续执行当前步骤，不要开始其他计划步骤。"
    )
}

async fn refresh_live_terminal_config(
    state: &AppState,
    prepared: &mut PreparedChat,
) -> AppResult<()> {
    let live = crate::app::chat::effective_tools_config(state, prepared.agent.id).await?;
    prepared.tools_config.terminal = live.terminal;
    Ok(())
}

const MAX_TOOL_RESULT_STDOUT_CHARS: usize = 24_000;
const MAX_TOOL_RESULT_STDERR_CHARS: usize = 4_000;
const MAX_WEB_SEARCH_SNIPPET_CHARS: usize = 600;
const MAX_WEB_SEARCH_CONTENT_EXCERPT_CHARS: usize = 2_000;
const MAX_WEB_SEARCH_RESULTS_FOR_LLM: usize = 15;
const MAX_WEB_EXTRACT_EXCERPT_CHARS: usize = 8_000;

fn sanitize_tool_result_for_llm(result: &ToolResult) -> Value {
    match result {
        ToolResult::Completed {
            stdout,
            stderr,
            exit_code,
            runtime,
            output_cursor,
            ..
        } => {
            let mut map = serde_json::Map::new();
            map.insert("status".to_string(), Value::String("completed".to_string()));
            map.insert(
                "stdout".to_string(),
                Value::String(compact_tool_stdout(stdout)),
            );
            let stderr = trim_chars(stderr, MAX_TOOL_RESULT_STDERR_CHARS);
            if !stderr.is_empty() {
                map.insert("stderr".to_string(), Value::String(stderr));
            }
            map.insert("exit_code".to_string(), json_i64(*exit_code as i64));
            if let Some(runtime) = runtime {
                map.insert("runtime".to_string(), Value::String(runtime.clone()));
            }
            if *output_cursor > 0 {
                map.insert("output_cursor".to_string(), json_u64(*output_cursor));
            }
            Value::Object(map)
        }
        ToolResult::Running {
            process_id,
            message,
            stdout,
            stderr,
            runtime,
            output_cursor,
            ..
        } => {
            let mut map = serde_json::Map::new();
            map.insert("status".to_string(), Value::String("running".to_string()));
            map.insert("process_id".to_string(), Value::String(process_id.clone()));
            map.insert("message".to_string(), Value::String(message.clone()));
            let stdout = compact_tool_stdout(stdout);
            if !stdout.is_empty() {
                map.insert("stdout".to_string(), Value::String(stdout));
            }
            let stderr = trim_chars(stderr, MAX_TOOL_RESULT_STDERR_CHARS);
            if !stderr.is_empty() {
                map.insert("stderr".to_string(), Value::String(stderr));
            }
            if let Some(runtime) = runtime {
                map.insert("runtime".to_string(), Value::String(runtime.clone()));
            }
            if *output_cursor > 0 {
                map.insert("output_cursor".to_string(), json_u64(*output_cursor));
            }
            Value::Object(map)
        }
        other => serde_json::to_value(other).unwrap_or(Value::Null),
    }
}

fn compact_tool_stdout(stdout: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(stdout) {
        if compact_web_search_value(&mut value) {
            return value.to_string();
        }
        if compact_web_extract_value(&mut value) {
            return trim_chars(&value.to_string(), 48_000);
        }
    }
    trim_chars(stdout, MAX_TOOL_RESULT_STDOUT_CHARS)
}

fn compact_web_search_value(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let is_search = obj.get("queries").is_some()
        && obj.get("source_runs").is_some()
        && obj.get("results").is_some();
    if !is_search {
        return false;
    }
    if let Some(results) = obj.get_mut("results").and_then(Value::as_array_mut) {
        if results.len() > MAX_WEB_SEARCH_RESULTS_FOR_LLM {
            results.truncate(MAX_WEB_SEARCH_RESULTS_FOR_LLM);
        }
        for hit in results.iter_mut() {
            if let Some(hit_obj) = hit.as_object_mut() {
                hit_obj.remove("score");
                if let Some(snippet) = hit_obj.get("snippet").and_then(Value::as_str) {
                    let trimmed = trim_chars(snippet, MAX_WEB_SEARCH_SNIPPET_CHARS);
                    hit_obj.insert("snippet".to_string(), Value::String(trimmed));
                }
                if let Some(content) = hit_obj.get("content_excerpt").and_then(Value::as_str) {
                    let trimmed = trim_chars(content, MAX_WEB_SEARCH_CONTENT_EXCERPT_CHARS);
                    hit_obj.insert("content_excerpt".to_string(), Value::String(trimmed));
                }
            }
        }
    }
    if let Some(syntheses) = obj.get_mut("ai_syntheses").and_then(Value::as_array_mut) {
        syntheses.truncate(6);
        for synthesis in syntheses {
            if let Some(answer) = synthesis.get("answer").and_then(Value::as_str) {
                let answer = trim_chars(answer, 1_200);
                if let Some(map) = synthesis.as_object_mut() {
                    map.insert("answer".to_string(), Value::String(answer));
                }
            }
        }
    }
    if let Some(results) = obj.get("results").and_then(Value::as_array) {
        let count = results.len() as u64;
        obj.insert("result_count".to_string(), json_u64(count));
    }
    true
}

fn compact_web_extract_value(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    if obj.get("citation_instruction").is_none() {
        return false;
    }
    let Some(documents) = obj.get_mut("documents").and_then(Value::as_array_mut) else {
        return false;
    };
    for document in documents {
        let Some(excerpts) = document
            .get_mut("relevant_excerpts")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let mut remaining = MAX_WEB_EXTRACT_EXCERPT_CHARS;
        for excerpt in excerpts.iter_mut() {
            if remaining == 0 {
                *excerpt = Value::String(String::new());
                continue;
            }
            let Some(text) = excerpt.as_str() else {
                continue;
            };
            let trimmed = trim_chars(text, remaining.min(2_000));
            remaining = remaining.saturating_sub(trimmed.chars().count());
            *excerpt = Value::String(trimmed);
        }
        excerpts.retain(|excerpt| excerpt.as_str().is_some_and(|text| !text.is_empty()));
    }
    true
}

fn trim_chars(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    let keep = max_chars.saturating_sub(32);
    let mut out: String = input.chars().take(keep).collect();
    out.push_str(&format!(
        "\n...[truncated {omitted} chars]",
        omitted = count - keep
    ));
    out
}

fn json_i64(value: i64) -> Value {
    Value::Number(value.into())
}

fn json_u64(value: u64) -> Value {
    Value::Number(value.into())
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

/// Mutable state of one streamed assistant message.
struct StreamAccumulator {
    message_id: Uuid,
    created_at: chrono::DateTime<Utc>,
    content: String,
    reasoning_content: String,
    openai_response_items: Vec<Value>,
    persisted: bool,
    persisted_bytes: usize,
    last_save: Instant,
}

#[derive(Clone, Copy)]
enum StreamDeltaKind {
    Text,
    Reasoning,
}

/// Persist the partial message as failed, then hand back the error.
async fn fail_stream(
    runtime: &AgentRuntime<'_>,
    acc: &StreamAccumulator,
    error: AppError,
) -> AppError {
    mark_interrupted(
        runtime.state,
        runtime.prepared,
        acc.message_id,
        acc.created_at,
        &acc.content,
        &acc.reasoning_content,
        acc.persisted,
    )
    .await;
    error
}

async fn append_delta_and_persist(
    runtime: &AgentRuntime<'_>,
    acc: &mut StreamAccumulator,
    delta: String,
    kind: StreamDeltaKind,
) -> AppResult<()> {
    let (buffer_len, overflow_label) = match kind {
        StreamDeltaKind::Text => (acc.content.len(), "response"),
        StreamDeltaKind::Reasoning => (acc.reasoning_content.len(), "reasoning"),
    };
    if buffer_len.saturating_add(delta.len()) > MAX_STREAM_CONTENT_BYTES {
        let error = AppError::Llm(format!(
            "streamed {overflow_label} exceeded {MAX_STREAM_CONTENT_BYTES} bytes"
        ));
        return Err(fail_stream(runtime, acc, error).await);
    }
    let event = match kind {
        StreamDeltaKind::Text => {
            acc.content.push_str(&delta);
            AgentRunEvent::Delta(delta)
        }
        StreamDeltaKind::Reasoning => {
            acc.reasoning_content.push_str(&delta);
            AgentRunEvent::ReasoningDelta(delta)
        }
    };
    if emit(runtime.events, event).await.is_err() {
        let error = AppError::Llm("stream client disconnected".to_string());
        return Err(fail_stream(runtime, acc, error).await);
    }
    let streamed_bytes = acc
        .content
        .len()
        .saturating_add(acc.reasoning_content.len());
    if !acc.persisted
        || streamed_bytes.saturating_sub(acc.persisted_bytes) >= STREAM_SAVE_BYTES
        || acc.last_save.elapsed() >= STREAM_SAVE_INTERVAL
    {
        let message = partial_assistant(
            runtime.prepared,
            acc.message_id,
            acc.created_at,
            &acc.content,
            &acc.reasoning_content,
        );
        if let Err(error) = runtime
            .state
            .store
            .upsert_conversation_message(message.conversation_id, message)
            .await
        {
            return Err(fail_stream(runtime, acc, error).await);
        }
        acc.persisted = true;
        acc.persisted_bytes = streamed_bytes;
        acc.last_save = Instant::now();
    }
    Ok(())
}

async fn stream_llm_iteration(
    runtime: &AgentRuntime<'_>,
    messages: &[ChatMessage],
    tools: Option<Vec<ToolDefinition>>,
) -> AppResult<IterationOutput> {
    let mut acc = StreamAccumulator {
        message_id: Uuid::new_v4(),
        created_at: Utc::now(),
        content: String::new(),
        reasoning_content: String::new(),
        openai_response_items: Vec::new(),
        persisted: false,
        persisted_bytes: 0,
        last_save: Instant::now(),
    };
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
                    let error = AppError::Llm("stream client disconnected".to_string());
                    return Err(fail_stream(runtime, &acc, error).await);
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
            Err(error) => return Err(fail_stream(runtime, &acc, error).await),
        };
        match event {
            StreamEvent::TextDelta(delta) => {
                append_delta_and_persist(runtime, &mut acc, delta, StreamDeltaKind::Text).await?;
            }
            StreamEvent::ReasoningDelta(delta) => {
                append_delta_and_persist(runtime, &mut acc, delta, StreamDeltaKind::Reasoning)
                    .await?;
            }
            StreamEvent::OpenAiResponseItem(item) => {
                if !acc
                    .openai_response_items
                    .iter()
                    .any(|existing| existing == &item)
                {
                    acc.openai_response_items.push(item);
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
                if !builder.started_emitted && !builder.id.is_empty() && !builder.name.is_empty() {
                    builder.started_emitted = true;
                    emit(
                        runtime.events,
                        AgentRunEvent::ToolCallStarted {
                            call_id: builder.id.clone(),
                            name: builder.name.clone(),
                            permission_mode: runtime
                                .prepared
                                .tools_config
                                .terminal
                                .permission_mode
                                .clone(),
                        },
                    )
                    .await?;
                }
            }
            StreamEvent::ToolCallArgsDelta { index, arguments } => {
                let builder = calls.entry(index).or_default();
                if builder.arguments.len().saturating_add(arguments.len()) > MAX_TOOL_ARGUMENT_BYTES
                {
                    let error = AppError::Llm(format!(
                        "tool arguments exceeded {MAX_TOOL_ARGUMENT_BYTES} bytes"
                    ));
                    return Err(fail_stream(runtime, &acc, error).await);
                }
                builder.arguments.push_str(&arguments);
                if !builder.id.is_empty() {
                    emit(
                        runtime.events,
                        AgentRunEvent::ToolCallArgsDelta {
                            call_id: builder.id.clone(),
                            arguments,
                        },
                    )
                    .await?;
                }
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
        let error = AppError::Llm("LLM stream ended before a completion event".to_string());
        return Err(fail_stream(runtime, &acc, error).await);
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
        Err(error) => return Err(fail_stream(runtime, &acc, error).await),
    };
    if finished_with_tool_calls && tool_calls.is_empty() {
        let error = AppError::Llm(
            "LLM reported tool calls but did not provide a complete tool call".to_string(),
        );
        return Err(fail_stream(runtime, &acc, error).await);
    }

    Ok(IterationOutput {
        message_id: acc.message_id,
        created_at: acc.created_at,
        output: ChatCompletionOutput {
            content: acc.content,
            reasoning_content: acc.reasoning_content,
            openai_response_items: acc.openai_response_items,
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
    started_emitted: bool,
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
    // Persist and re-send a compact tool payload: drop volatile fields like
    // duration_ms and trim oversized stdout/stderr so prompt-cache tails stay smaller.
    let content = serde_json::to_string(&sanitize_tool_result_for_llm(result))?;
    let tool_message = ChatMessage {
        id: Uuid::new_v4(),
        agent_id: prepared.agent.id,
        conversation_id: prepared.user_message.conversation_id,
        role: ChatRole::Tool,
        content,
        reasoning_content: None,
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: Some(call_id.to_string()),
        attachments: Vec::new(),
        openai_response_items: Vec::new(),
        usage: None,
    };
    state
        .store
        .append_conversation_messages(
            tool_message.conversation_id,
            std::slice::from_ref(&tool_message),
        )
        .await?;
    if let Some(planning_id) = task_plan_id_from_result(result) {
        state
            .store
            .associate_task_plan_with_message(
                tool_message.conversation_id,
                planning_id,
                tool_message.id,
            )
            .await?;
    }
    messages.push(tool_message);
    Ok(())
}

fn task_plan_id_from_result(result: &ToolResult) -> Option<Uuid> {
    let ToolResult::Completed { stdout, .. } = result else {
        return None;
    };
    let value = serde_json::from_str::<Value>(stdout).ok()?;
    if value.get("tool").and_then(Value::as_str) != Some(TASK_PLANNING_TOOL_NAME) {
        return None;
    }
    value
        .get("planning_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
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
        openai_response_items: Vec::new(),
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
                + tokens.cache_write_tokens as f64 * config.cache_write_price.unwrap_or(0.0)
                    / 1_000_000.0
                + tokens.cache_read_tokens as f64 * config.cache_read_price.unwrap_or(0.0)
                    / 1_000_000.0
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
    let mut message =
        partial_assistant(prepared, message_id, created_at, content, reasoning_content);
    message.status = ChatMessageStatus::Failed;
    let _ = state
        .store
        .upsert_conversation_message(message.conversation_id, message)
        .await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_completed_drops_duration_ms() {
        let result = ToolResult::Completed {
            stdout: "hello".into(),
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 1234,
            runtime: Some("builtin_web_search".into()),
            output_cursor: 0,
        };
        let value = sanitize_tool_result_for_llm(&result);
        assert_eq!(value["status"], "completed");
        assert_eq!(value["stdout"], "hello");
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["runtime"], "builtin_web_search");
        assert!(value.get("duration_ms").is_none());
    }

    #[test]
    fn sanitize_web_search_stdout_trims_snippets_and_scores() {
        let long = "x".repeat(800);
        let long_content = "y".repeat(2_400);
        let stdout = json!({
            "mode": "balanced",
            "queries": ["rust"],
            "result_count": 2,
            "source_runs": [],
            "results": [
                {"title": "A", "url": "https://a.test", "snippet": long, "content_excerpt": long_content, "score": 0.9},
                {"title": "B", "url": "https://b.test", "snippet": "short", "score": 0.1}
            ]
        })
        .to_string();
        let result = ToolResult::Completed {
            stdout,
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 50,
            runtime: Some("builtin_web_search".into()),
            output_cursor: 0,
        };
        let value = sanitize_tool_result_for_llm(&result);
        let compact: serde_json::Value =
            serde_json::from_str(value["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(compact["result_count"], 2);
        assert!(compact["results"][0].get("score").is_none());
        assert!(compact["results"][0]["snippet"].as_str().unwrap().len() < 800);
        assert!(
            compact["results"][0]["snippet"]
                .as_str()
                .unwrap()
                .contains("truncated")
        );
        assert!(
            compact["results"][0]["content_excerpt"]
                .as_str()
                .unwrap()
                .contains("truncated")
        );
    }

    #[test]
    fn trim_chars_keeps_short_text() {
        assert_eq!(trim_chars("abc", 10), "abc");
    }

    #[test]
    fn task_plan_progress_counter_starts_after_plan_creation_and_resets() {
        let mut budget = ToolBudget::default();
        assert!(!budget.task_plan_progress_due(false, 3));
        assert!(!budget.task_plan_progress_due(true, 3));
        assert!(!budget.task_plan_progress_due(true, 3));
        assert!(budget.task_plan_progress_due(true, 3));
        budget.reset_task_plan_progress();
        assert!(!budget.task_plan_progress_due(true, 3));
    }

    #[test]
    fn task_plan_progress_is_added_without_corrupting_json_stdout() {
        let mut stdout = json!({"tool": "example", "status": "completed"}).to_string();
        append_progress_to_stdout(&mut stdout, "\n\nprogress");
        let value: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["tool"], "example");
        assert_eq!(value["task_plan_progress"], "progress");
    }

    #[test]
    fn task_plan_progress_focuses_only_on_current_step() {
        let plan = TaskPlan {
            conversation_id: Uuid::new_v4(),
            planning_id: Uuid::new_v4(),
            conversation_message_id: None,
            explanation: None,
            plan: vec![
                crate::domain::TaskPlanItem {
                    step: "实现功能".to_string(),
                    status: TaskPlanItemStatus::InProgress,
                },
                crate::domain::TaskPlanItem {
                    step: "运行测试".to_string(),
                    status: TaskPlanItemStatus::Pending,
                },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            active: true,
        };
        let notice = format_task_plan_progress(&plan);
        assert!(notice.contains("当前步骤：实现功能"));
        assert!(notice.contains("请只判断当前步骤是否完成"));
        assert!(notice.contains("立即调用 task_planning 将当前步骤更新为 completed"));
        assert!(notice.contains("继续执行当前步骤，不要开始其他计划步骤"));
        assert!(!notice.contains("下一步："));
        assert!(!notice.contains("\n是否完成："));
    }
}
