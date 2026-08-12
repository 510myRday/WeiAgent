export type RetryReasonKind = 'timeout' | 'rateLimited' | 'upstream' | 'connection' | 'request';

export function classifyRetryReason(reason: string): RetryReasonKind {
  const normalized = reason.toLowerCase();

  // Check explicit upstream status first: 504 contains "timeout", but it is
  // still an upstream gateway failure rather than a client-side timeout.
  if (
    normalized.includes('502')
    || normalized.includes('503')
    || normalized.includes('504')
    || normalized.includes('bad gateway')
    || normalized.includes('service unavailable')
    || normalized.includes('gateway timeout')
    || normalized.includes('upstream')
    || normalized.includes('上游')
  ) {
    return 'upstream';
  }

  if (
    normalized.includes('429')
    || normalized.includes('rate limit')
    || normalized.includes('too many requests')
    || normalized.includes('throttl')
    || normalized.includes('限流')
  ) {
    return 'rateLimited';
  }

  if (
    normalized.includes('timeout')
    || normalized.includes('timed out')
    || normalized.includes('deadline exceeded')
    || normalized.includes('超时')
  ) {
    return 'timeout';
  }

  if (
    normalized.includes('connection')
    || normalized.includes('network')
    || normalized.includes('socket')
    || normalized.includes('connection reset')
    || normalized.includes('broken pipe')
    || normalized.includes('incomplete message')
    || normalized.includes('连接')
    || normalized.includes('网络')
  ) {
    return 'connection';
  }

  return 'request';
}
