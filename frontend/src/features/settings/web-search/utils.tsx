import { CheckCircle2, CircleAlert, CircleX, Clock3 } from 'lucide-react';

import type {
  AgentToolsConfig,
  AgentToolsOverrides,
  WebSearchKeyCheckResult,
  WebSearchKeyStatusRecord,
  WebSearchKeyStatus,
  WebSearchProviderKind,
  WebSearchSourceConfig,
  WebSearchToolConfig,
} from '../../../api';
import type { IpProxy, IpProxyProtocol } from '../../../types';
import { PRESERVE_KEYS_MARKER } from './types';

export function defaultWebSearch(): WebSearchToolConfig {
  return {
    enabled: true,
    max_results: 5,
    timeout_ms: 15000,
    key_selection: 'sequential',
    extraction: {
      remote_enabled: true,
      max_remote_attempts_per_url: 3,
      allow_grok_fallback: true,
    },
    sources: [
      { id: 'tavily', provider: 'tavily', display_name: 'Tavily', enabled: false, deep_enabled: true, extract_enabled: true, priority: 10, independence_group: 'tavily', roles: ['general_search', 'current_news', 'fact_checking', 'market_research', 'product_research'], api_keys: [], proxy_url: null },
      { id: 'exa', provider: 'exa', display_name: 'Exa', enabled: false, deep_enabled: true, extract_enabled: true, priority: 20, independence_group: 'exa', roles: ['semantic_research', 'official_docs', 'technical_research', 'academic_research', 'fact_checking', 'product_research'], api_keys: [], proxy_url: null },
      { id: 'jina', provider: 'jina', display_name: 'Jina Search', enabled: true, deep_enabled: true, extract_enabled: true, priority: 30, independence_group: 'jina', roles: ['general_search', 'semantic_research', 'official_docs', 'technical_research', 'fact_checking', 'multilingual_search'], api_keys: [], proxy_url: null },
      { id: 'grok', provider: 'grok', display_name: 'Grok Research', enabled: false, deep_enabled: true, extract_enabled: true, priority: 40, independence_group: 'grok', roles: ['ai_research', 'current_news', 'social_search', 'fact_checking', 'market_research', 'product_research'], api_keys: [], proxy_url: null },
      { id: 'firecrawl', provider: 'firecrawl', display_name: 'Firecrawl', enabled: false, deep_enabled: true, extract_enabled: true, priority: 50, independence_group: 'firecrawl', roles: ['general_search', 'current_news', 'official_docs', 'technical_research', 'academic_research', 'fact_checking', 'market_research', 'product_research', 'multilingual_search'], api_keys: [], proxy_url: null },
    ],
  };

}

/**
 * Fill the optional fields the backend omits (`skip_serializing_if`) and clamp
 * `enabled` to sources that actually have usable keys. All other fields are
 * always serialized by the backend and trusted as-is.
 */
export function normalizeWebSearchSources(sources: WebSearchSourceConfig[]): WebSearchSourceConfig[] {
  return sources.map((source) => {
    const normalized = {
      ...source,
      api_keys: source.api_keys ?? [],
      key_statuses: source.key_statuses ?? [],
      extract_key_statuses: source.extract_key_statuses ?? [],
      proxy_url: source.proxy_url ?? null,
    };
    return {
      ...normalized,
      enabled: normalized.enabled && sourceCanBeEnabled(normalized),
    };
  });
}

export function sourceCanBeEnabled(source: WebSearchSourceConfig): boolean {
  if (source.provider === 'jina') return true;
  return source.api_keys.some((key, index) => (
    key.trim().length > 0 && source.key_statuses?.[index]?.status !== 'officially_blocked'
  ));
}

export function withWebSearch(config: AgentToolsConfig): AgentToolsConfig {
  const backendSources = config.web_search?.sources ?? [];
  const backendIds = new Set(backendSources.map((s) => s.id));
  const missingDefaults = defaultWebSearch().sources.filter(
    (defaultSource) => !backendIds.has(defaultSource.id),
  );
  const sources = normalizeWebSearchSources(backendSources.concat(missingDefaults));
  return {
    ...config,
    evidence_storage: {
      enabled: config.evidence_storage?.enabled ?? true,
      retention_days: config.evidence_storage?.retention_days ?? 30,
      max_total_mb: config.evidence_storage?.max_total_mb ?? 200,
    },
    web_search: {
      ...defaultWebSearch(),
      ...config.web_search,
      extraction: {
        ...defaultWebSearch().extraction,
        ...config.web_search?.extraction,
      },
      sources,
    },
  };
}

export function serializeDefaultsForAutosave(
  config: AgentToolsConfig,
  keysDirty: Record<string, boolean>,
): { serialized: string; payload: AgentToolsConfig } {
  const payload: AgentToolsConfig = {
    ...config,
    web_search: {
      ...config.web_search,
      sources: config.web_search.sources.map((source) => {
        if (!keysDirty[source.id]) {
          return { ...source, api_keys: [PRESERVE_KEYS_MARKER] };
        }
        return source;
      }),
    },
  };
  return {
    serialized: JSON.stringify(payload),
    payload,
  };
}

