//! Always-on context auto-compaction.
//!
//! UI transcript remains complete. Only the LLM-facing message list is rewritten
//! through a conversation-level [`ContextCheckpoint`] plus an optional recent tail.
//! Compaction is enforced before every model call (user turns and tool loops).

use chrono::Utc;
use uuid::Uuid;

use crate::{
    domain::{
        Agent, ChatMessage, ChatMessageStatus, ChatRole, ContextCheckpoint, ModelProvider,
        ProviderType, TaskPlan, TaskPlanItemStatus, ThinkingLevel,
        settings::coerce_context_compress_threshold,
    },
    error::{AppError, AppResult},
    llm::{
        client::{DynChatClient, RequestOptions},
        types::ToolDefinition,
    },
};

const MIN_OUTPUT_RESERVE_TOKENS: u64 = 512;
const STANDARD_OUTPUT_RESERVE_TOKENS: u64 = 4_096;
const THINKING_OUTPUT_RESERVE_TOKENS: u64 = 32_768;
const COMPACTION_MAX_OUTPUT_TOKENS: u64 = 2_048;
const IMAGE_TOKEN_ESTIMATE: u64 = 2_048;
/// Keep at least this many newest message groups after a summary rewrite.
const DEFAULT_RETAINED_TAIL_GROUPS: usize = 2;
/// Hard cap for text included in the summarizer input per message.
const MAX_SUMMARY_MESSAGE_CHARS: usize = 4_000;
/// Hard cap for tool payload text included in the summarizer input.
const MAX_SUMMARY_TOOL_CHARS: usize = 2_000;
/// Maximum summary characters kept in the checkpoint.
const MAX_SUMMARY_CHARS: usize = 12_000;
/// Keep the active plan compact while preserving it as a protected prefix.
const MAX_ACTIVE_TASK_PLAN_CHARS: usize = 8_000;
const SUMMARY_ANCHOR_PREFIX: &str = "[Conversation summary — auto-compacted]";
const ACTIVE_TASK_PLAN_PREFIX: &str = "[Active task plan]";

const COMPACTION_SYSTEM_PROMPT: &str = "\
You are a context compaction engine.
Do not call tools. Do not continue the user task. Produce a dense handoff summary only.

Summarize the conversation so a future model turn can continue the work without the older messages.

Use this exact structure:
1. User goal
2. Completed work
3. In progress
4. Remaining / TODO
5. Critical constraints, decisions, paths, IDs, configs, errors
6. Next concrete step

Rules:
- Prefer executable detail over narration.
- Keep absolute paths, commands, IDs, schemas, and unresolved errors.
- Drop repeated tool stdout noise, raw dumps, and redundant back-and-forth.
- If images or large tool outputs were present, note what mattered from them.
- Output plain text only.
";

const COMPACTION_USER_PROMPT: &str =
    "Compact the supplied conversation state using the required handoff structure.";

#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub context_size: u32,
    pub threshold_percent: u8,
    pub output_reserve_tokens: u64,
}

impl CompactionConfig {
    pub fn from_agent_provider_and_threshold(
        agent: &Agent,
        provider: &ModelProvider,
        threshold_percent: u8,
        thinking_level: Option<ThinkingLevel>,
    ) -> Self {
        let context_size = provider
            .model_configs
            .iter()
            .find(|config| config.model_id == agent.model_id)
            .map(|config| config.context_size)
            .filter(|size| *size > 0)
            .unwrap_or(128_000);
        let context_size_u64 = u64::from(context_size.max(1));
        let desired_output_reserve =
            if thinking_level.is_some_and(|level| level != ThinkingLevel::Off) {
                THINKING_OUTPUT_RESERVE_TOKENS
            } else {
                STANDARD_OUTPUT_RESERVE_TOKENS
            };
        let proportional_reserve = (context_size_u64 / 4).max(MIN_OUTPUT_RESERVE_TOKENS);
        let output_reserve_tokens = desired_output_reserve
            .min(proportional_reserve)
            .min(context_size_u64.saturating_sub(1));
        Self {
            context_size,
            threshold_percent: coerce_context_compress_threshold(threshold_percent),
            output_reserve_tokens,
        }
    }

    pub fn soft_limit_tokens(&self) -> u64 {
        let percent = u64::from(self.threshold_percent);
        let size = u64::from(self.context_size.max(1));
        // threshold is 1..=99 so this is always < context_size.
        let threshold_limit = ((size.saturating_mul(percent)) / 100).max(1);
        let output_safe_limit = size.saturating_sub(self.output_reserve_tokens).max(1);
        threshold_limit.min(output_safe_limit)
    }

    pub fn target_tokens_after_compact(&self) -> u64 {
        let soft = self.soft_limit_tokens();
        let sixty_percent = soft.saturating_mul(60) / 100;
        sixty_percent.min(soft.saturating_sub(1).max(1)).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactReason {
    WithinBudget,
    SemanticSummary,
    StructuralTrim,
}

#[derive(Debug, Clone)]
pub struct CompactOutcome {
    pub messages: Vec<ChatMessage>,
    pub checkpoint: Option<ContextCheckpoint>,
    pub reason: CompactReason,
    pub did_compact: bool,
    pub estimated_tokens_before: u64,
    pub estimated_tokens_after: u64,
}

/// Current task-plan state that must remain visible to the next model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTaskPlanContext {
    pub planning_id: Uuid,
    pub current_step: String,
    pub steps: Vec<String>,
}

impl ActiveTaskPlanContext {
    pub fn from_task_plan(plan: &TaskPlan) -> Self {
        let current_step = plan
            .plan
            .iter()
            .find(|item| item.status == TaskPlanItemStatus::InProgress)
            .map(|item| item.step.clone())
            .unwrap_or_else(|| "none".to_string());
        let steps = plan
            .plan
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let status = match item.status {
                    TaskPlanItemStatus::Pending => "pending",
                    TaskPlanItemStatus::InProgress => "in_progress",
                    TaskPlanItemStatus::Completed => "completed",
                };
                format!("{}. [{}] {}", index + 1, status, item.step)
            })
            .collect();
        Self {
            planning_id: plan.planning_id,
            current_step,
            steps,
        }
    }
}

