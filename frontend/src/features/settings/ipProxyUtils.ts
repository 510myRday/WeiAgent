import type { IpProxy } from '../../types';

export type ProxySortKey = 'latency' | 'quality' | 'enabled';
export type SortDirection = 'asc' | 'desc';
export type ImportProxyRow = {
  key: string;
  address: string;
  name: string;
  customName: boolean;
};

export function parseProxyAddresses(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function deriveProxyName(address: string): string {
  const trimmed = address.trim();
  const candidate = trimmed.includes('://') ? trimmed : `http://${trimmed}`;
  try {
    const hostname = new URL(candidate).hostname
      .replace(/^\[/, '')
      .replace(/\]$/, '')
      .trim();
    return hostname || 'proxy';
  } catch {
    return 'proxy';
  }
}

export function reconcileImportRows(
  value: string,
  currentRows: ImportProxyRow[] = [],
  existingNames: string[] = [],
): ImportProxyRow[] {
  const occurrences = new Map<string, number>();
  const descriptors = parseProxyAddresses(value).map((address) => {
    const occurrence = (occurrences.get(address) ?? 0) + 1;
    occurrences.set(address, occurrence);
    return { address, key: `${occurrence}:${address}` };
  });
  const currentByKey = new Map(currentRows.map((row) => [row.key, row]));
  const usedNames = new Set(existingNames.map((name) => name.trim().toLocaleLowerCase()).filter(Boolean));

  for (const descriptor of descriptors) {
    const current = currentByKey.get(descriptor.key);
    if (current?.customName && current.name.trim()) {
      usedNames.add(current.name.trim().toLocaleLowerCase());
    }
  }

  return descriptors.map((descriptor) => {
    const current = currentByKey.get(descriptor.key);
    if (current?.customName) {
      return { ...current, address: descriptor.address };
    }
    return {
      ...descriptor,
      name: uniqueProxyName(deriveProxyName(descriptor.address), usedNames),
      customName: false,
    };
  });
}

export function findImportNameConflicts(
  rows: ImportProxyRow[],
  existingNames: string[] = [],
): Set<string> {
  const conflicts = new Set<string>();
  const existing = new Set(existingNames.map((name) => name.trim().toLocaleLowerCase()).filter(Boolean));
  const rowsByName = new Map<string, ImportProxyRow[]>();

  for (const row of rows) {
    const normalized = row.name.trim().toLocaleLowerCase();
    if (!normalized) {
      conflicts.add(row.key);
      continue;
    }
    const matching = rowsByName.get(normalized) ?? [];
    matching.push(row);
    rowsByName.set(normalized, matching);
    if (existing.has(normalized)) conflicts.add(row.key);
  }

  for (const matching of rowsByName.values()) {
    if (matching.length > 1) matching.forEach((row) => conflicts.add(row.key));
  }
  return conflicts;
}

export function findImportAddressIssues(
  rows: ImportProxyRow[],
  existingUrls: string[] = [],
): Set<string> {
  const issues = new Set<string>();
  const existing = new Set(existingUrls.map(normalizeProxyAddress).filter((value): value is string => Boolean(value)));
  const rowsByUrl = new Map<string, ImportProxyRow[]>();

  for (const row of rows) {
    const normalized = normalizeProxyAddress(row.address);
    if (!normalized) {
      issues.add(row.key);
      continue;
    }
    const matching = rowsByUrl.get(normalized) ?? [];
    matching.push(row);
    rowsByUrl.set(normalized, matching);
    if (existing.has(normalized)) issues.add(row.key);
  }

  for (const matching of rowsByUrl.values()) {
    if (matching.length > 1) matching.forEach((row) => issues.add(row.key));
  }
  return issues;
}

function normalizeProxyAddress(address: string): string | null {
  const trimmed = address.trim();
  if (!trimmed) return null;
  const withNormalizedSocks = trimmed.replace(/^socks:\/\//i, 'socks5://');
  const candidate = withNormalizedSocks.includes('://')
    ? withNormalizedSocks
    : `http://${withNormalizedSocks}`;
  try {
    const url = new URL(candidate);
    if (!['http:', 'https:', 'socks4:', 'socks4a:', 'socks5:', 'socks5h:'].includes(url.protocol)) return null;
    if (!url.hostname || url.search || url.hash || !['', '/'].includes(url.pathname)) return null;
    return url.toString().replace(/\/$/, '');
  } catch {
    return null;
  }
}

function uniqueProxyName(baseName: string, usedNames: Set<string>): string {
  const normalizedBase = baseName.trim() || 'proxy';
  if (!usedNames.has(normalizedBase.toLocaleLowerCase())) {
    usedNames.add(normalizedBase.toLocaleLowerCase());
    return normalizedBase;
  }
  for (let index = 1; ; index += 1) {
    const candidate = `${normalizedBase}_${index}`;
    if (!usedNames.has(candidate.toLocaleLowerCase())) {
      usedNames.add(candidate.toLocaleLowerCase());
      return candidate;
    }
  }
}

export function sortIpProxies(proxies: IpProxy[], key: ProxySortKey, direction: SortDirection) {
  const multiplier = direction === 'asc' ? 1 : -1;
  return [...proxies].sort((left, right) => {
    const pinOrder = Number(right.is_pinned) - Number(left.is_pinned);
    if (pinOrder !== 0) return pinOrder;

    const result = compareProxyValues(left, right, key);
    if (result !== 0) {
      const leftEmpty = key === 'quality' ? qualityAverage(left) == null : left.latency_ms == null;
      const rightEmpty = key === 'quality' ? qualityAverage(right) == null : right.latency_ms == null;
      const keepEmptyValueLast = (key === 'latency' || key === 'quality') && (leftEmpty || rightEmpty);
      return keepEmptyValueLast ? result : result * multiplier;
    }
    return left.created_at.localeCompare(right.created_at) || left.id.localeCompare(right.id);
  });
}

function compareProxyValues(left: IpProxy, right: IpProxy, key: ProxySortKey) {
  if (key === 'latency') {
    if (left.latency_ms == null && right.latency_ms == null) return 0;
    if (left.latency_ms == null) return 1;
    if (right.latency_ms == null) return -1;
    return left.latency_ms - right.latency_ms;
  }
  if (key === 'quality') {
    const leftAverage = qualityAverage(left);
    const rightAverage = qualityAverage(right);
    if (leftAverage == null && rightAverage == null) return 0;
    if (leftAverage == null) return 1;
    if (rightAverage == null) return -1;
    return leftAverage - rightAverage;
  }
  if (key === 'enabled') return Number(left.enabled) - Number(right.enabled);
  return 0;
}

function qualityAverage(proxy: IpProxy): number | null {
  const latencies = Object.values(proxy.api_quality ?? {})
    .map((probe) => probe.latency_ms)
    .filter((latency): latency is number => latency != null);
  if (latencies.length === 0) return null;
  return latencies.reduce((total, latency) => total + latency, 0) / latencies.length;
}

