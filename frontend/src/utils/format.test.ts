import { describe, expect, it } from 'vitest';

import { formatBytes } from './format';
import { proxyProtocolLabel } from './proxy';

describe('formatBytes', () => {
  it('formats byte sizes with compact units', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(1024 * 1024)).toBe('1 MB');
  });
});

describe('proxyProtocolLabel', () => {
  it('keeps socks5 as SOCKS5 and uppercases other protocols', () => {
    expect(proxyProtocolLabel('socks5')).toBe('SOCKS5');
    expect(proxyProtocolLabel('http')).toBe('HTTP');
  });
});