/// Build the LLM-facing message list from full history + optional checkpoint.
pub fn build_llm_messages(
    history: &[ChatMessage],
    checkpoint: Option<&ContextCheckpoint>,
    active_task_plan: Option<&ActiveTaskPlanContext>,
) -> Vec<ChatMessage> {
    let Some(checkpoint) = checkpoint else {
        return history.to_vec();
    };
    if checkpoint.summary.trim().is_empty() {
        return history.to_vec();
    }

    let boundary = history
        .iter()
        .position(|message| message.id == checkpoint.covers_until_message_id);

    let (covered, tail) = match boundary {
        Some(index) => history.split_at(index.saturating_add(1)),
        None => {
            // Stale checkpoint id (history rewritten). Fall back to full history.
            return history.to_vec();
        }
    };

    if covered.is_empty() {
        return history.to_vec();
    }

    let summary_message = summary_anchor_message(covered, checkpoint, active_task_plan);
    let mut out = Vec::with_capacity(tail.len() + 1);
    out.push(summary_message);
    out.extend(tail.iter().cloned().map(clear_message_usage));
    out
}

fn summary_anchor_message(
    covered: &[ChatMessage],
    checkpoint: &ContextCheckpoint,
    active_task_plan: Option<&ActiveTaskPlanContext>,
) -> ChatMessage {
    let agent_id = covered
        .last()
        .map(|message| message.agent_id)
        .unwrap_or_else(Uuid::nil);
    let conversation_id = covered
        .last()
        .map(|message| message.conversation_id)
        .unwrap_or_else(Uuid::nil);
    let attachments = covered
        .iter()
        .flat_map(|message| message.attachments.iter().cloned())
        .collect();
    ChatMessage {
        id: checkpoint.covers_until_message_id,
        agent_id,
        conversation_id,
        role: ChatRole::User,
        content: render_summary_anchor_content(&checkpoint.summary, active_task_plan),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: checkpoint.created_at,
        tool_calls: None,
        tool_call_id: None,
        attachments,
        usage: None,
    }
}

fn render_summary_anchor_content(
    summary: &str,
    active_task_plan: Option<&ActiveTaskPlanContext>,
) -> String {
    let summary = summary
        .split(ACTIVE_TASK_PLAN_PREFIX)
        .next()
        .unwrap_or(summary)
        .trim_end();
    let Some(active_task_plan) = active_task_plan else {
        return format!("{SUMMARY_ANCHOR_PREFIX}\n{summary}");
    };
    let steps = active_task_plan.steps.join("\n");
    let plan = format!(
        "{ACTIVE_TASK_PLAN_PREFIX}\nplanning_id: {}\ncurrent_step: {}\nsteps:\n{}",
        active_task_plan.planning_id, active_task_plan.current_step, steps
    );
    let plan = truncate_chars(&plan, MAX_ACTIVE_TASK_PLAN_CHARS);
    format!("{SUMMARY_ANCHOR_PREFIX}\n{summary}\n\n{plan}")
}

/// Refresh the protected anchor after a plan is created or its status changes.
pub fn refresh_active_task_plan_anchor(
    messages: &mut [ChatMessage],
    active_task_plan: Option<&ActiveTaskPlanContext>,
) {
    let Some(anchor) = messages
        .iter_mut()
        .find(|message| message.content.starts_with(SUMMARY_ANCHOR_PREFIX))
    else {
        return;
    };
    anchor.content = render_summary_anchor_content(&anchor.content, active_task_plan);
}

fn estimate_messages_tokens(messages: &[ChatMessage]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum::<u64>()
}

fn estimate_text_tokens(value: &str) -> u64 {
    let mut ascii = 0_u64;
    let mut non_ascii = 0_u64;
    for character in value.chars() {
        if character.is_ascii() {
            ascii = ascii.saturating_add(1);
        } else {
            non_ascii = non_ascii.saturating_add(1);
        }
    }
    ascii.div_ceil(4).saturating_add(non_ascii)
}

fn estimate_message_tokens(message: &ChatMessage) -> u64 {
    let tool_call_tokens = message
        .tool_calls
        .as_ref()
        .map_or(0, |value| estimate_text_tokens(&value.to_string()));
    let response_item_tokens = message
        .openai_response_items
        .iter()
        .map(|item| estimate_text_tokens(&item.to_string()))
        .sum::<u64>();
    let attachment_tokens = message.attachments.iter().fold(0_u64, |total, attachment| {
        total.saturating_add(match &attachment.source {
            crate::domain::ImageSource::Base64 { .. } => IMAGE_TOKEN_ESTIMATE,
            crate::domain::ImageSource::Url { url } => {
                IMAGE_TOKEN_ESTIMATE.saturating_add(estimate_text_tokens(url))
            }
            crate::domain::ImageSource::Local { path } => {
                IMAGE_TOKEN_ESTIMATE.saturating_add(estimate_text_tokens(path))
            }
        })
    });
    8_u64
        .saturating_add(estimate_text_tokens(&message.content))
        .saturating_add(
            message
                .reasoning_content
                .as_deref()
                .map_or(0, estimate_text_tokens),
        )
        .saturating_add(
            message
                .tool_call_id
                .as_deref()
                .map_or(0, estimate_text_tokens),
        )
        .saturating_add(tool_call_tokens)
        .saturating_add(response_item_tokens)
        .saturating_add(attachment_tokens)
}

