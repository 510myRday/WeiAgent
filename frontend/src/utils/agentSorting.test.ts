import { expect, test } from 'vitest';

import type { Agent } from '../types';
import { sortAgentsByRecentConversation } from './agentSorting';

const agents: Agent[] = [
  {
    id: 'agent-old', name: '旧智能体', description: '', model_provider_id: 'provider-1', model_id: 'model-1',
    avatar_icon: 'bot', avatar_color: '#07c160', is_builtin: false,
    created_at: '2026-08-01T00:00:00Z', updated_at: '2026-08-01T00:00:00Z',
  },
  {
    id: 'agent-recent', name: '最近使用', description: '', model_provider_id: 'provider-1', model_id: 'model-2',
    avatar_icon: 'sparkles', avatar_color: '#ff4fa3', is_builtin: false,
    created_at: '2026-08-02T00:00:00Z', updated_at: '2026-08-02T00:00:00Z',
  },
  {
    id: 'agent-never-used', name: '未使用', description: '', model_provider_id: 'provider-1', model_id: 'model-3',
    avatar_icon: 'zap', avatar_color: '#08a6d9', is_builtin: false,
    created_at: '2026-08-03T00:00:00Z', updated_at: '2026-08-03T00:00:00Z',
  },
];

test('puts the agent with the newest conversation activity first', () => {
  const sorted = sortAgentsByRecentConversation(agents, {
    'agent-old': '2026-08-20T10:00:00Z',
    'agent-recent': '2026-08-21T10:00:00Z',
  });

  expect(sorted.map((agent) => agent.id)).toEqual(['agent-recent', 'agent-old', 'agent-never-used']);
});

test('keeps the original order when activity timestamps are tied or missing', () => {
  const sorted = sortAgentsByRecentConversation(agents, { 'agent-old': 'not-a-date' });

  expect(sorted.map((agent) => agent.id)).toEqual(['agent-old', 'agent-recent', 'agent-never-used']);
});
