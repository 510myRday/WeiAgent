import { expect, test } from 'vitest';

import {
  agentRunRuntimeSchema,
  conversationMessagePageRuntimeSchema,
  parseJsonStreamEvent,
} from './apiSchemas';

test('conversation page accepts a null cursor on the last history page', () => {
  expect(conversationMessagePageRuntimeSchema.parse({
    messages: [{
      id: 'message-1',
      agent_id: 'agent-1',
      conversation_id: 'conversation-1',
      role: 'user',
      content: 'hello',
      status: 'done',
      created_at: '2026-07-17T00:00:00Z',
    }],
    next_cursor: null,
    has_more: false,
  })).toMatchObject({
    next_cursor: null,
    has_more: false,
  });
});

test('agent run accepts nullable fields emitted by Rust Option values', () => {
  expect(agentRunRuntimeSchema.parse({
    id: 'run-1',
    conversation_id: 'conversation-1',
    user_message_id: null,
    status: 'running',
    phase: null,
    latest_sequence: 0,
    error: null,
    created_at: '2026-07-17T00:00:00Z',
    finished_at: null,
  })).toMatchObject({
    user_message_id: null,
    phase: null,
    error: null,
    finished_at: null,
  });
});

test('stream schema rejects malformed tool calls', () => {
  expect(() => parseJsonStreamEvent('tool_call', JSON.stringify({
    call_id: 'call-1',
    name: 'terminal',
    arguments: [],
    permission_mode: 'full_access',
  }))).toThrow();
});

test('stream schema accepts incremental tool call events', () => {
  expect(parseJsonStreamEvent('tool_call_started', JSON.stringify({
    call_id: 'call-1',
    name: 'execute_terminal',
    permission_mode: 'full_access',
  }))).toMatchObject({
    event: 'tool_call_started',
    data: {
      call_id: 'call-1',
      name: 'execute_terminal',
      permission_mode: 'full_access',
    },
  });

  expect(parseJsonStreamEvent('tool_call_args_delta', JSON.stringify({
    call_id: 'call-1',
    arguments: '{"command":"echo',
  }))).toMatchObject({
    event: 'tool_call_args_delta',
    data: {
      call_id: 'call-1',
      arguments: '{"command":"echo',
    },
  });
});

test('stream schema accepts valid phase events', () => {
  expect(parseJsonStreamEvent('phase', JSON.stringify({
    phase: 'waiting_for_model',
    iteration: 1,
  }))).toEqual({
    event: 'phase',
    data: {
      phase: 'waiting_for_model',
      iteration: 1,
    },
    sequence: undefined,
  });
});

test('stream schema accepts null optional fields emitted by Rust events', () => {
  expect(parseJsonStreamEvent('phase', JSON.stringify({
    phase: 'waiting_for_model',
    iteration: 0,
    call_id: null,
  }))).toEqual({
    event: 'phase',
    data: {
      phase: 'waiting_for_model',
      iteration: 0,
    },
    sequence: undefined,
  });

  expect(parseJsonStreamEvent('approval_required', JSON.stringify({
    call_id: 'call-1',
    agent_id: 'agent-1',
    command: 'echo hello',
    approval_id: 'approval-1',
    cwd: null,
    runtime: 'codex_sandbox',
    permission: 'terminal',
    justification: null,
    background: false,
  }))).toEqual({
    event: 'approval_required',
    data: {
      call_id: 'call-1',
      agent_id: 'agent-1',
      command: 'echo hello',
      approval_id: 'approval-1',
      runtime: 'codex_sandbox',
      permission: 'terminal',
      background: false,
    },
    sequence: undefined,
  });
});