pub fn fixed_request_tokens(system_prompt: &str, tools: Option<&[ToolDefinition]>) -> u64 {
    let tool_tokens = tools.map_or(0, |definitions| {
        serde_json::to_string(definitions)
            .map(|value| estimate_text_tokens(&value))
            .unwrap_or_default()
    });
    estimate_text_tokens(system_prompt)
        .saturating_add(tool_tokens)
        .saturating_add(16)
}

fn usage_context_tokens(message: &ChatMessage) -> Option<u64> {
    message.usage.as_ref().map(|usage| {
        let prompt_tokens = if usage.tokens.total_prompt_tokens > 0 {
            usage.tokens.total_prompt_tokens
        } else {
            usage
                .tokens
                .input_tokens
                .saturating_add(usage.tokens.cache_read_tokens)
                .saturating_add(usage.tokens.cache_write_tokens)
        };
        prompt_tokens.saturating_add(usage.tokens.output_tokens)
    })
}

fn latest_usage_context_tokens(messages: &[ChatMessage]) -> Option<(usize, u64)> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| usage_context_tokens(message).map(|tokens| (index, tokens)))
}

fn projected_context_tokens(messages: &[ChatMessage]) -> u64 {
    let estimate = estimate_messages_tokens(messages);
    match latest_usage_context_tokens(messages) {
        Some((index, usage)) => usage
            .saturating_add(estimate_messages_tokens(
                &messages[index.saturating_add(1)..],
            ))
            .max(estimate),
        None => estimate,
    }
}

fn clear_message_usage(mut message: ChatMessage) -> ChatMessage {
    message.usage = None;
    message
}

fn compaction_instruction_message(agent: &Agent, covered: &[ChatMessage]) -> ChatMessage {
    ChatMessage {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        conversation_id: covered
            .last()
            .map(|message| message.conversation_id)
            .unwrap_or_else(Uuid::nil),
        role: ChatRole::User,
        content: COMPACTION_USER_PROMPT.to_string(),
        reasoning_content: None,
        openai_response_items: Vec::new(),
        status: ChatMessageStatus::Done,
        created_at: Utc::now(),
        tool_calls: None,
        tool_call_id: None,
        attachments: Vec::new(),
        usage: None,
    }
}

fn provider_for_compaction(provider: &ModelProvider) -> ModelProvider {
    let mut provider = provider.clone();
    let mut extra = provider.extra_body.as_object().cloned().unwrap_or_default();
    match provider.provider_type {
        ProviderType::OpenAiCompatible | ProviderType::AnthropicMessages => {
            extra.insert(
                "max_tokens".to_string(),
                serde_json::json!(COMPACTION_MAX_OUTPUT_TOKENS),
            );
        }
        ProviderType::OpenAiResponses => {
            extra.insert(
                "max_output_tokens".to_string(),
                serde_json::json!(COMPACTION_MAX_OUTPUT_TOKENS),
            );
        }
        ProviderType::GoogleGemini => {
            let mut generation_config = extra
                .get("generationConfig")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();
            generation_config.insert(
                "maxOutputTokens".to_string(),
                serde_json::json!(COMPACTION_MAX_OUTPUT_TOKENS),
            );
            extra.insert(
                "generationConfig".to_string(),
                serde_json::Value::Object(generation_config),
            );
        }
        ProviderType::OpenAiImages => {}
    }
    provider.extra_body = serde_json::Value::Object(extra);
    provider
}

pub fn projected_request_tokens(
    messages: &[ChatMessage],
    system_prompt: &str,
    tools: Option<&[ToolDefinition]>,
) -> u64 {
    projected_request_tokens_with_fixed(messages, fixed_request_tokens(system_prompt, tools))
}

/// Same as [`projected_request_tokens`] with a precomputed fixed-prompt token
/// count, so per-iteration callers avoid re-serializing the tool definitions.
pub fn projected_request_tokens_with_fixed(messages: &[ChatMessage], fixed_tokens: u64) -> u64 {
    let full_estimate = estimate_messages_tokens(messages).saturating_add(fixed_tokens);
    match latest_usage_context_tokens(messages) {
        Some((index, usage)) => usage
            .saturating_add(estimate_messages_tokens(
                &messages[index.saturating_add(1)..],
            ))
            .max(full_estimate),
        None => full_estimate,
    }
}

pub fn needs_compaction_with_fixed(
    messages: &[ChatMessage],
    config: &CompactionConfig,
    fixed_tokens: u64,
) -> bool {
    projected_request_tokens_with_fixed(messages, fixed_tokens) >= config.soft_limit_tokens()
}

