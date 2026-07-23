import { describe, expect, it } from 'vitest';

import {
  deriveProxyName,
  findImportAddressIssues,
  findImportNameConflicts,
  parseProxyAddresses,
  reconcileImportRows,
  sortIpProxies,
} from './IpProxySettings';
import type { IpProxy } from '../../types';

describe('IP proxy batch import helpers', () => {
  it('parses one proxy address per non-empty line', () => {
    expect(parseProxyAddresses(`
      http://127.0.0.1:7890
      socks4://127.0.0.1:1080
      socks5://127.0.0.1:1080
    `)).toEqual([
      'http://127.0.0.1:7890',
      'socks4://127.0.0.1:1080',
      'socks5://127.0.0.1:1080',
    ]);
  });

  it('uses the IP or domain and numbers duplicate generated names', () => {
    expect(deriveProxyName('127.0.0.1:7890')).toBe('127.0.0.1');
    expect(deriveProxyName('socks4://proxy.test:1080')).toBe('proxy.test');
    const rows = reconcileImportRows(
      'http://127.0.0.1:7890\nsocks5://proxy.test:1080\nhttps://proxy.test:8443',
      [],
      ['proxy.test'],
    );
    expect(rows.map((row) => row.name)).toEqual([
      '127.0.0.1',
      'proxy.test_1',
      'proxy.test_2',
    ]);
  });

  it('preserves edited names and reports global or batch conflicts', () => {
    const rows = reconcileImportRows('a.test:8000\nb.test:8000');
    const edited = rows.map((row) => ({ ...row, name: 'Shared', customName: true }));
    const reconciled = reconcileImportRows('a.test:8000\nb.test:8000', edited);
    expect(reconciled.map((row) => row.name)).toEqual(['Shared', 'Shared']);
    expect(findImportNameConflicts(reconciled)).toEqual(new Set(reconciled.map((row) => row.key)));
    expect(findImportNameConflicts([rows[0]], ['a.test'])).toEqual(new Set([rows[0].key]));
  });

  it('flags invalid, duplicate, and already configured addresses', () => {
    const rows = reconcileImportRows('127.0.0.1:7890\nhttp://127.0.0.1:7890\nftp://bad.test:21');
    expect(findImportAddressIssues(rows)).toEqual(new Set(rows.map((row) => row.key)));
    const existingRows = reconcileImportRows('proxy.test:8080');
    expect(findImportAddressIssues(existingRows, ['http://proxy.test:8080'])).toEqual(
      new Set([existingRows[0].key]),
    );
  });
});

describe('IP proxy sorting', () => {
  const proxy = (overrides: Partial<IpProxy>): IpProxy => ({
    id: 'id',
    name: 'proxy',
    url: 'http://proxy.test:8080',
    protocol: 'http',
    enabled: true,
    is_pinned: false,
    status: 'available',
    latency_ms: 100,
    last_checked_at: null,
    last_error: null,
    ip_address: null,
    country: null,
    country_code: null,
    region: null,
    city: null,
    api_quality: {
      google: { status: 'available', latency_ms: 100, error: null },
      openai: { status: 'available', latency_ms: 200, error: null },
      anthropic: { status: 'available', latency_ms: 300, error: null },
      grok: { status: 'available', latency_ms: 400, error: null },
    },
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  });

  it('keeps pinned proxies first and sorts quality by average latency', () => {
    const sorted = sortIpProxies([
      proxy({ id: 'slow', name: 'Slow', api_quality: {
        google: { status: 'available', latency_ms: 500, error: null },
        openai: { status: 'available', latency_ms: 600, error: null },
        anthropic: { status: 'available', latency_ms: 700, error: null },
        grok: { status: 'available', latency_ms: 800, error: null },
      } }),
      proxy({ id: 'pinned', name: 'Pinned', is_pinned: true, api_quality: {
        google: { status: 'available', latency_ms: 100, error: null },
        openai: { status: 'available', latency_ms: 100, error: null },
        anthropic: { status: 'available', latency_ms: 100, error: null },
        grok: { status: 'available', latency_ms: 100, error: null },
      } }),
      proxy({ id: 'fast', name: 'Fast', api_quality: {
        google: { status: 'available', latency_ms: 50, error: null },
        openai: { status: 'available', latency_ms: 50, error: null },
        anthropic: { status: 'available', latency_ms: 50, error: null },
        grok: { status: 'available', latency_ms: 50, error: null },
      } }),
    ], 'quality', 'asc');

    expect(sorted.map((item) => item.id)).toEqual(['pinned', 'fast', 'slow']);
  });
});
