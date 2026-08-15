import { shouldShowAgentWaitingBubble } from './messageListItems';
import type { UiMessageItem } from '../../types';

function assistantMessage(): UiMessageItem {
  return {
    id: 'assistant-1',
    agent_id: 'agent-1',
    conversation_id: 'conversation-1',
    role: 'assistant',
    content: 'tool round',
    status: 'streaming',
    created_at: '2026-08-15T00:00:00.000Z',
    clientId: 'client-1',
  };
}

test('keeps retry details inside an existing assistant bubble', () => {
  expect(shouldShowAgentWaitingBubble(
    [assistantMessage()],
    true,
    'waiting_for_model',
    { attempt: 1, maxAttempts: 3, reason: 'upstream error' },
  )).toBe(false);
});

test('shows a standalone retry bubble when no assistant bubble exists', () => {
  expect(shouldShowAgentWaitingBubble(
    [],
    true,
    'waiting_for_model',
    { attempt: 1, maxAttempts: 3, reason: 'upstream error' },
  )).toBe(true);
});
