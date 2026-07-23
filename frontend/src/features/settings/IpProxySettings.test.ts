import { describe, expect, it } from 'vitest';

import {
  deriveProxyName,
  findImportAddressIssues,
  findImportNameConflicts,
  parseProxyAddresses,
  reconcileImportRows,
} from './IpProxySettings';

describe('IP proxy batch import helpers', () => {
  it('parses one proxy address per non-empty line', () => {
    expect(parseProxyAddresses(`
      http://127.0.0.1:7890
      socks5://127.0.0.1:1080
    `)).toEqual([
      'http://127.0.0.1:7890',
      'socks5://127.0.0.1:1080',
    ]);
  });

  it('uses the IP or domain and numbers duplicate generated names', () => {
    expect(deriveProxyName('127.0.0.1:7890')).toBe('127.0.0.1');
    expect(deriveProxyName('socks5://proxy.test:1080')).toBe('proxy.test');
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