export function mergeToolsConfig(defaults: AgentToolsConfig, overrides: AgentToolsOverrides): AgentToolsConfig {
  return {
    max_llm_iterations: overrides.max_llm_iterations ?? defaults.max_llm_iterations,
    evidence_storage: defaults.evidence_storage,
    terminal: {
      enabled: overrides.terminal.enabled ?? defaults.terminal.enabled,
      permission_mode: overrides.terminal.permission_mode ?? defaults.terminal.permission_mode,
      approval_grants: overrides.terminal.approval_grants,
      default_timeout_ms: overrides.terminal.default_timeout_ms ?? defaults.terminal.default_timeout_ms,
      allowed_cwd_roots: overrides.terminal.allowed_cwd_roots ?? defaults.terminal.allowed_cwd_roots,
    },
    web_search: {
      ...defaults.web_search,
      enabled: overrides.web_search?.enabled ?? defaults.web_search.enabled,
      max_results: overrides.web_search?.max_results ?? defaults.web_search.max_results,
    },
    task_planning: {
      enabled: overrides.task_planning?.enabled ?? defaults.task_planning.enabled,
      progress_interval:
        overrides.task_planning?.progress_interval ?? defaults.task_planning.progress_interval ?? 3,
    },
  };
}

export function providerLabel(provider: WebSearchProviderKind, t: (key: string) => string) {
  return t(`tools.webSearch.provider.${provider}`);
}

export function hydrateKeyStatuses(sources: WebSearchSourceConfig[]): Record<string, Record<number, WebSearchKeyCheckResult>> {
  return sources.reduce<Record<string, Record<number, WebSearchKeyCheckResult>>>((result, source) => {
    const statuses = (source.key_statuses ?? []).reduce<Record<number, WebSearchKeyCheckResult>>((byIndex, status, keyIndex) => {
      if (status) {
        byIndex[keyIndex] = {
          key_index: keyIndex,
          key_hint: source.api_keys[keyIndex] ?? '',
          status: status.status,
          detail: status.detail,
          latency_ms: 0,
          checked_at: status.checked_at,
        };
      }
      return byIndex;
    }, {});
    result[source.id] = statuses;
    return result;
  }, {});
}

export function keyStatusRank(status?: WebSearchKeyStatus): number {
  switch (status) {
    case 'normal': return 0;
    case undefined: return 1;
    case 'rate_limited': return 2;
    case 'quota_limited': return 3;
    case 'quota_exhausted': return 4;
    case 'permission_denied': return 5;
    case 'anti_bot': return 6;
    case 'network_error': return 7;
    case 'service_error': return 8;
    case 'configuration_error': return 9;
    case 'error': return 10;
    case 'unauthorized': return 11;
    case 'invalid_key': return 12;
    case 'officially_blocked': return 13;
    default: return 14;
  }
}

export function mergeCheckedStatuses(
  source: WebSearchSourceConfig,
  results: WebSearchKeyCheckResult[],
): WebSearchSourceConfig {
  const statuses: Array<WebSearchKeyStatusRecord | null> = [...(source.key_statuses ?? [])];
  while (statuses.length < source.api_keys.length) statuses.push(null);
  results.forEach((result) => {
    statuses[result.key_index] = {
      status: result.status,
      detail: result.detail,
      checked_at: result.checked_at,
    };
  });
  statuses.length = source.api_keys.length;
  return { ...source, key_statuses: statuses };
}

export function keyStatusLabel(status: WebSearchKeyStatus, t: (key: string) => string) {
  return t(`tools.webSearch.keyStatus.${status}`);
}

export function keyStatusIcon(status: WebSearchKeyStatus) {
  if (status === 'normal') return <CheckCircle2 size={14} />;
  if (status === 'officially_blocked' || status === 'unauthorized' || status === 'invalid_key') return <CircleX size={14} />;
  if (status === 'quota_limited' || status === 'quota_exhausted' || status === 'rate_limited' || status === 'permission_denied' || status === 'anti_bot') return <CircleAlert size={14} />;
  if (status === 'network_error') return <Clock3 size={14} />;
  return <CircleAlert size={14} />;
}

export function formatCheckedAt(value: string, locale: 'zh' | 'en'): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

export function summarizeKeyStatuses(
  keyCount: number,
  statuses: Record<number, WebSearchKeyCheckResult>,
) {
  let healthy = 0;
  let issues = 0;
  let checked = 0;
  Object.entries(statuses).forEach(([keyIndex, status]) => {
    if (Number(keyIndex) >= keyCount) return;
    checked += 1;
    if (status.status === 'normal') healthy += 1;
    else issues += 1;
  });
  return {
    healthy,
    issues,
    unchecked: Math.max(0, keyCount - checked),
  };
}

export function displayProxyUrl(value: string) {
  try {
    const url = new URL(value);
    if (url.password) url.password = '****';
    return url.toString().replace(/\/$/, '');
  } catch {
    return value;
  }
}

export function formatProxyLocation(proxy: IpProxy) {
  const location = [proxy.country, proxy.region, proxy.city].filter(Boolean).join(' · ');
  return location || proxy.ip_address || '--';
}

export function proxyProtocolLabel(protocol: IpProxyProtocol) {
  return protocol === 'socks5' ? 'SOCKS5' : protocol.toUpperCase();
}