/// Ensure `messages` fit under the global threshold. May call the model for a summary.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_within_threshold<T: Send>(
    client: &DynChatClient,
    agent: &Agent,
    provider: &ModelProvider,
    config: &CompactionConfig,
    system_prompt: &str,
    options: &RequestOptions,
    tools: Option<&[ToolDefinition]>,
    active_task_plan: Option<&ActiveTaskPlanContext>,
    mut messages: Vec<ChatMessage>,
    existing_checkpoint: Option<ContextCheckpoint>,
    cancel: Option<&tokio::sync::mpsc::Sender<T>>,
) -> AppResult<CompactOutcome> {
    refresh_active_task_plan_anchor(&mut messages, active_task_plan);
    let fixed_prompt_tokens = fixed_request_tokens(system_prompt, tools);
    if fixed_prompt_tokens >= config.soft_limit_tokens() {
        return Err(AppError::BadRequest(format!(
            "system prompt and tool definitions require about {fixed_prompt_tokens} tokens, exceeding the safe input budget of {} tokens for this model",
            config.soft_limit_tokens()
        )));
    }
    let before = projected_request_tokens(&messages, system_prompt, tools);
    if before < config.soft_limit_tokens() {
        return Ok(CompactOutcome {
            messages,
            checkpoint: existing_checkpoint,
            reason: CompactReason::WithinBudget,
            did_compact: false,
            estimated_tokens_before: before,
            estimated_tokens_after: before,
        });
    }

    // Prefer semantic summary; fall back to structural trim if the model call fails.
    match compact_with_summary(
        client,
        agent,
        provider,
        system_prompt,
        options,
        tools,
        active_task_plan,
        &messages,
        existing_checkpoint.as_ref(),
        config,
        cancel,
    )
    .await
    {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            tracing::warn!(%error, "semantic context compaction failed; applying structural trim");
            Ok(compact_structurally(
                messages,
                existing_checkpoint,
                config,
                fixed_prompt_tokens,
                before,
                active_task_plan,
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn compact_with_summary<T>(
    client: &DynChatClient,
    agent: &Agent,
    provider: &ModelProvider,
    system_prompt: &str,
    options: &RequestOptions,
    tools: Option<&[ToolDefinition]>,
    active_task_plan: Option<&ActiveTaskPlanContext>,
    messages: &[ChatMessage],
    existing_checkpoint: Option<&ContextCheckpoint>,
    config: &CompactionConfig,
    cancel: Option<&tokio::sync::mpsc::Sender<T>>,
) -> AppResult<CompactOutcome>
where
    T: Send,
{
    let before = projected_request_tokens(messages, system_prompt, tools);
    let (covered, tail) = split_for_compaction(messages, DEFAULT_RETAINED_TAIL_GROUPS);
    if covered.is_empty() {
        return Ok(compact_structurally(
            messages.to_vec(),
            existing_checkpoint.cloned(),
            config,
            fixed_request_tokens(system_prompt, tools),
            before,
            active_task_plan,
        ));
    }

    let mut summary_input = build_summarizer_messages(covered, existing_checkpoint);
    let previous_summary_id = existing_checkpoint
        .filter(|checkpoint| !checkpoint.summary.trim().is_empty())
        .and_then(|_| summary_input.first().map(|message| message.id));
    // Keep the summarizer request itself under budget.
    let summary_fixed_tokens = fixed_request_tokens(COMPACTION_SYSTEM_PROMPT, None).saturating_add(
        estimate_message_tokens(&compaction_instruction_message(agent, covered)),
    );
    let summary_input_budget = u64::from(config.context_size)
        .saturating_sub(COMPACTION_MAX_OUTPUT_TOKENS)
        .saturating_sub(summary_fixed_tokens)
        .max(1);
    summary_input =
        trim_messages_to_token_budget(summary_input, summary_input_budget, previous_summary_id);

    let mut summary_messages = summary_input;
    summary_messages.push(compaction_instruction_message(agent, covered));

    let mut compact_options = options.clone();
    compact_options.thinking_level = Some(ThinkingLevel::Off);
    // One retry is enough for compaction; keep it snappy.
    compact_options.retry_attempts = compact_options.retry_attempts.clamp(1, 2);

    let compaction_provider = provider_for_compaction(provider);
    let completion = client.complete(
        agent,
        &compaction_provider,
        &summary_messages,
        COMPACTION_SYSTEM_PROMPT,
        compact_options,
        None,
    );

    let output = if let Some(sender) = cancel {
        tokio::select! {
            result = completion => result?,
            _ = sender.closed() => {
                return Err(AppError::Llm("stream client disconnected".to_string()));
            }
        }
    } else {
        completion.await?
    };

    let summary = normalize_summary(&output.content);
    if summary.trim().is_empty() {
        return Err(AppError::Llm(
            "context compaction returned an empty summary".to_string(),
        ));
    }

    let covers_until_message_id = covered
        .last()
        .map(|message| message.id)
        .ok_or_else(|| AppError::Llm("context compaction covered no messages".to_string()))?;

    let generation = existing_checkpoint
        .map(|checkpoint| checkpoint.generation.saturating_add(1))
        .unwrap_or(1);
    let checkpoint = ContextCheckpoint {
        summary,
        covers_until_message_id,
        created_at: Utc::now(),
        generation,
        source_tokens_estimate: before,
    };

    let mut rebuilt = vec![summary_anchor_message(
        covered,
        &checkpoint,
        active_task_plan,
    )];
    rebuilt.extend(tail.iter().cloned().map(clear_message_usage));

    // If still oversized (huge tail), structurally trim the tail while keeping the summary.
    let request_fixed_tokens = fixed_request_tokens(system_prompt, tools);
    let target_message_tokens = config
        .target_tokens_after_compact()
        .saturating_sub(request_fixed_tokens)
        .max(1);
    let after_estimate = projected_request_tokens(&rebuilt, system_prompt, tools);
    let rebuilt = if after_estimate >= config.soft_limit_tokens() {
        trim_messages_to_token_budget(
            rebuilt,
            target_message_tokens,
            Some(checkpoint.covers_until_message_id),
        )
    } else {
        rebuilt
    };

    let after = projected_request_tokens(&rebuilt, system_prompt, tools);
    Ok(CompactOutcome {
        messages: rebuilt,
        checkpoint: Some(checkpoint),
        reason: CompactReason::SemanticSummary,
        did_compact: true,
        estimated_tokens_before: before,
        estimated_tokens_after: after,
    })
}

fn compact_structurally(
    messages: Vec<ChatMessage>,
    existing_checkpoint: Option<ContextCheckpoint>,
    config: &CompactionConfig,
    fixed_prompt_tokens: u64,
    before: u64,
    active_task_plan: Option<&ActiveTaskPlanContext>,
) -> CompactOutcome {
    let target = config
        .target_tokens_after_compact()
        .saturating_sub(fixed_prompt_tokens)
        .max(1);
    let (covered, tail) = split_for_compaction(&messages, DEFAULT_RETAINED_TAIL_GROUPS.max(1));
    let mechanical = mechanical_summary(covered, existing_checkpoint.as_ref());
    let covers_until = covered
        .last()
        .map(|message| message.id)
        .or_else(|| messages.last().map(|message| message.id));

    let Some(covers_until_message_id) = covers_until else {
        return CompactOutcome {
            messages: trim_messages_to_token_budget(messages, target, None),
            checkpoint: existing_checkpoint,
            reason: CompactReason::StructuralTrim,
            did_compact: true,
            estimated_tokens_before: before,
            estimated_tokens_after: target,
        };
    };

    let generation = existing_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.generation.saturating_add(1))
        .unwrap_or(1);
    let checkpoint = ContextCheckpoint {
        summary: mechanical,
        covers_until_message_id,
        created_at: Utc::now(),
        generation,
        source_tokens_estimate: before,
    };
    let covered_for_anchor = if covered.is_empty() {
        messages.as_slice()
    } else {
        covered
    };
    let mut rebuilt = vec![summary_anchor_message(
        covered_for_anchor,
        &checkpoint,
        active_task_plan,
    )];
    rebuilt.extend(tail.iter().cloned().map(clear_message_usage));
    let rebuilt =
        trim_messages_to_token_budget(rebuilt, target, Some(checkpoint.covers_until_message_id));
    let after = projected_context_tokens(&rebuilt).saturating_add(fixed_prompt_tokens);
    CompactOutcome {
        messages: rebuilt,
        checkpoint: Some(checkpoint),
        reason: CompactReason::StructuralTrim,
        did_compact: true,
        estimated_tokens_before: before,
        estimated_tokens_after: after,
    }
}

fn split_for_compaction(
    messages: &[ChatMessage],
    retain_tail_groups: usize,
) -> (&[ChatMessage], &[ChatMessage]) {
    if messages.is_empty() {
        return (&[], &[]);
    }
    let groups = owned_context_groups(messages);
    if groups.len() <= retain_tail_groups {
        // Still compact most of it: keep only the newest group as tail.
        if groups.len() <= 1 {
            return (messages, &[]);
        }
        let keep = 1;
        let tail_start_idx = groups.len().saturating_sub(keep);
        let start_message_index = groups[..tail_start_idx]
            .iter()
            .map(|group| group.len())
            .sum::<usize>();
        return messages.split_at(start_message_index.min(messages.len()));
    }
    let tail_start_group = groups.len().saturating_sub(retain_tail_groups);
    let start_message_index = groups[..tail_start_group]
        .iter()
        .map(|group| group.len())
        .sum::<usize>();
    messages.split_at(start_message_index.min(messages.len()))
}

fn owned_context_groups(messages: &[ChatMessage]) -> Vec<Vec<usize>> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if message.role == ChatRole::Assistant && message.tool_calls.is_some() {
            let mut group = vec![index];
            index += 1;
            while index < messages.len() && messages[index].role == ChatRole::Tool {
                group.push(index);
                index += 1;
            }
            groups.push(group);
        } else if message.role == ChatRole::Tool {
            // Orphan tool rows are skipped as their own empty contribution.
            index += 1;
        } else {
            groups.push(vec![index]);
            index += 1;
        }
    }
    groups
}

fn build_summarizer_messages(
    covered: &[ChatMessage],
    existing_checkpoint: Option<&ContextCheckpoint>,
) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    if let Some(checkpoint) = existing_checkpoint
        && !checkpoint.summary.trim().is_empty()
    {
        out.push(ChatMessage {
            id: Uuid::new_v4(),
            agent_id: covered
                .first()
                .map(|message| message.agent_id)
                .unwrap_or_else(Uuid::nil),
            conversation_id: covered
                .first()
                .map(|message| message.conversation_id)
                .unwrap_or_else(Uuid::nil),
            role: ChatRole::User,
            content: format!(
                "[Previous compaction summary]\n{}",
                checkpoint.summary.trim()
            ),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: checkpoint.created_at,
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        });
    }
    for message in covered {
        if existing_checkpoint.is_some_and(|checkpoint| {
            message.id == checkpoint.covers_until_message_id
                && message.content.starts_with(SUMMARY_ANCHOR_PREFIX)
        }) {
            continue;
        }
        out.push(truncate_message_for_summary(message));
    }
    out
}

