import { mergeAssistantEvent } from './conversationMessages';
import type { UiMessage } from '../types';

function assistant(overrides: Partial<UiMessage> = {}): UiMessage {
  return {
    id: 'assistant-1',
    agent_id: 'agent-1',
    conversation_id: 'conversation-1',
    role: 'assistant',
    content: 'model turn',
    status: 'done',
    created_at: '2026-08-15T00:00:00.000Z',
    clientId: 'client-1',
    ...overrides,
  };
}

test('keeps an assistant bubble streaming until the run emits done', () => {
  const next = mergeAssistantEvent(
    [assistant({ content: 'tool round' })],
    assistant({ id: 'assistant-2', content: '' }),
  );

  expect(next).toHaveLength(1);
  expect(next[0]).toMatchObject({ content: 'tool round', status: 'streaming' });
});

test('keeps failed assistant events failed', () => {
  const next = mergeAssistantEvent(
    [assistant()],
    assistant({ id: 'assistant-error', content: 'request failed', status: 'failed' }),
  );

  expect(next[0]).toMatchObject({ content: 'request failed', status: 'failed' });
});
