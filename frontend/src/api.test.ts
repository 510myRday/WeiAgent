import { afterEach, expect, test, vi } from 'vitest';

import { streamRunEvents } from './api';
import type { StreamEvent } from './types';

afterEach(() => {
  vi.unstubAllGlobals();
});

test('streamRunEvents preserves blank lines in JSON encoded text deltas', async () => {
  const markdown = [
    '> Solve:',
    '>',
    '> $$',
    String.raw`\begin{cases}`,
    String.raw`3x + 2y = 13 \\`,
    String.raw`2x - 3y = 5`,
    String.raw`\end{cases}`,
    '$$',
    '',
    '---',
    '',
    '## Next problem',
  ].join('\n');
  const payload = [
    'id: 1',
    'event: delta',
    `data: ${JSON.stringify({ text: markdown })}`,
    '',
    'id: 2',
    'event: done',
    'data: ok',
    '',
    '',
  ].join('\n');
  const bytes = new TextEncoder().encode(payload);
  const splitAt = Math.floor(bytes.length / 2);
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes.slice(0, splitAt));
      controller.enqueue(bytes.slice(splitAt));
      controller.close();
    },
  });
  vi.stubGlobal('fetch', vi.fn(async () => ({
    ok: true,
    body,
    statusText: 'OK',
  } as Response)));
  const events: StreamEvent[] = [];

  await streamRunEvents('run-1', 0, (event) => events.push(event));

  expect(events[0]).toEqual({
    event: 'delta',
    data: markdown,
    sequence: 1,
  });
  expect(events[1]).toEqual({
    event: 'done',
    data: 'ok',
    sequence: 2,
  });
});

test('streamRunEvents still accepts legacy multiline text deltas', async () => {
  const payload = [
    'event: delta',
    'data: first line',
    'data: ',
    'data: third line',
    '',
    'event: done',
    'data: ok',
    '',
    '',
  ].join('\n');
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode(payload));
      controller.close();
    },
  });
  vi.stubGlobal('fetch', vi.fn(async () => ({
    ok: true,
    body,
    statusText: 'OK',
  } as Response)));
  const events: StreamEvent[] = [];

  await streamRunEvents('run-1', 0, (event) => events.push(event));

  expect(events[0]).toMatchObject({
    event: 'delta',
    data: 'first line\n\nthird line',
  });
});