fn truncate_message_for_summary(message: &ChatMessage) -> ChatMessage {
    let mut cloned = message.clone();
    let limit = if cloned.role == ChatRole::Tool {
        MAX_SUMMARY_TOOL_CHARS
    } else {
        MAX_SUMMARY_MESSAGE_CHARS
    };
    cloned.content = truncate_chars(&cloned.content, limit);
    if let Some(reasoning) = cloned.reasoning_content.as_mut() {
        *reasoning = truncate_chars(reasoning, MAX_SUMMARY_TOOL_CHARS);
    }
    cloned.openai_response_items.clear();
    // Drop bulky attachments from the summarizer request; note presence only.
    if !cloned.attachments.is_empty() {
        let count = cloned.attachments.len();
        cloned.attachments.clear();
        if !cloned.content.contains("[attachments omitted") {
            cloned
                .content
                .push_str(&format!("\n[attachments omitted: {count} image(s)]"));
        }
    }
    cloned.usage = None;
    cloned
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    if max_chars < 64 {
        return input.chars().take(max_chars).collect();
    }
    let keep_head = max_chars.saturating_mul(70) / 100;
    let keep_tail = max_chars.saturating_sub(keep_head + 32);
    let head: String = input.chars().take(keep_head).collect();
    let tail: String = input
        .chars()
        .rev()
        .take(keep_tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!(
        "{head}\n…[truncated {omitted} chars]…\n{tail}",
        omitted = count - keep_head - keep_tail
    )
}

fn trim_messages_to_token_budget(
    messages: Vec<ChatMessage>,
    budget_tokens: u64,
    required_prefix_id: Option<Uuid>,
) -> Vec<ChatMessage> {
    let messages = messages
        .into_iter()
        .map(clear_message_usage)
        .collect::<Vec<_>>();
    if messages.is_empty() || estimate_messages_tokens(&messages) <= budget_tokens {
        return messages;
    }
    let groups = owned_context_groups(&messages);
    if groups.is_empty() {
        return messages;
    }
    let required_index =
        required_prefix_id.and_then(|id| messages.iter().position(|message| message.id == id));
    let required_message = required_index.map(|index| messages[index].clone());
    let required_tokens = required_message.as_ref().map_or(0, estimate_message_tokens);
    let available_for_tail = budget_tokens.saturating_sub(required_tokens);
    let mut selected_rev: Vec<Vec<usize>> = Vec::new();
    let mut tokens = 0_u64;
    for group in groups.iter().rev() {
        if required_index.is_some_and(|required| group.contains(&required)) {
            continue;
        }
        let group_messages: Vec<ChatMessage> = group.iter().map(|&i| messages[i].clone()).collect();
        let group_tokens = estimate_messages_tokens(&group_messages);
        if !selected_rev.is_empty() && tokens.saturating_add(group_tokens) > available_for_tail {
            break;
        }
        tokens = tokens.saturating_add(group_tokens);
        selected_rev.push(group.clone());
        if selected_rev.len() == 1 && tokens > available_for_tail {
            // The newest complete group is mandatory; force-fit it below.
            break;
        }
    }
    selected_rev.reverse();
    let mut out = Vec::new();
    if let Some(required) = required_message {
        out.push(required);
    }
    for group in selected_rev {
        for index in group {
            out.push(messages[index].clone());
        }
    }
    if out.is_empty()
        && let Some(last) = messages.last()
    {
        let mut last = last.clone();
        last.content = truncate_chars(&last.content, 2_000);
        last.attachments.clear();
        out.push(last);
    }
    force_fit_messages(out, budget_tokens, required_prefix_id)
}

fn force_fit_messages(
    mut messages: Vec<ChatMessage>,
    budget_tokens: u64,
    required_prefix_id: Option<Uuid>,
) -> Vec<ChatMessage> {
    if estimate_messages_tokens(&messages) <= budget_tokens {
        return messages;
    }

    for message in &mut messages {
        message.usage = None;
        message.reasoning_content = None;
        message.openai_response_items.clear();
        if !message.attachments.is_empty() {
            let count = message.attachments.len();
            message.attachments.clear();
            message.content.push_str(&format!(
                "\n[attachments omitted during emergency trim: {count}]"
            ));
        }
        if message.role == ChatRole::Tool {
            message.content = truncate_chars(&message.content, 800);
        }
        if required_prefix_id != Some(message.id) {
            compact_tool_call_arguments(message);
        }
    }

    while estimate_messages_tokens(&messages) > budget_tokens {
        let candidate = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                let current = message.content.chars().count();
                if current <= 16 {
                    return false;
                }
                required_prefix_id != Some(message.id)
                    || truncate_summary_anchor_preserving_plan(&message.content, current / 2)
                        .chars()
                        .count()
                        < current
            })
            .max_by_key(|(_, message)| message.content.chars().count())
            .map(|(index, _)| index);
        let Some(index) = candidate else {
            break;
        };
        let current = messages[index].content.chars().count();
        messages[index].content = if required_prefix_id == Some(messages[index].id) {
            truncate_summary_anchor_preserving_plan(&messages[index].content, current / 2)
        } else {
            truncate_chars(&messages[index].content, current / 2)
        };
    }

    while estimate_messages_tokens(&messages) > budget_tokens && messages.len() > 2 {
        let groups = owned_context_groups(&messages);
        let removable = groups
            .iter()
            .take(groups.len().saturating_sub(1))
            .find(|group| {
                !group
                    .iter()
                    .any(|&index| required_prefix_id.is_some_and(|id| messages[index].id == id))
            });
        let Some(group) = removable else {
            break;
        };
        for &index in group.iter().rev() {
            messages.remove(index);
        }
    }

    messages
}

