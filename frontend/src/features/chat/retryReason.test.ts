import { describe, expect, test } from 'vitest';

import { classifyRetryReason } from './retryReason';

describe('classifyRetryReason', () => {
  test('keeps 502 upstream errors out of the timeout category', () => {
    expect(classifyRetryReason('upstream returned 502 Bad Gateway: error code: 502')).toBe('upstream');
  });

  test('keeps 504 gateway timeout as an upstream error', () => {
    expect(classifyRetryReason('upstream returned 504 Gateway Timeout')).toBe('upstream');
  });

  test('recognizes a client-side timeout', () => {
    expect(classifyRetryReason('request idle timeout while waiting for response')).toBe('timeout');
  });
});
