import { describe, expect, test } from 'vitest';

import type { UiMessageItem } from '../../types';
import { getTaskPlanSyncKey } from './taskPlanSync';

function message(status: 'running' | 'completed', result?: Record<string, unknown>): UiMessageItem {
  return {
    id: 'assistant-1',
    agent_id: 'agent-1',
    conversation_id: 'conversation-1',
    role: 'assistant',
    content: '',
    status: 'streaming',
    created_at: '2026-08-15T00:00:00Z',
    clientId: 'client-1',
    blocks: [{
      type: 'tool',
      tool: {
        call_id: 'task-call-1',
        name: 'task_planning',
        arguments: { action: 'create' },
        status,
        result,
      },
    }],
  };
}

describe('getTaskPlanSyncKey', () => {
  test('changes when a task plan tool result is added without adding a message', () => {
    const runningKey = getTaskPlanSyncKey([message('running')]);
    const completedKey = getTaskPlanSyncKey([message('completed', { status: 'completed', plan: [{ step: 'Ship it' }] })]);

    expect(completedKey).not.toBe(runningKey);
  });

  test('ignores unrelated tool results', () => {
    const base = message('running');
    const unrelated: UiMessageItem = {
      ...base,
      blocks: [
        ...base.blocks!,
        {
          type: 'tool',
          tool: {
            call_id: 'other-call-1',
            name: 'terminal',
            status: 'completed',
            arguments: {},
            result: { stdout: 'done' },
          },
        },
      ],
    };

    expect(getTaskPlanSyncKey([base])).toBe(getTaskPlanSyncKey([unrelated]));
  });
});
