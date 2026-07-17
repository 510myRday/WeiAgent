import { vi } from 'vitest';

import { createAbortSignal } from './abort';

test('createAbortSignal forwards a parent abort', () => {
  const parent = new AbortController();
  const handle = createAbortSignal(1000, parent.signal);
  parent.abort();
  expect(handle.signal.aborted).toBe(true);
  handle.dispose();
});

test('createAbortSignal aborts after the timeout', () => {
  vi.useFakeTimers();
  const handle = createAbortSignal(500);
  vi.advanceTimersByTime(500);
  expect(handle.signal.aborted).toBe(true);
  handle.dispose();
  vi.useRealTimers();
});