fn truncate_summary_anchor_preserving_plan(content: &str, max_chars: usize) -> String {
    let Some(plan_start) = content.find(ACTIVE_TASK_PLAN_PREFIX) else {
        return truncate_chars(content, max_chars);
    };
    let summary = content[..plan_start].trim_end();
    let plan = content[plan_start..].trim_start();
    let summary_budget = max_chars.saturating_sub(plan.chars().count().saturating_add(2));
    if summary_budget == 0 {
        return format!("{SUMMARY_ANCHOR_PREFIX}\n\n{plan}");
    }
    format!("{}\n\n{plan}", truncate_chars(summary, summary_budget))
}

fn compact_tool_call_arguments(message: &mut ChatMessage) {
    let Some(calls) = message
        .tool_calls
        .as_mut()
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for call in calls {
        let Some(arguments) = call
            .get_mut("function")
            .and_then(|function| function.get_mut("arguments"))
        else {
            continue;
        };
        if arguments
            .as_str()
            .is_some_and(|value| value.chars().count() > 2_000)
        {
            *arguments = serde_json::Value::String(r#"{"_context_compacted":true}"#.to_string());
        }
    }
}

fn mechanical_summary(
    covered: &[ChatMessage],
    existing_checkpoint: Option<&ContextCheckpoint>,
) -> String {
    let mut lines = Vec::new();
    lines.push("1. User goal".to_string());
    if let Some(first_user) = covered.iter().find(|message| {
        message.role == ChatRole::User && !message.content.starts_with(SUMMARY_ANCHOR_PREFIX)
    }) {
        lines.push(format!(
            "- {}",
            truncate_chars(first_user.content.trim(), 400)
        ));
    } else if let Some(checkpoint) = existing_checkpoint {
        lines.push(format!(
            "- (from prior summary) {}",
            truncate_chars(checkpoint.summary.trim(), 400)
        ));
    } else {
        lines.push("- (unavailable after emergency trim)".to_string());
    }
    lines.push("2. Completed work".to_string());
    let assistant_bits: Vec<_> = covered
        .iter()
        .filter(|message| message.role == ChatRole::Assistant && !message.content.trim().is_empty())
        .rev()
        .take(3)
        .map(|message| format!("- {}", truncate_chars(message.content.trim(), 240)))
        .collect();
    if assistant_bits.is_empty() {
        lines.push("- See retained recent turns.".to_string());
    } else {
        lines.extend(assistant_bits.into_iter().rev());
    }
    lines.push("3. In progress".to_string());
    lines.push("- Context was auto-compacted after structural trim.".to_string());
    lines.push("4. Remaining / TODO".to_string());
    lines.push("- Continue from the retained recent messages and this summary.".to_string());
    lines.push("5. Critical constraints, decisions, paths, IDs, configs, errors".to_string());
    if let Some(checkpoint) = existing_checkpoint {
        lines.push(format!(
            "- Prior summary excerpt: {}",
            truncate_chars(checkpoint.summary.trim(), 600)
        ));
    } else {
        lines.push("- None captured during emergency trim.".to_string());
    }
    lines.push("6. Next concrete step".to_string());
    lines.push("- Resume the user's latest request using the retained tail.".to_string());
    lines.join("\n")
}

fn normalize_summary(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= MAX_SUMMARY_CHARS {
        return trimmed.to_string();
    }
    truncate_chars(trimmed, MAX_SUMMARY_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{ModelUsage, TokenUsage, settings::DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT},
        llm::types::FunctionDefinition,
    };

    fn test_config(
        context_size: u32,
        threshold_percent: u8,
        output_reserve_tokens: u64,
    ) -> CompactionConfig {
        CompactionConfig {
            context_size,
            threshold_percent,
            output_reserve_tokens,
        }
    }

    fn msg(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            role,
            content: content.to_string(),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }
    }

    #[test]
    fn soft_limit_respects_threshold_band() {
        let config = test_config(100_000, 90, 4_096);
        assert_eq!(config.soft_limit_tokens(), 90_000);
        let config = test_config(100_000, 1, 4_096);
        assert_eq!(config.soft_limit_tokens(), 1_000);
        let small = test_config(8_000, 90, 2_000);
        assert_eq!(small.soft_limit_tokens(), 6_000);
    }

    #[test]
    fn build_llm_messages_injects_summary_and_keeps_tail() {
        let mut a = msg(ChatRole::User, "old");
        let mut b = msg(ChatRole::Assistant, "old answer");
        let mut c = msg(ChatRole::User, "new");
        a.id = Uuid::new_v4();
        b.id = Uuid::new_v4();
        c.id = Uuid::new_v4();
        let checkpoint = ContextCheckpoint {
            summary: "goal: ship compact".into(),
            covers_until_message_id: b.id,
            created_at: Utc::now(),
            generation: 1,
            source_tokens_estimate: 100,
        };
        let plan = ActiveTaskPlanContext {
            planning_id: Uuid::new_v4(),
            current_step: "Implement compaction".to_string(),
            steps: vec![
                "1. [in_progress] Implement compaction".to_string(),
                "2. [pending] Run tests".to_string(),
            ],
        };
        let built = build_llm_messages(&[a, b, c.clone()], Some(&checkpoint), Some(&plan));
        assert_eq!(built.len(), 2);
        assert!(built[0].content.contains("goal: ship compact"));
        assert!(
            built[0]
                .content
                .contains(&format!("planning_id: {}", plan.planning_id))
        );
        assert!(
            built[0]
                .content
                .contains("current_step: Implement compaction")
        );
        assert_eq!(built[1].id, c.id);
    }

    #[test]
    fn checkpoint_rewrite_drops_stale_usage_snapshot() {
        let old_user = msg(ChatRole::User, "old");
        let old_assistant = msg(ChatRole::Assistant, "old answer");
        let mut retained = msg(ChatRole::Assistant, "recent answer");
        retained.usage = Some(ModelUsage {
            tokens: TokenUsage {
                total_prompt_tokens: 90_000,
                output_tokens: 5_000,
                ..TokenUsage::default()
            },
            total_tokens: 95_000,
            cost_usd: None,
            provider_id: Uuid::new_v4(),
            model_id: "test".to_string(),
        });
        let checkpoint = ContextCheckpoint {
            summary: "compact state".to_string(),
            covers_until_message_id: old_assistant.id,
            created_at: Utc::now(),
            generation: 1,
            source_tokens_estimate: 95_000,
        };

        let built = build_llm_messages(
            &[old_user, old_assistant, retained],
            Some(&checkpoint),
            None,
        );

        assert!(built.iter().all(|message| message.usage.is_none()));
        assert!(projected_context_tokens(&built) < 1_000);
    }

    #[test]
    fn repeated_compaction_includes_previous_summary_once() {
        let covered = vec![
            ChatMessage {
                id: Uuid::new_v4(),
                agent_id: Uuid::new_v4(),
                conversation_id: Uuid::new_v4(),
                role: ChatRole::User,
                content: format!("{SUMMARY_ANCHOR_PREFIX}\nprevious state"),
                reasoning_content: None,
                openai_response_items: Vec::new(),
                status: ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: None,
                tool_call_id: None,
                attachments: Vec::new(),
                usage: None,
            },
            msg(ChatRole::User, "new work"),
        ];
        let checkpoint = ContextCheckpoint {
            summary: "previous state".to_string(),
            covers_until_message_id: covered[0].id,
            created_at: Utc::now(),
            generation: 1,
            source_tokens_estimate: 100,
        };

        let summary_input = build_summarizer_messages(&covered, Some(&checkpoint));
        let occurrences = summary_input
            .iter()
            .filter(|message| message.content.contains("previous state"))
            .count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn structural_trim_reduces_projected_tokens() {
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(msg(
                if i % 2 == 0 {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                &"x".repeat(2_000),
            ));
        }
        let before = projected_context_tokens(&messages);
        let config = test_config(8_000, DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT, 2_000);
        let outcome = compact_structurally(messages, None, &config, 200, before, None);
        assert!(outcome.did_compact);
        assert!(outcome.estimated_tokens_after < before);
        assert!(outcome.checkpoint.is_some());
    }

    #[test]
    fn needs_compaction_uses_soft_limit() {
        let messages = vec![msg(ChatRole::User, &"a".repeat(20_000))];
        let config = test_config(1_000, 50, 100);
        assert!(needs_compaction_with_fixed(
            &messages,
            &config,
            fixed_request_tokens("", None),
        ));
        let tiny = vec![msg(ChatRole::User, "hi")];
        assert!(!needs_compaction_with_fixed(
            &tiny,
            &config,
            fixed_request_tokens("", None),
        ));
    }

    #[test]
    fn request_budget_counts_system_prompt_and_tools() {
        let messages = vec![msg(ChatRole::User, "hi")];
        let tools = vec![ToolDefinition {
            kind: "function".to_string(),
            function: FunctionDefinition {
                name: "large_tool".to_string(),
                description: "x".repeat(4_000),
                parameters: serde_json::json!({"type": "object"}),
            },
        }];
        let config = test_config(1_000, 90, 100);

        assert!(needs_compaction_with_fixed(
            &messages,
            &config,
            fixed_request_tokens(&"s".repeat(1_000), Some(&tools)),
        ));
    }

    #[test]
    fn emergency_trim_keeps_summary_and_latest_group() {
        let mut summary = msg(
            ChatRole::User,
            &format!("{SUMMARY_ANCHOR_PREFIX}\n{}", "s".repeat(4_000)),
        );
        let latest = msg(ChatRole::User, &"latest".repeat(2_000));
        let summary_id = summary.id;
        summary.usage = None;

        let trimmed =
            trim_messages_to_token_budget(vec![summary, latest.clone()], 800, Some(summary_id));

        assert_eq!(trimmed.first().map(|message| message.id), Some(summary_id));
        assert_eq!(trimmed.last().map(|message| message.id), Some(latest.id));
        assert!(estimate_messages_tokens(&trimmed) <= 800);
    }

    #[test]
    fn emergency_trim_preserves_active_plan_inside_summary_anchor() {
        let plan = ActiveTaskPlanContext {
            planning_id: Uuid::new_v4(),
            current_step: "Keep the plan ID".to_string(),
            steps: vec!["1. [in_progress] Keep the plan ID".to_string()],
        };
        let summary = msg(
            ChatRole::User,
            &render_summary_anchor_content(&"s".repeat(4_000), Some(&plan)),
        );
        let summary_id = summary.id;
        let latest = msg(ChatRole::User, &"latest".repeat(2_000));

        let trimmed =
            trim_messages_to_token_budget(vec![summary, latest.clone()], 800, Some(summary_id));

        assert_eq!(trimmed.first().map(|message| message.id), Some(summary_id));
        assert!(
            trimmed[0]
                .content
                .contains(&format!("planning_id: {}", plan.planning_id))
        );
        assert!(
            trimmed[0]
                .content
                .contains("current_step: Keep the plan ID")
        );
    }
}
