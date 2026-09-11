import {
  appendReasoningDeltaToAssistant,
  appendTextDeltaToAssistant,
  mergeAssistantEvent,
  upsertToolOnMessages,
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

test('keeps the assistant reply after a persisted compaction marker visible', () => {
  const marker = assistant({
    id: 'compaction-1',
    content: '',
    system_event: {
      type: 'context_compacted',
      source: 'automatic',
      threshold_percent: 90,
      before_tokens: 9700,
      after_tokens: 1300,
      created_at: '2026-08-15T00:01:00.000Z',
    },
  });
  const items = toUiMessageItems([
    assistant({ id: 'assistant-before', content: 'before compaction' }),
    marker,
    assistant({ id: 'assistant-after', content: 'after compaction' }),
  ]);

  expect(items).toHaveLength(3);
  expect(items[1]).toMatchObject({ system_event: marker.system_event, content: '' });
  expect(items[2]).toMatchObject({ content: 'after compaction' });
});

test('starts a new assistant bubble after a live compaction event', () => {
  const marker = assistant({
    id: 'compaction-1',
    content: '',
    system_event: {
      type: 'context_compacted',
      source: 'automatic',
      threshold_percent: 90,
      before_tokens: 9700,
      after_tokens: 1300,
      created_at: '2026-08-15T00:01:00.000Z',
    },
  });
  const items = mergeAssistantEvent([marker], assistant({ id: 'assistant-after', content: 'after compaction' }));

  expect(items).toHaveLength(2);
  expect(items[0]).toMatchObject({ system_event: marker.system_event });
  expect(items[1]).toMatchObject({ content: 'after compaction' });
});

test('does not attach a post-compaction tool call to the separator', () => {
  const marker = assistant({
    id: 'compaction-1',
    content: '',
    system_event: {
      type: 'context_compacted',
      source: 'automatic',
      threshold_percent: 90,
      before_tokens: 9700,
      after_tokens: 1300,
      created_at: '2026-08-15T00:01:00.000Z',
    },
  });
  const items = upsertToolOnMessages(
    [marker],
    {
      call_id: 'call-after-compaction',
      name: 'lookup',
      arguments: {},
      status: 'running',
    },
    'agent-1',
    'conversation-1',
  );

  expect(items).toHaveLength(2);
  expect(items[0]).toMatchObject({ system_event: marker.system_event });
  expect(items[1]).toMatchObject({ role: 'assistant', content: '' });
  expect(items[1]).not.toMatchObject({ system_event: expect.anything() });
  expect(items[1].role === 'assistant' && items[1].blocks?.[0]).toMatchObject({
    type: 'tool',
    tool: { call_id: 'call-after-compaction' },
  });
});
