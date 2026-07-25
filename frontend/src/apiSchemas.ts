import {
  array,
  boolean,
  custom,
  literal,
  looseObject,
  nullable,
  number,
  optional,
  parse,
  picklist,
  string,
  type BaseIssue,
  type BaseSchema,
} from 'valibot';

import type {
  AgentRun,
  ChatMessage,
  ConversationMessagePage,
  StreamEvent,
} from './types';

const permissionModeSchema = picklist(['request_approval', 'full_access']);
const toolCallStatusSchema = picklist([
  'running',
  'pending_approval',
  'completed',
  'blocked',
  'rejected',
  'error',
]);
const agentRunPhaseSchema = picklist([
  'waiting_for_model',
  'compacting_context',
  'executing_tool',
  'waiting_for_approval',
]);
const toolArgumentsSchema = custom<Record<string, unknown>>(
  (value) => Boolean(value) && typeof value === 'object' && !Array.isArray(value),
  'Tool arguments must be an object',
);

const chatToolCallSchema = looseObject({
  id: string(),
  type: literal('function'),
  permission_mode: optional(permissionModeSchema),
  function: looseObject({
    name: string(),
    arguments: string(),
  }),
});

const imageAttachmentSchema = looseObject({
  id: string(),
  name: string(),
  media_type: picklist(['image/png', 'image/jpeg', 'image/webp', 'image/gif']),
  source_type: picklist(['base64', 'url']),
  data: optional(string()),
  url: optional(string()),
});

const tokenUsageFields = {
  input_tokens: number(),
  output_tokens: number(),
  cache_write_tokens: number(),
  cache_read_tokens: number(),
  total_prompt_tokens: optional(number()),
  uncached_input_tokens: optional(number()),
  cache_hit_ratio: optional(number()),
};

const modelUsageSchema = looseObject({
  ...tokenUsageFields,
  total_tokens: number(),
  cost_usd: optional(nullable(number())),
  provider_id: string(),
  model_id: string(),
});

const conversationUsageSchema = looseObject({
  ...tokenUsageFields,
  total_tokens: number(),
  cost_usd: optional(nullable(number())),
});

export const chatMessageSchema = looseObject({
  id: string(),
  agent_id: string(),
  conversation_id: string(),
  role: picklist(['user', 'assistant', 'tool']),
  content: string(),
  reasoning_content: optional(string()),
  status: optional(picklist(['done', 'streaming', 'failed'])),
  created_at: string(),
  tool_calls: optional(array(chatToolCallSchema)),
  tool_call_id: optional(string()),
  attachments: optional(array(imageAttachmentSchema)),
  usage: optional(modelUsageSchema),
});

export const conversationMessagePageSchema = looseObject({
  messages: array(chatMessageSchema),
  next_cursor: optional(nullable(string())),
  has_more: boolean(),
  usage: optional(conversationUsageSchema),
});

export const agentRunSchema = looseObject({
  id: string(),
  conversation_id: string(),
  user_message_id: optional(nullable(string())),
  status: picklist(['running', 'completed', 'failed', 'cancelled']),
  phase: optional(nullable(agentRunPhaseSchema)),
  latest_sequence: number(),
  error: optional(nullable(string())),
  created_at: string(),
  finished_at: optional(nullable(string())),
});

const phaseEventDataSchema = looseObject({
  phase: agentRunPhaseSchema,
  iteration: number(),
  call_id: optional(nullable(string())),
});

const retryEventDataSchema = looseObject({
  attempt: number(),
  max_attempts: number(),
  reason: string(),
});

const toolCallEventDataSchema = looseObject({
  call_id: string(),
  name: string(),
  arguments: toolArgumentsSchema,
  permission_mode: permissionModeSchema,
});

const toolCallStartedEventDataSchema = looseObject({
  call_id: string(),
  name: string(),
  permission_mode: permissionModeSchema,
});

const toolCallArgsDeltaEventDataSchema = looseObject({
  call_id: string(),
  arguments: string(),
});

const toolResultEventDataSchema = looseObject({
  call_id: string(),
  result: looseObject({
    stdout: optional(string()),
    stderr: optional(string()),
    exit_code: optional(number()),
    duration_ms: optional(number()),
    process_id: optional(string()),
    output_cursor: optional(number()),
    message: optional(string()),
    reason: optional(string()),
    runtime: optional(string()),
    status: toolCallStatusSchema,
  }),
});

const approvalRequiredEventDataSchema = looseObject({
  call_id: string(),
  agent_id: string(),
  command: string(),
  approval_id: string(),
  cwd: optional(nullable(string())),
  runtime: string(),
  permission: string(),
  justification: optional(nullable(string())),
  background: boolean(),
});

type AnySchema = BaseSchema<unknown, unknown, BaseIssue<unknown>>;

export function runtimeSchema<T>(schema: AnySchema): { parse: (value: unknown) => T } {
  return {
    parse: (value) => parse(schema, value) as T,
  };
}

export const conversationMessagePageRuntimeSchema = runtimeSchema<ConversationMessagePage>(
  conversationMessagePageSchema,
);
export const agentRunRuntimeSchema = runtimeSchema<AgentRun>(agentRunSchema);
export const nullableAgentRunRuntimeSchema = runtimeSchema<AgentRun | null>(
  nullable(agentRunSchema),
);
export const agentRunListRuntimeSchema = runtimeSchema<AgentRun[]>(array(agentRunSchema));

export function parseChatMessage(value: unknown): ChatMessage {
  return parse(chatMessageSchema, value) as ChatMessage;
}

export function parseJsonStreamEvent(
  event: 'user' | 'assistant' | 'phase' | 'retry' | 'tool_call_started' | 'tool_call_args_delta' | 'tool_call' | 'tool_result' | 'approval_required',
  data: string,
  sequence?: number,
): StreamEvent {
  const parsed: unknown = JSON.parse(data);
  switch (event) {
    case 'user':
    case 'assistant':
      return { event, data: parseChatMessage(parsed), sequence };
    case 'phase': {
      const { call_id, ...phaseData } = parse(phaseEventDataSchema, parsed);
      return {
        event,
        data: call_id == null ? phaseData : { ...phaseData, call_id },
        sequence,
      };
    }
    case 'retry':
      return { event, data: parse(retryEventDataSchema, parsed), sequence };
    case 'tool_call_started':
      return { event, data: parse(toolCallStartedEventDataSchema, parsed), sequence };
    case 'tool_call_args_delta':
      return { event, data: parse(toolCallArgsDeltaEventDataSchema, parsed), sequence };
    case 'tool_call':
      return { event, data: parse(toolCallEventDataSchema, parsed), sequence };
    case 'tool_result':
      return { event, data: parse(toolResultEventDataSchema, parsed), sequence };
    case 'approval_required': {
      const { cwd, justification, ...approvalData } = parse(
        approvalRequiredEventDataSchema,
        parsed,
      );
      return {
        event,
        data: {
          ...approvalData,
          ...(cwd == null ? {} : { cwd }),
          ...(justification == null ? {} : { justification }),
        },
        sequence,
      };
    }
  }
}
