import {
  appendReasoningDeltaToAssistant,
  appendTextDeltaToAssistant,
  mergeAssistantEvent,
  toUiMessageItems,
} from './conversationMessages';
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

test('does not duplicate streamed reasoning when the completed assistant event arrives', () => {
  const next = mergeAssistantEvent(
    [assistant({
      content: 'reply 1',
      reasoning_content: 'thinking 1',
      blocks: [
        { type: 'text', content: 'reply 1' },
        { type: 'reasoning', content: 'thinking 1' },
      ],
    })],
    assistant({ content: 'reply 1', reasoning_content: 'thinking 1' }),
  );

  expect(next[0]).toMatchObject({ reasoning_content: 'thinking 1' });
  expect(next[0].blocks?.map((block) => block.type)).toEqual(['text', 'reasoning']);
});

test('keeps failed assistant events failed', () => {
  const next = mergeAssistantEvent(
    [assistant()],
    assistant({ id: 'assistant-error', content: 'request failed', status: 'failed' }),
  );

  expect(next[0]).toMatchObject({ content: 'request failed', status: 'failed' });
});

test('keeps one reasoning block directly before the latest streamed reply', () => {
  let current = assistant({ content: 'reply 1', blocks: [{ type: 'text', content: 'reply 1' }] });
  current = appendReasoningDeltaToAssistant(current, 'thinking 1');
  current = appendTextDeltaToAssistant(current, 'reply 2');
  current = appendReasoningDeltaToAssistant(current, 'thinking 2');
  current = appendTextDeltaToAssistant(current, 'reply 3');

  expect(current.blocks?.map((block) => block.type)).toEqual(['text', 'text', 'reasoning', 'text']);
  expect(current.blocks?.[2]).toMatchObject({ type: 'reasoning', content: 'thinking 1thinking 2' });
});

test('rebuilds loop history with one reasoning block before the latest assistant turn', () => {
  const items = toUiMessageItems([
    {
      ...assistant({ id: 'assistant-1', content: 'reply 1', reasoning_content: 'thinking 1' }),
      tool_calls: [{
        id: 'call-1',
        type: 'function',
        function: { name: 'lookup', arguments: '{}' },
      }],
    },
    assistant({ id: 'assistant-2', content: 'reply 2', reasoning_content: 'thinking 2' }),
  ]);

  expect(items).toHaveLength(1);
  expect(items[0].blocks?.map((block) => block.type)).toEqual([
    'text',
    'tool',
    'reasoning',
    'text',
  ]);
});
