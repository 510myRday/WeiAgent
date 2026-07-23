import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  CheckCircle2,
  ChevronDown,
  CircleAlert,
  CircleX,
  Clock3,
  KeyRound,
  LoaderCircle,
  Pause,
  Plus,
  Play,
  RefreshCw,
  ScanSearch,
  Settings2,
  Trash2,
} from 'lucide-react';

import type {
  AgentToolsConfig,
  AgentToolsOverrides,
  AgentToolsSettings,
  WebSearchKeyCheckResult,
  WebSearchKeyStatusRecord,
  WebSearchKeyStatus,
  WebSearchKeySelection,
  WebSearchProviderKind,
  WebSearchSourceRole,
  WebSearchSourceConfig,
  WebSearchToolConfig,
} from '../../api';
import {
  readAgentToolsConfig,
  MAX_WEB_SEARCH_KEYS_PER_SOURCE,
  checkWebSearchKeys,
  deleteWebSearchKey,
  importWebSearchKeys,
  readToolDefaults,
  updateAgentToolsConfig,
  updateToolDefaults,
} from '../../api';
import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { useBeforeUnloadGuard } from '../../hooks';
import { AutoSaveIndicator, Button, Dialog, IconButton, useFeedback } from '../../components/ui';

interface WebSearchToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
  onBack?: () => void;
}

type AutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

type SourceSection = 'preferences' | 'keys' | 'connection';
type ExpandedSourceSections = Record<string, Partial<Record<SourceSection, boolean>>>;
type KeyCheckProgress = {
  total: number;
  completed: number;
  healthy: number;
  issues: number;
  activeKeyIndex: number | null;
  paused: boolean;
  pauseRequested: boolean;
};

type PendingSave =
  | {
    contextKey: string;
    serialized: string;
    scope: 'defaults';
    config: AgentToolsConfig;
  }
  | {
    contextKey: string;
    serialized: string;
    scope: 'agent';
    agentId: string;
    overrides: AgentToolsOverrides;
  };

const AUTO_SAVE_DELAY_MS = 600;
const AUTO_SAVE_RETRY_DELAY_MS = 1200;
const AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;
const MAX_RESULTS = 20;
const MAX_EVIDENCE_RETENTION_DAYS = 3650;
const MAX_EVIDENCE_TOTAL_MB = 10240;
const PRESERVE_KEYS_MARKER = '__preserve__';
const SOURCE_ROLE_GROUPS: Array<{
  key: 'discovery' | 'content' | 'intent';
  roles: WebSearchSourceRole[];
}> = [
  {
    key: 'discovery',
    roles: ['general_search', 'semantic_research', 'multilingual_search', 'ai_research'],
  },
  {
    key: 'content',
    roles: ['current_news', 'official_docs', 'technical_research', 'academic_research', 'social_search'],
  },
  {
    key: 'intent',
    roles: ['fact_checking', 'market_research', 'product_research'],
  },
];

function defaultWebSearch(): WebSearchToolConfig {
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
      { id: 'tavily', provider: 'tavily', display_name: 'Tavily', enabled: false, deep_enabled: true, extract_enabled: true, priority: 10, independence_group: 'tavily', roles: ['general_search', 'current_news', 'fact_checking', 'market_research', 'product_research'], api_keys: [], extract_base_url: null, flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'exa', provider: 'exa', display_name: 'Exa', enabled: false, deep_enabled: true, extract_enabled: true, priority: 20, independence_group: 'exa', roles: ['semantic_research', 'official_docs', 'technical_research', 'academic_research', 'fact_checking', 'product_research'], api_keys: [], extract_base_url: null, flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'jina', provider: 'jina', display_name: 'Jina Search', enabled: true, deep_enabled: true, extract_enabled: true, priority: 30, independence_group: 'jina', roles: ['general_search', 'semantic_research', 'official_docs', 'technical_research', 'fact_checking', 'multilingual_search'], api_keys: [], extract_base_url: null, flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'grok', provider: 'grok', display_name: 'Grok Research', enabled: false, deep_enabled: true, extract_enabled: true, priority: 40, independence_group: 'grok', roles: ['ai_research', 'current_news', 'social_search', 'fact_checking', 'market_research', 'product_research'], api_keys: [], extract_base_url: null, flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'firecrawl', provider: 'firecrawl', display_name: 'Firecrawl', enabled: false, deep_enabled: true, extract_enabled: true, priority: 50, independence_group: 'firecrawl', roles: ['general_search', 'current_news', 'official_docs', 'technical_research', 'academic_research', 'fact_checking', 'market_research', 'product_research', 'multilingual_search'], api_keys: [], extract_base_url: null, flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
    ],
  };
}

function normalizeWebSearchSources(sources: WebSearchSourceConfig[]): WebSearchSourceConfig[] {
  return sources.map((source, index) => {
    const normalized = {
      ...source,
      display_name: providerLabelFallback(source.provider),
      deep_enabled: source.deep_enabled ?? true,
      extract_enabled: source.extract_enabled ?? true,
      priority: (index + 1) * 10,
      independence_group: source.provider,
      roles: source.roles?.length ? source.roles : defaultRoles(source.provider),
      api_keys: source.api_keys ?? [],
      key_statuses: source.key_statuses ?? [],
      extract_key_statuses: source.extract_key_statuses ?? [],
      extract_base_url: source.extract_base_url ?? null,
      flaresolverr_url: source.flaresolverr_url ?? null,
      flaresolverr_proxy_url: source.flaresolverr_proxy_url ?? null,
      proxy_url: source.proxy_url ?? null,
    };
    return {
      ...normalized,
      enabled: normalized.enabled && sourceCanBeEnabled(normalized),
    };
  });
}

function sourceCanBeEnabled(source: WebSearchSourceConfig): boolean {
  if (source.provider === 'jina') return true;
  return source.api_keys.some((key, index) => (
    key.trim().length > 0 && source.key_statuses?.[index]?.status !== 'officially_blocked'
  ));
}

function providerLabelFallback(provider: WebSearchProviderKind): string {
  if (provider === 'jina') return 'Jina Search';
  if (provider === 'grok') return 'Grok Research';
  if (provider === 'firecrawl') return 'Firecrawl';
  return provider[0].toUpperCase() + provider.slice(1);
}

function defaultRoles(provider: WebSearchProviderKind): WebSearchSourceRole[] {
  if (provider === 'tavily') {
    return ['general_search', 'current_news', 'fact_checking', 'market_research', 'product_research'];
  }
  if (provider === 'exa') {
    return ['semantic_research', 'official_docs', 'technical_research', 'academic_research', 'fact_checking', 'product_research'];
  }
  if (provider === 'jina') {
    return ['general_search', 'semantic_research', 'official_docs', 'technical_research', 'fact_checking', 'multilingual_search'];
  }
  if (provider === 'firecrawl') {
    return ['general_search', 'current_news', 'official_docs', 'technical_research', 'academic_research', 'fact_checking', 'market_research', 'product_research', 'multilingual_search'];
  }
  return ['ai_research', 'current_news', 'social_search', 'fact_checking', 'market_research', 'product_research'];
}

function withWebSearch(config: AgentToolsConfig): AgentToolsConfig {
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

function serializeDefaultsForAutosave(
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

function mergeToolsConfig(defaults: AgentToolsConfig, overrides: AgentToolsOverrides): AgentToolsConfig {
  return {
    max_llm_iterations: overrides.max_llm_iterations ?? defaults.max_llm_iterations,
    evidence_storage: defaults.evidence_storage,
    terminal: {
      enabled: overrides.terminal.enabled ?? defaults.terminal.enabled,
      permission_mode: overrides.terminal.permission_mode ?? defaults.terminal.permission_mode,
      approval_grants: overrides.terminal.approval_grants ?? [],
      default_timeout_ms: overrides.terminal.default_timeout_ms ?? defaults.terminal.default_timeout_ms,
      allowed_cwd_roots: overrides.terminal.allowed_cwd_roots ?? defaults.terminal.allowed_cwd_roots,
    },
    web_search: {
      ...defaults.web_search,
      enabled: overrides.web_search?.enabled ?? defaults.web_search.enabled,
      max_results: overrides.web_search?.max_results ?? defaults.web_search.max_results,
    },
  };
}

function providerLabel(provider: WebSearchProviderKind, t: (key: string) => string) {
  return t(`tools.webSearch.provider.${provider}`);
}

function hydrateKeyStatuses(sources: WebSearchSourceConfig[]): Record<string, Record<number, WebSearchKeyCheckResult>> {
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

function keyStatusRank(status?: WebSearchKeyStatus): number {
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

function mergeCheckedStatuses(
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

function keyStatusLabel(status: WebSearchKeyStatus, t: (key: string) => string) {
  return t(`tools.webSearch.keyStatus.${status}`);
}

function keyStatusIcon(status: WebSearchKeyStatus) {
  if (status === 'normal') return <CheckCircle2 size={14} />;
  if (status === 'officially_blocked' || status === 'unauthorized' || status === 'invalid_key') return <CircleX size={14} />;
  if (status === 'quota_limited' || status === 'quota_exhausted' || status === 'rate_limited' || status === 'permission_denied' || status === 'anti_bot') return <CircleAlert size={14} />;
  if (status === 'network_error') return <Clock3 size={14} />;
  return <CircleAlert size={14} />;
}

function formatCheckedAt(value: string): string {
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? value : new Date(timestamp).toLocaleString();
}

function defaultExpandedSources(): Record<string, boolean> {
  return {};
}

function defaultExpandedSourceSections(): ExpandedSourceSections {
  return {};
}

function summarizeKeyStatuses(
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

export function WebSearchToolSettings({
  scope,
  agentId,
  onBack,
}: WebSearchToolSettingsProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const contextKey = scope === 'defaults' ? 'defaults' : `agent:${agentId ?? ''}`;
  const [defaults, setDefaults] = useState<AgentToolsConfig | null>(null);
  const [agentSettings, setAgentSettings] = useState<AgentToolsSettings | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [configLoadError, setConfigLoadError] = useState('');
  const [loadRequestKey, setLoadRequestKey] = useState(0);
  const [autoSaveStatus, setAutoSaveStatus] = useState<AutoSaveStatus>('idle');
  const [draftKeys, setDraftKeys] = useState<Record<string, string>>({});
  const [keysDirty, setKeysDirty] = useState<Record<string, boolean>>({});
  const [selectedKeyIndices, setSelectedKeyIndices] = useState<Record<string, number[]>>({});
  const [expandedSources, setExpandedSources] = useState<Record<string, boolean>>({});
  const [expandedSourceSections, setExpandedSourceSections] = useState<ExpandedSourceSections>({});
  const [keyStatuses, setKeyStatuses] = useState<Record<string, Record<number, WebSearchKeyCheckResult>>>({});
  const [checkingKeys, setCheckingKeys] = useState<Record<string, boolean>>({});
  const [checkProgress, setCheckProgress] = useState<Record<string, KeyCheckProgress>>({});
  const [importingKeys, setImportingKeys] = useState<Record<string, boolean>>({});
  const [deletingKeys, setDeletingKeys] = useState<Record<string, boolean>>({});
  const [addKeySourceId, setAddKeySourceId] = useState<string | null>(null);

  const isMountedRef = useRef(true);
  const activeContextRef = useRef(contextKey);
  const loadedContextRef = useRef('');
  const latestSerializedRef = useRef(new Map<string, string>());
  const persistedSerializedRef = useRef(new Map<string, string>());
  const pendingSavesRef = useRef(new Map<string, PendingSave>());
  const saveInFlightRef = useRef(false);
  const debounceTimerRef = useRef<number | undefined>(undefined);
  const debounceContextRef = useRef('');
  const retryTimerRef = useRef<number | undefined>(undefined);
  const retryCountsRef = useRef(new Map<string, number>());
  const checkPendingIndicesRef = useRef<Record<string, number[]>>({});
  const checkPauseRequestedRef = useRef<Record<string, boolean>>({});

  const config = useMemo(() => {
    if (scope === 'defaults') return defaults ? withWebSearch(defaults) : null;
    if (!agentSettings) return null;
    return withWebSearch(agentSettings.effective);
  }, [agentSettings, defaults, scope]);

  const overrides = agentSettings?.overrides ?? null;

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    activeContextRef.current = contextKey;
  }, [contextKey]);

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setConfigLoadError('');
    const load = scope === 'defaults'
      ? readToolDefaults().then((next) => {
        if (cancelled) return;
        const normalized = withWebSearch(next);
        setDefaults(normalized);
        setAgentSettings(null);
        setDraftKeys({});
        setAddKeySourceId(null);
        setKeysDirty({});
        setSelectedKeyIndices({});
        setExpandedSources(defaultExpandedSources());
        setExpandedSourceSections(defaultExpandedSourceSections());
        setKeyStatuses(hydrateKeyStatuses(normalized.web_search.sources));
        const serialized = serializeDefaultsForAutosave(normalized, {}).serialized;
        latestSerializedRef.current.set(contextKey, serialized);
        persistedSerializedRef.current.set(contextKey, serialized);
        loadedContextRef.current = contextKey;
      })
      : agentId
        ? readAgentToolsConfig(agentId).then((next) => {
          if (cancelled) return;
          const normalized = {
            ...next,
            defaults: withWebSearch(next.defaults),
            effective: withWebSearch(next.effective),
            overrides: {
              ...next.overrides,
              web_search: next.overrides.web_search ?? {},
            },
          };
          setAgentSettings(normalized);
          setDefaults(normalized.defaults);
          setAddKeySourceId(null);
          setSelectedKeyIndices({});
          setExpandedSources(defaultExpandedSources());
          setExpandedSourceSections(defaultExpandedSourceSections());
          setKeyStatuses(hydrateKeyStatuses(normalized.effective.web_search.sources));
          const serialized = JSON.stringify(normalized.overrides);
          latestSerializedRef.current.set(contextKey, serialized);
          persistedSerializedRef.current.set(contextKey, serialized);
          loadedContextRef.current = contextKey;
        })
        : Promise.reject(new Error(t('tools.webSearch.agentRequired')));

    load
      .catch((error) => {
        if (cancelled) return;
        setConfigLoadError(error instanceof Error ? error.message : t('tools.webSearch.loadFailed'));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [agentId, contextKey, loadRequestKey, scope, t]);

  const flushSaveQueue = useCallback(async () => {
    if (saveInFlightRef.current) return;
    const nextSave = pendingSavesRef.current.values().next().value as PendingSave | undefined;
    if (!nextSave) return;

    pendingSavesRef.current.delete(nextSave.contextKey);
    saveInFlightRef.current = true;
    if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
      setAutoSaveStatus('saving');
    }

    let retryDelay = 0;
    try {
      if (nextSave.scope === 'defaults') {
        const saved = await updateToolDefaults(nextSave.config);
        if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
          setDefaults(withWebSearch(saved));
          setKeyStatuses(hydrateKeyStatuses(saved.web_search.sources));
          setKeysDirty({});
        }
      } else {
        const saved = await updateAgentToolsConfig(nextSave.agentId, nextSave.overrides);
        if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
          setAgentSettings({
            ...saved,
            defaults: withWebSearch(saved.defaults),
            effective: withWebSearch(saved.effective),
          });
          setKeyStatuses(hydrateKeyStatuses(saved.effective.web_search.sources));
        }
      }
      persistedSerializedRef.current.set(nextSave.contextKey, nextSave.serialized);
      retryCountsRef.current.delete(nextSave.contextKey);
      if (
        isMountedRef.current
        && activeContextRef.current === nextSave.contextKey
        && latestSerializedRef.current.get(nextSave.contextKey) === nextSave.serialized
      ) {
        setAutoSaveStatus('saved');
      }
    } catch (error) {
      const attempt = (retryCountsRef.current.get(nextSave.contextKey) ?? 0) + 1;
      retryCountsRef.current.set(nextSave.contextKey, attempt);
      const isLatest = latestSerializedRef.current.get(nextSave.contextKey) === nextSave.serialized;
      if (isLatest) {
        pendingSavesRef.current.set(nextSave.contextKey, nextSave);
        retryDelay = Math.min(
          AUTO_SAVE_MAX_RETRY_DELAY_MS,
          AUTO_SAVE_RETRY_DELAY_MS * (2 ** Math.min(attempt - 1, 5)),
        );
      }
      if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
        setAutoSaveStatus('error');
        if (attempt === 1) {
          toast.error(error instanceof Error ? error.message : t('tools.webSearch.saveFailed'));
        }
      }
    } finally {
      saveInFlightRef.current = false;
      if (pendingSavesRef.current.size > 0) {
        if (retryTimerRef.current !== undefined) {
          window.clearTimeout(retryTimerRef.current);
        }
        retryTimerRef.current = window.setTimeout(() => {
          retryTimerRef.current = undefined;
          void flushSaveQueue();
        }, retryDelay);
      }
    }
  }, [t, toast]);

  useEffect(() => {
    if (isLoading || !config || loadedContextRef.current !== contextKey) return;

    let pendingSave: PendingSave;
    if (scope === 'defaults') {
      const { serialized, payload } = serializeDefaultsForAutosave(config, keysDirty);
      latestSerializedRef.current.set(contextKey, serialized);
      pendingSave = {
        contextKey,
        serialized,
        scope: 'defaults',
        config: payload,
      };
    } else if (overrides && agentId) {
      const serialized = JSON.stringify(overrides);
      latestSerializedRef.current.set(contextKey, serialized);
      pendingSave = {
        contextKey,
        serialized,
        scope: 'agent',
        agentId,
        overrides,
      };
    } else {
      return;
    }

    if (persistedSerializedRef.current.get(contextKey) === pendingSave.serialized) {
      pendingSavesRef.current.delete(contextKey);
      return;
    }

    pendingSavesRef.current.set(contextKey, pendingSave);
    setAutoSaveStatus('pending');
    if (debounceTimerRef.current !== undefined) {
      window.clearTimeout(debounceTimerRef.current);
    }
    debounceContextRef.current = contextKey;
    debounceTimerRef.current = window.setTimeout(() => {
      debounceTimerRef.current = undefined;
      debounceContextRef.current = '';
      void flushSaveQueue();
    }, AUTO_SAVE_DELAY_MS);
  }, [agentId, config, contextKey, flushSaveQueue, isLoading, keysDirty, overrides, scope]);

  useBeforeUnloadGuard(autoSaveStatus === 'pending' || autoSaveStatus === 'saving' || autoSaveStatus === 'error');

  function retryAutoSaveNow() {
    if (debounceTimerRef.current !== undefined) {
      window.clearTimeout(debounceTimerRef.current);
      debounceTimerRef.current = undefined;
    }
    if (retryTimerRef.current !== undefined) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = undefined;
    }
    void flushSaveQueue();
  }

  function updateGlobalConfig(next: AgentToolsConfig) {
    setDefaults(withWebSearch(next));
  }

  function updateOverrides(next: AgentToolsOverrides) {
    setAgentSettings((current) => (current ? {
      ...current,
      overrides: next,
      effective: mergeToolsConfig(current.defaults, next),
    } : current));
  }

  function updateWebSearch(patch: Partial<WebSearchToolConfig>) {
    if (!config) return;
    if (scope === 'defaults') {
      updateGlobalConfig({ ...config, web_search: { ...config.web_search, ...patch } });
      return;
    }
    if (!overrides) return;
    const nextWebSearch = { ...overrides.web_search };
    if (patch.enabled !== undefined) nextWebSearch.enabled = patch.enabled;
    if (patch.max_results !== undefined) nextWebSearch.max_results = patch.max_results;
    updateOverrides({ ...overrides, web_search: nextWebSearch });
  }

  function updateExtraction(patch: Partial<WebSearchToolConfig['extraction']>) {
    updateWebSearch({
      extraction: {
        ...config!.web_search.extraction,
        ...patch,
      },
    });
  }

  function updateEvidenceStorage(patch: Partial<AgentToolsConfig['evidence_storage']>) {
    if (!config || scope !== 'defaults') return;
    updateGlobalConfig({
      ...config,
      evidence_storage: { ...config.evidence_storage, ...patch },
    });
  }

  function setEnabledInherited(inherited: boolean) {
    if (!overrides || !config) return;
    const next = { ...overrides, web_search: { ...overrides.web_search } };
    if (inherited) delete next.web_search.enabled;
    else next.web_search.enabled = config.web_search.enabled;
    updateOverrides(next);
  }

  function setMaxResultsInherited(inherited: boolean) {
    if (!overrides || !config) return;
    const next = { ...overrides, web_search: { ...overrides.web_search } };
    if (inherited) delete next.web_search.max_results;
    else next.web_search.max_results = config.web_search.max_results;
    updateOverrides(next);
  }

  function updateSource(index: number, patch: Partial<WebSearchSourceConfig>) {
    if (!config || scope !== 'defaults') return;
    const sources = config.web_search.sources.map((source, i) => (i === index ? { ...source, ...patch } : source));
    updateWebSearch({ sources });
  }

  function moveSource(index: number, direction: -1 | 1) {
    if (!config || scope !== 'defaults') return;
    const target = index + direction;
    if (target < 0 || target >= config.web_search.sources.length) return;
    const sources = [...config.web_search.sources];
    const [item] = sources.splice(index, 1);
    sources.splice(target, 0, item);
    updateWebSearch({
      sources: sources.map((source, sourceIndex) => ({
        ...source,
        priority: (sourceIndex + 1) * 10,
      })),
    });
  }

  function toggleSourceRole(index: number, role: WebSearchSourceRole, checked: boolean) {
    if (!config || scope !== 'defaults') return;
    const source = config.web_search.sources[index];
    const roles = checked
      ? Array.from(new Set([...source.roles, role]))
      : source.roles.filter((value) => value !== role);
    updateSource(index, { roles });
  }

  async function addKeys(sourceId: string) {
    if (!config || scope !== 'defaults') return;
    const values = (draftKeys[sourceId] ?? '')
      .split(/\r?\n/)
      .map((value) => value.trim())
      .filter(Boolean);
    if (values.length === 0) return;

    const source = config.web_search.sources.find((item) => item.id === sourceId);
    if (!source) return;
    const availableSlots = MAX_WEB_SEARCH_KEYS_PER_SOURCE - source.api_keys.length;
    if (availableSlots <= 0) {
      toast.error(t('tools.webSearch.keyLimitReached', { count: MAX_WEB_SEARCH_KEYS_PER_SOURCE }));
      return;
    }
    const valuesToImport = values.slice(0, availableSlots);

    setImportingKeys((current) => ({ ...current, [sourceId]: true }));
    try {
      const response = await importWebSearchKeys(sourceId, valuesToImport);
      const sources = config.web_search.sources.map((item) => (
        item.id === sourceId ? response.source : item
      ));
      setKeysDirty((current) => ({ ...current, [sourceId]: true }));
      updateWebSearch({ sources });
      setKeyStatuses((current) => ({
        ...current,
        [sourceId]: hydrateKeyStatuses([response.source])[sourceId] ?? {},
      }));
      setDraftKeys((current) => ({ ...current, [sourceId]: '' }));
      const message = t(
        source.provider === 'grok'
          ? 'tools.webSearch.grokKeyImportSummary'
          : 'tools.webSearch.keyImportSummary',
        {
          imported: response.imported,
          duplicates: response.duplicates,
          blocked: response.blocked,
        },
      );
      if (response.imported > 0) toast.success(message);
      else toast.info(message);
      if (valuesToImport.length < values.length) {
        toast.info(t('tools.webSearch.keyLimitReached', { count: MAX_WEB_SEARCH_KEYS_PER_SOURCE }));
      }
      setAddKeySourceId((current) => current === sourceId ? null : current);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.webSearch.saveFailed'));
    } finally {
      setImportingKeys((current) => ({ ...current, [sourceId]: false }));
    }
  }

  function closeAddKeyDialog() {
    if (addKeySourceId) {
      setDraftKeys((current) => ({ ...current, [addKeySourceId]: '' }));
    }
    setAddKeySourceId(null);
  }

  async function removeKey(sourceId: string, keyIndex: number) {
    if (!config || scope !== 'defaults') return;
    const source = config.web_search.sources.find((item) => item.id === sourceId);
    if (!source) return;
    const keyHint = source.api_keys[keyIndex];
    if (keyHint === undefined) return;
    const confirmed = await confirm({
      title: t('tools.webSearch.deleteKeyTitle'),
      message: t('tools.webSearch.deleteKeyConfirm'),
      confirmLabel: t('tools.webSearch.removeKey'),
      tone: 'danger',
    });
    if (!confirmed) return;

    setDeletingKeys((current) => ({ ...current, [sourceId]: true }));
    try {
      const response = await deleteWebSearchKey(sourceId, keyIndex, keyHint);
      const sources = config.web_search.sources.map((item) => (
        item.id === sourceId ? response.source : item
      ));
      setKeysDirty((current) => ({ ...current, [sourceId]: true }));
      setSelectedKeyIndices((current) => ({ ...current, [sourceId]: [] }));
      setKeyStatuses((current) => ({
        ...current,
        [sourceId]: hydrateKeyStatuses([response.source])[sourceId] ?? {},
      }));
      updateWebSearch({ sources });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.webSearch.saveFailed'));
    } finally {
      setDeletingKeys((current) => ({ ...current, [sourceId]: false }));
    }
  }

  function toggleSource(sourceId: string) {
    setExpandedSources((current) => ({
      ...current,
      [sourceId]: !current[sourceId],
    }));
  }

  function toggleSourceSection(sourceId: string, section: SourceSection) {
    setExpandedSourceSections((current) => ({
      ...current,
      [sourceId]: {
        ...current[sourceId],
        [section]: !current[sourceId]?.[section],
      },
    }));
  }

  async function deleteSelectedKeys(sourceId: string) {
    if (!config || scope !== 'defaults') return;
    const source = config.web_search.sources.find((item) => item.id === sourceId);
    if (!source) return;
    const selected = (selectedKeyIndices[sourceId] ?? [])
      .filter((keyIndex) => keyIndex >= 0 && keyIndex < source.api_keys.length);
    if (selected.length === 0) return;

    const confirmed = await confirm({
      title: t('tools.webSearch.deleteSelectedTitle'),
      message: t('tools.webSearch.deleteSelectedConfirm', { count: selected.length }),
      confirmLabel: t('tools.webSearch.deleteSelected'),
      tone: 'danger',
    });
    if (!confirmed) return;

    setDeletingKeys((current) => ({ ...current, [sourceId]: true }));
    let latestSource = source;
    let deletedCount = 0;
    try {
      const indices = selected
        .map((keyIndex) => ({ keyIndex, keyHint: source.api_keys[keyIndex] }))
        .filter((entry): entry is { keyIndex: number; keyHint: string } => entry.keyHint !== undefined)
        .sort((left, right) => right.keyIndex - left.keyIndex);
      for (const { keyIndex, keyHint } of indices) {
        const response = await deleteWebSearchKey(sourceId, keyIndex, keyHint);
        latestSource = response.source;
        deletedCount += 1;
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.webSearch.saveFailed'));
    } finally {
      if (deletedCount > 0) {
        const sources = config.web_search.sources.map((item) => (
          item.id === sourceId ? latestSource : item
        ));
        setKeysDirty((current) => ({ ...current, [sourceId]: true }));
        setSelectedKeyIndices((current) => ({ ...current, [sourceId]: [] }));
        setKeyStatuses((current) => ({
          ...current,
          [sourceId]: hydrateKeyStatuses([latestSource])[sourceId] ?? {},
        }));
        updateWebSearch({ sources });
      }
      setDeletingKeys((current) => ({ ...current, [sourceId]: false }));
    }
  }

  async function clearKeys(sourceId: string) {
    if (!config || scope !== 'defaults') return;
    const source = config.web_search.sources.find((item) => item.id === sourceId);
    if (!source || source.api_keys.length === 0) return;
    const confirmed = await confirm({
      title: t('tools.webSearch.clearKeysTitle'),
      message: t('tools.webSearch.clearKeysConfirm', {
        provider: providerLabel(source.provider, t),
        count: source.api_keys.length,
      }),
      confirmLabel: t('tools.webSearch.clearKeys'),
      tone: 'danger',
    });
    if (!confirmed) return;
    const sources = config.web_search.sources.map((source) => (
      source.id === sourceId ? { ...source, api_keys: [] } : source
    ));
    const nextSources = sources.map((source) => (
      source.id === sourceId ? { ...source, key_statuses: [] } : source
    ));
    setKeysDirty((current) => ({ ...current, [sourceId]: true }));
    setSelectedKeyIndices((current) => ({ ...current, [sourceId]: [] }));
    setKeyStatuses((current) => ({ ...current, [sourceId]: {} }));
    updateWebSearch({ sources: nextSources });
  }

  function toggleKeySelection(sourceId: string, keyIndex: number, selected: boolean) {
    setSelectedKeyIndices((current) => {
      const selectedIndices = new Set(current[sourceId] ?? []);
      if (selected) selectedIndices.add(keyIndex);
      else selectedIndices.delete(keyIndex);
      return { ...current, [sourceId]: Array.from(selectedIndices).sort((a, b) => a - b) };
    });
  }

  function selectAllKeys(sourceId: string, count: number) {
    setSelectedKeyIndices((current) => ({
      ...current,
      [sourceId]: Array.from({ length: count }, (_, index) => index),
    }));
  }

  function requestPauseSourceKeys(sourceId: string) {
    if (!checkingKeys[sourceId]) return;
    checkPauseRequestedRef.current[sourceId] = true;
    setCheckProgress((current) => ({
      ...current,
      [sourceId]: current[sourceId]
        ? { ...current[sourceId], pauseRequested: true }
        : current[sourceId],
    }));
  }

  async function checkSourceKeys(sourceId: string, indices: number[]) {
    if (checkingKeys[sourceId] || indices.length === 0) return;
    const queue = Array.from(new Set(indices.filter((index) => Number.isInteger(index) && index >= 0)));
    if (queue.length === 0) return;

    checkPendingIndicesRef.current[sourceId] = queue;
    checkPauseRequestedRef.current[sourceId] = false;
    setCheckProgress((current) => ({
      ...current,
      [sourceId]: {
        total: queue.length,
        completed: 0,
        healthy: 0,
        issues: 0,
        activeKeyIndex: queue[0] ?? null,
        paused: false,
        pauseRequested: false,
      },
    }));
    setCheckingKeys((current) => ({ ...current, [sourceId]: true }));

    let firstRequestError = '';
    try {
      while (checkPendingIndicesRef.current[sourceId]?.length) {
        if (checkPauseRequestedRef.current[sourceId]) break;
        const pending = checkPendingIndicesRef.current[sourceId];
        const keyIndex = pending[0];
        try {
          const response = await checkWebSearchKeys(sourceId, [keyIndex]);
          const result = response.results[0];
          if (result) {
            setKeyStatuses((current) => ({
              ...current,
              [sourceId]: { ...(current[sourceId] ?? {}), [result.key_index]: result },
            }));
            if (scope === 'defaults') {
              setDefaults((current) => {
                if (!current) return current;
                const sources = current.web_search.sources.map((source) => (
                  source.id === sourceId ? mergeCheckedStatuses(source, [result]) : source
                ));
                return withWebSearch({
                  ...current,
                  web_search: { ...current.web_search, sources },
                });
              });
            }
            setCheckProgress((current) => {
              const progress = current[sourceId];
              if (!progress) return current;
              return {
                ...current,
                [sourceId]: {
                  ...progress,
                  completed: progress.completed + 1,
                  healthy: progress.healthy + (result.status === 'normal' ? 1 : 0),
                  issues: progress.issues + (result.status === 'normal' ? 0 : 1),
                },
              };
            });
          }
        } catch (error) {
          if (!firstRequestError) {
            firstRequestError = error instanceof Error ? error.message : t('tools.webSearch.checkFailed');
          }
          setCheckProgress((current) => {
            const progress = current[sourceId];
            if (!progress) return current;
            return {
              ...current,
              [sourceId]: {
                ...progress,
                completed: progress.completed + 1,
                issues: progress.issues + 1,
              },
            };
          });
        } finally {
          checkPendingIndicesRef.current[sourceId] = (checkPendingIndicesRef.current[sourceId] ?? [])
            .slice(1);
          const remaining = checkPendingIndicesRef.current[sourceId] ?? [];
          setCheckProgress((current) => {
            const progress = current[sourceId];
            if (!progress) return current;
            return {
              ...current,
              [sourceId]: {
                ...progress,
                activeKeyIndex: checkPauseRequestedRef.current[sourceId] ? null : (remaining[0] ?? null),
              },
            };
          });
        }
      }
    } finally {
      const remaining = checkPendingIndicesRef.current[sourceId]?.length ?? 0;
      const paused = checkPauseRequestedRef.current[sourceId] && remaining > 0;
      setCheckProgress((current) => ({
        ...current,
        [sourceId]: current[sourceId]
          ? { ...current[sourceId], activeKeyIndex: null, paused, pauseRequested: false }
          : current[sourceId],
      }));
      setCheckingKeys((current) => ({ ...current, [sourceId]: false }));
      checkPauseRequestedRef.current[sourceId] = false;
      if (firstRequestError) toast.error(firstRequestError);
    }
  }

  if (isLoading || !config) {
    return (
      <div className="terminal-tool-settings-v2">
        {isLoading ? <p>{t('tools.webSearch.loading')}</p> : null}
        {!isLoading && configLoadError ? (
          <div className="terminal-load-error" role="alert">
            <strong>{t('tools.webSearch.loadFailed')}</strong>
            <span>{configLoadError}</span>
            <Button variant="secondary" type="button" onClick={() => setLoadRequestKey((c) => c + 1)} icon={<RefreshCw size={15} />}>
              {t('common.retry')}
            </Button>
          </div>
        ) : null}
        {!isLoading && !configLoadError ? <p>{t('tools.webSearch.noConfig')}</p> : null}
      </div>
    );
  }

  const isAgentScope = scope === 'agent';
  const webSearch = config.web_search;
  const evidenceStorage = config.evidence_storage;
  const addKeySource = addKeySourceId
    ? webSearch.sources.find((source) => source.id === addKeySourceId) ?? null
    : null;
  const isAddingKeys = addKeySource ? Boolean(importingKeys[addKeySource.id]) : false;
  const enabledInherited = isAgentScope && overrides?.web_search?.enabled === undefined;
  const maxResultsInherited = isAgentScope && overrides?.web_search?.max_results === undefined;

  return (
    <div className={`terminal-tool-settings-v2 ${isAgentScope ? 'is-agent-scope' : 'is-default-scope'}`}>
      {isAgentScope ? (
        <div className="tool-config-heading">
          <div className="tool-config-heading__main">
            {onBack ? (
              <IconButton type="button" variant="ghost" className="tool-config-back" onClick={onBack} label={t('tools.backToList')}>
                <ArrowLeft size={18} />
              </IconButton>
            ) : null}
            <div>
              <h2>{t('tools.webSearch.agentConfigTitle')}</h2>
              <p>{t('tools.webSearch.agentConfigDesc')}</p>
            </div>
          </div>
          <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryAutoSaveNow} />
        </div>
      ) : null}

      <div className="terminal-settings-form">
        <section className="settings-card basic-settings-card">
          <div className="card-header">
            <h3>{t('tools.webSearch.basicSettings')}</h3>
            {!isAgentScope ? (
              <span className="terminal-settings-save-status">
                <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryAutoSaveNow} />
              </span>
            ) : null}
          </div>
          <div className="card-body">
            <div className="setting-row">
              <div className="setting-info">
                <label>{t('tools.webSearch.enabled')}</label>
                <p className="setting-desc">{t('tools.webSearch.enabledDesc')}</p>
              </div>
              {isAgentScope ? (
                <InheritanceControl
                  inherited={Boolean(enabledInherited)}
                  onChange={setEnabledInherited}
                  t={t}
                />
              ) : null}
              <label className="toggle-switch">
                <input
                  type="checkbox"
                  aria-label={t('tools.webSearch.enabled')}
                  checked={webSearch.enabled}
                  disabled={Boolean(isAgentScope && enabledInherited)}
                  onChange={(e) => updateWebSearch({ enabled: e.target.checked })}
                />
                <span className="toggle-slider" />
              </label>
            </div>

            <div className="setting-row">
              <div className="setting-info">
                <label>{t('tools.webSearch.maxResults')}</label>
                <p className="setting-desc">{t('tools.webSearch.maxResultsDesc')}</p>
              </div>
              {isAgentScope ? (
                <InheritanceControl
                  inherited={Boolean(maxResultsInherited)}
                  onChange={setMaxResultsInherited}
                  t={t}
                />
              ) : null}
              <div className="timeout-input">
                <input
                  type="number"
                  min={1}
                  max={MAX_RESULTS}
                  disabled={Boolean(isAgentScope && maxResultsInherited)}
                  value={Math.min(MAX_RESULTS, Math.max(1, webSearch.max_results || 1))}
                  onChange={(e) => {
                    const value = Number(e.target.value);
                    if (!Number.isFinite(value)) return;
                    updateWebSearch({ max_results: Math.min(MAX_RESULTS, Math.max(1, Math.round(value))) });
                  }}
                />
              </div>
            </div>

            {!isAgentScope ? (
              <>
                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.timeout')}</label>
                    <p className="setting-desc">{t('tools.webSearch.timeoutDesc')}</p>
                  </div>
                  <div className="timeout-input">
                    <input
                      type="number"
                      min={1000}
                      max={60000}
                      step={1000}
                      value={webSearch.timeout_ms}
                      onChange={(e) => {
                        const value = Number(e.target.value);
                        if (!Number.isFinite(value)) return;
                        updateWebSearch({ timeout_ms: Math.min(60000, Math.max(1000, Math.round(value))) });
                      }}
                    />
                  </div>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.keySelection')}</label>
                    <p className="setting-desc">{t('tools.webSearch.keySelectionDesc')}</p>
                  </div>
                  <select
                    className="web-search-select"
                    value={webSearch.key_selection}
                    onChange={(e) => updateWebSearch({ key_selection: e.target.value as WebSearchKeySelection })}
                  >
                    <option value="sequential">{t('tools.webSearch.keySelectionSequential')}</option>
                    <option value="round_robin">{t('tools.webSearch.keySelectionRoundRobin')}</option>
                    <option value="random">{t('tools.webSearch.keySelectionRandom')}</option>
                  </select>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.remoteExtractionEnabled')}</label>
                    <p className="setting-desc">{t('tools.webSearch.remoteExtractionEnabledDesc')}</p>
                  </div>
                  <label className="toggle-switch">
                    <input
                      type="checkbox"
                      aria-label={t('tools.webSearch.remoteExtractionEnabled')}
                      checked={webSearch.extraction.remote_enabled}
                      onChange={(event) => updateExtraction({ remote_enabled: event.target.checked })}
                    />
                    <span className="toggle-slider" />
                  </label>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.maxRemoteExtractAttempts')}</label>
                    <p className="setting-desc">{t('tools.webSearch.maxRemoteExtractAttemptsDesc')}</p>
                  </div>
                  <div className="timeout-input">
                    <input
                      type="number"
                      min={1}
                      max={5}
                      value={webSearch.extraction.max_remote_attempts_per_url}
                      onChange={(event) => {
                        const value = Number(event.target.value);
                        if (!Number.isFinite(value)) return;
                        updateExtraction({
                          max_remote_attempts_per_url: Math.min(5, Math.max(1, Math.round(value))),
                        });
                      }}
                    />
                  </div>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.allowGrokExtractFallback')}</label>
                    <p className="setting-desc">{t('tools.webSearch.allowGrokExtractFallbackDesc')}</p>
                  </div>
                  <label className="toggle-switch">
                    <input
                      type="checkbox"
                      aria-label={t('tools.webSearch.allowGrokExtractFallback')}
                      checked={webSearch.extraction.allow_grok_fallback}
                      disabled={!webSearch.extraction.remote_enabled}
                      onChange={(event) => updateExtraction({ allow_grok_fallback: event.target.checked })}
                    />
                    <span className="toggle-slider" />
                  </label>
                </div>

                <div className="web-search-evidence-divider">
                  <div className="settings-section-title">
                    <h3>{t('tools.webSearch.evidenceStorageTitle')}</h3>
                    <p>{t('tools.webSearch.evidenceStorageDesc')}</p>
                  </div>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.evidenceStorageEnabled')}</label>
                    <p className="setting-desc">{t('tools.webSearch.evidenceStorageEnabledDesc')}</p>
                  </div>
                  <label className="toggle-switch">
                    <input
                      type="checkbox"
                      aria-label={t('tools.webSearch.evidenceStorageEnabled')}
                      checked={evidenceStorage.enabled}
                      onChange={(e) => updateEvidenceStorage({ enabled: e.target.checked })}
                    />
                    <span className="toggle-slider" />
                  </label>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.evidenceRetentionDays')}</label>
                    <p className="setting-desc">{t('tools.webSearch.evidenceRetentionDaysDesc')}</p>
                  </div>
                  <div className="timeout-input">
                    <input
                      type="number"
                      min={1}
                      max={MAX_EVIDENCE_RETENTION_DAYS}
                      value={evidenceStorage.retention_days}
                      onChange={(e) => {
                        const value = Number(e.target.value);
                        if (!Number.isFinite(value)) return;
                        updateEvidenceStorage({
                          retention_days: Math.min(MAX_EVIDENCE_RETENTION_DAYS, Math.max(1, Math.round(value))),
                        });
                      }}
                    />
                  </div>
                </div>

                <div className="setting-row">
                  <div className="setting-info">
                    <label>{t('tools.webSearch.evidenceMaxTotalMb')}</label>
                    <p className="setting-desc">{t('tools.webSearch.evidenceMaxTotalMbDesc')}</p>
                  </div>
                  <div className="timeout-input">
                    <input
                      type="number"
                      min={1}
                      max={MAX_EVIDENCE_TOTAL_MB}
                      value={evidenceStorage.max_total_mb}
                      onChange={(e) => {
                        const value = Number(e.target.value);
                        if (!Number.isFinite(value)) return;
                        updateEvidenceStorage({
                          max_total_mb: Math.min(MAX_EVIDENCE_TOTAL_MB, Math.max(1, Math.round(value))),
                        });
                      }}
                    />
                  </div>
                </div>
              </>
            ) : null}
          </div>
        </section>

        {!isAgentScope ? (
          <section className="settings-card web-search-sources-card">
            <div className="card-header">
              <div className="settings-section-title">
                <h3>{t('tools.webSearch.sourcesTitle')}</h3>
                <p>{t('tools.webSearch.sourcesDesc')}</p>
              </div>
              <span className="badge">{webSearch.sources.length}</span>
            </div>
            <div className="card-body">
              <div className="web-search-source-list">
                {webSearch.sources.map((source, index) => {
                  const keyCount = source.api_keys.length;
                  const selected = (selectedKeyIndices[source.id] ?? [])
                    .filter((keyIndex) => keyIndex >= 0 && keyIndex < keyCount);
                  const sourceStatuses = keyStatuses[source.id] ?? {};
                  const canEnableSource = sourceCanBeEnabled(source);
                  const isChecking = Boolean(checkingKeys[source.id]);
                  const isDeleting = Boolean(deletingKeys[source.id]);
                  const isExpanded = Boolean(expandedSources[source.id]);
                  const isPreferencesExpanded = Boolean(expandedSourceSections[source.id]?.preferences);
                  const isKeysExpanded = Boolean(expandedSourceSections[source.id]?.keys);
                  const isConnectionExpanded = Boolean(expandedSourceSections[source.id]?.connection);
                  const allSelected = keyCount > 0 && selected.length === keyCount;
                  const statusSummary = summarizeKeyStatuses(keyCount, sourceStatuses);
                  const currentCheckProgress = checkProgress[source.id];
                  const hasPausedChecks = Boolean(
                    currentCheckProgress?.paused && (checkPendingIndicesRef.current[source.id]?.length ?? 0) > 0,
                  );
                  const checkIndices = selected.length > 0
                    ? selected
                    : source.api_keys.map((_, keyIndex) => keyIndex);
                  const emptyLabel = t(source.provider === 'grok' ? 'tools.webSearch.noSso' : 'tools.webSearch.noKeys');
                  const sourceBodyId = `web-search-source-body-${source.id}`;
                  const preferencesBodyId = `web-search-preferences-body-${source.id}`;
                  const keysBodyId = `web-search-keys-body-${source.id}`;
                  const connectionBodyId = `web-search-connection-body-${source.id}`;
                  return (
                    <div
                      key={source.id}
                      className={`web-search-source ${source.enabled ? 'is-enabled' : 'is-disabled'} ${isExpanded ? 'is-expanded' : 'is-collapsed'}`}
                    >
                      <div className="web-search-source__header">
                        <button
                          type="button"
                          className="web-search-source__summary"
                          aria-expanded={isExpanded}
                          aria-controls={sourceBodyId}
                          onClick={() => toggleSource(source.id)}
                        >
                          <span
                            className="web-search-source__priority"
                            title={t('tools.webSearch.sourcePriority', { index: String(index + 1) })}
                          >
                            {index + 1}
                          </span>
                          <span className="web-search-source__meta">
                            <span className="web-search-source__title-line">
                              <strong>{providerLabel(source.provider, t)}</strong>
                            </span>
                            <span className="web-search-source__health" aria-label={t('tools.webSearch.statusSummary')}>
                              <span className="is-total">
                                <KeyRound size={13} />
                                {t('tools.webSearch.totalKeyCount', { count: keyCount })}
                              </span>
                              <span className="is-healthy">
                                <CheckCircle2 size={13} />
                                {t('tools.webSearch.healthyCount', { count: statusSummary.healthy })}
                              </span>
                              <span className="is-issue">
                                <CircleAlert size={13} />
                                {t('tools.webSearch.issueCount', { count: statusSummary.issues })}
                              </span>
                              <span className="is-unchecked">
                                <Clock3 size={13} />
                                {t('tools.webSearch.uncheckedCount', { count: statusSummary.unchecked })}
                              </span>
                            </span>
                          </span>
                          <ChevronDown size={18} className="web-search-source__chevron" />
                        </button>
                        <div className="web-search-source__actions">
                          <label
                            className={`toggle-switch web-search-source__toggle ${canEnableSource ? '' : 'is-disabled'}`}
                            title={canEnableSource
                              ? t(source.enabled ? 'tools.enabled' : 'tools.disabled')
                              : t('tools.webSearch.keyRequiredToEnable')}
                          >
                            <input
                              type="checkbox"
                              checked={source.enabled}
                              disabled={!canEnableSource}
                              onChange={(e) => updateSource(index, { enabled: e.target.checked })}
                            />
                            <span className="toggle-slider" />
                            <span className="web-search-source__toggle-text">
                              {t(source.enabled ? 'tools.enabled' : 'tools.disabled')}
                            </span>
                          </label>
                          <div
                            className="web-search-source__reorder"
                            role="group"
                            aria-label={t('tools.webSearch.sourcePriority', { index: String(index + 1) })}
                          >
                            <IconButton
                              type="button"
                              variant="ghost"
                              className="web-search-source__reorder-btn"
                              label={t('tools.webSearch.moveUp')}
                              onClick={() => moveSource(index, -1)}
                              disabled={index === 0}
                            >
                              <ArrowUp size={15} />
                            </IconButton>
                            <IconButton
                              type="button"
                              variant="ghost"
                              className="web-search-source__reorder-btn"
                              label={t('tools.webSearch.moveDown')}
                              onClick={() => moveSource(index, 1)}
                              disabled={index === webSearch.sources.length - 1}
                            >
                              <ArrowDown size={15} />
                            </IconButton>
                          </div>
                        </div>
                      </div>

                      {isExpanded ? (
                        <div className="web-search-source__body" id={sourceBodyId}>
                          <section className={`web-search-subsection web-search-source-profile ${isPreferencesExpanded ? 'is-open' : ''}`}>
                            <div className="web-search-subsection__header">
                              <button
                                type="button"
                                className="web-search-subsection__toggle"
                                aria-expanded={isPreferencesExpanded}
                                aria-controls={preferencesBodyId}
                                onClick={() => toggleSourceSection(source.id, 'preferences')}
                              >
                                <span className="web-search-section-icon"><Settings2 size={15} /></span>
                                <span className="web-search-subsection__copy">
                                  <strong>{t('tools.webSearch.sourceProfile')}</strong>
                                  <small>{t('tools.webSearch.sourceProfileDesc')}</small>
                                </span>
                                <ChevronDown size={16} className="web-search-subsection__chevron" />
                              </button>
                            </div>
                            {isPreferencesExpanded ? (
                              <div
                                className="web-search-subsection__content web-search-source-profile__content"
                                id={preferencesBodyId}
                              >
                                <div className="web-search-source-profile__groups">
                                  {SOURCE_ROLE_GROUPS.map((group) => (
                                    <fieldset className="web-search-source-profile__group" key={group.key}>
                                      <legend>{t(`tools.webSearch.sourcePreferenceGroup.${group.key}`)}</legend>
                                      <div className="web-search-source-profile__options">
                                        {group.roles.map((role) => (
                                          <label key={role} title={t(`tools.webSearch.sourceRoleDesc.${role}`)}>
                                            <input
                                              type="checkbox"
                                              checked={source.roles.includes(role)}
                                              onChange={(event) => toggleSourceRole(index, role, event.target.checked)}
                                            />
                                            <span>{t(`tools.webSearch.sourceRole.${role}`)}</span>
                                          </label>
                                        ))}
                                        {group.key === 'intent' ? (
                                          <>
                                            <label title={t('tools.webSearch.deepEnabledDesc')}>
                                              <input
                                                type="checkbox"
                                                checked={source.deep_enabled}
                                                onChange={(event) => updateSource(index, { deep_enabled: event.target.checked })}
                                              />
                                              <span>{t('tools.webSearch.deepEnabled')}</span>
                                            </label>
                                            <label title={t('tools.webSearch.extractEnabledDesc')}>
                                              <input
                                                type="checkbox"
                                                checked={source.extract_enabled}
                                                onChange={(event) => updateSource(index, { extract_enabled: event.target.checked })}
                                              />
                                              <span>{t('tools.webSearch.extractEnabled')}</span>
                                            </label>
                                          </>
                                        ) : null}
                                      </div>
                                    </fieldset>
                                  ))}
                                </div>
                              </div>
                            ) : null}
                          </section>

                          <section className={`web-search-subsection web-search-keys ${isKeysExpanded ? 'is-open' : ''}`}>
                            <div className={`web-search-subsection__header web-search-keys__heading ${selected.length > 0 ? 'has-selection' : ''}`}>
                              <button
                                type="button"
                                className="web-search-subsection__toggle"
                                aria-expanded={isKeysExpanded}
                                aria-controls={keysBodyId}
                                onClick={() => toggleSourceSection(source.id, 'keys')}
                              >
                                <span className="web-search-section-icon"><KeyRound size={15} /></span>
                                <span className="web-search-subsection__copy">
                                  <strong>{t('tools.webSearch.keysLabel')}</strong>
                                  <small>{t('tools.webSearch.keysSectionDesc')}</small>
                                </span>
                                <ChevronDown size={16} className="web-search-subsection__chevron" />
                              </button>
                              <div className="web-search-keys__toolbar">
                                {isKeysExpanded && keyCount > 0 ? (
                                  <label className="web-search-keys__select-all">
                                    <input
                                      type="checkbox"
                                      checked={allSelected}
                                      ref={(node) => {
                                        if (node) node.indeterminate = selected.length > 0 && !allSelected;
                                      }}
                                      onChange={() => selectAllKeys(source.id, allSelected ? 0 : keyCount)}
                                    />
                                    <span>{allSelected ? t('tools.webSearch.clearSelection') : t('tools.webSearch.selectAll')}</span>
                                  </label>
                                ) : null}
                                {isKeysExpanded && keyCount > 0 ? (
                                  <>
                                    <Button
                                      type="button"
                                      variant="secondary"
                                      className="web-search-keys__check-btn"
                                      icon={isChecking
                                        ? (currentCheckProgress?.pauseRequested
                                          ? <LoaderCircle size={14} className="is-spinning" />
                                          : <Pause size={14} />)
                                        : hasPausedChecks ? <Play size={14} /> : <ScanSearch size={15} />}
                                      title={t('tools.webSearch.keyCheckHint')}
                                      onClick={() => {
                                        if (isChecking) {
                                          requestPauseSourceKeys(source.id);
                                        } else if (hasPausedChecks) {
                                          void checkSourceKeys(source.id, checkPendingIndicesRef.current[source.id] ?? []);
                                        } else {
                                          void checkSourceKeys(source.id, checkIndices);
                                        }
                                      }}
                                      disabled={isDeleting || Boolean(currentCheckProgress?.pauseRequested)}
                                    >
                                      {isChecking
                                        ? (currentCheckProgress?.pauseRequested
                                          ? t('tools.webSearch.checkPausing')
                                          : t('tools.webSearch.pauseCheck'))
                                        : hasPausedChecks
                                          ? t('tools.webSearch.resumeCheck')
                                          : selected.length > 0
                                            ? t('tools.webSearch.checkSelected', { count: selected.length })
                                            : t('tools.webSearch.checkAll')}
                                    </Button>
                                    {currentCheckProgress ? (
                                      <span className={`web-search-key-progress ${currentCheckProgress.paused ? 'is-paused' : ''}`} role="status">
                                        {currentCheckProgress.paused
                                          ? t('tools.webSearch.checkProgressPaused', {
                                            completed: currentCheckProgress.completed,
                                            total: currentCheckProgress.total,
                                            healthy: currentCheckProgress.healthy,
                                            issues: currentCheckProgress.issues,
                                            remaining: currentCheckProgress.total - currentCheckProgress.completed,
                                          })
                                          : t('tools.webSearch.checkProgress', {
                                            completed: currentCheckProgress.completed,
                                            total: currentCheckProgress.total,
                                            healthy: currentCheckProgress.healthy,
                                            issues: currentCheckProgress.issues,
                                            remaining: currentCheckProgress.total - currentCheckProgress.completed,
                                          })}
                                      </span>
                                    ) : null}
                                  </>
                                ) : null}
                                {isKeysExpanded && selected.length > 0 ? (
                                  <Button
                                    type="button"
                                    variant="danger"
                                    className="web-search-keys__delete-selected"
                                    icon={isDeleting ? <LoaderCircle size={14} className="is-spinning" /> : <Trash2 size={14} />}
                                    onClick={() => void deleteSelectedKeys(source.id)}
                                    disabled={isDeleting || isChecking}
                                  >
                                    {t('tools.webSearch.deleteSelected')}
                                  </Button>
                                ) : null}
                                {isKeysExpanded && keyCount > 0 ? (
                                  <Button
                                    type="button"
                                    variant="danger"
                                    className="web-search-keys__clear"
                                    icon={<Trash2 size={14} />}
                                    onClick={() => void clearKeys(source.id)}
                                    disabled={isDeleting || isChecking}
                                  >
                                    {t('tools.webSearch.clearKeys')}
                                  </Button>
                                ) : null}
                                <IconButton
                                  type="button"
                                  variant="default"
                                  className="web-search-keys__add"
                                  label={t('tools.webSearch.addKey')}
                                  onClick={() => setAddKeySourceId(source.id)}
                                  disabled={isDeleting || isChecking || Boolean(importingKeys[source.id])}
                                >
                                  {importingKeys[source.id]
                                    ? <LoaderCircle size={17} className="is-spinning" />
                                    : <Plus size={18} />}
                                </IconButton>
                              </div>
                            </div>

                            {isKeysExpanded ? (
                              <div className="web-search-subsection__content web-search-keys__content" id={keysBodyId}>
                                {keyCount === 0 ? (
                                  <div className="web-search-keys__empty">
                                    <KeyRound size={18} />
                                    <div>
                                      <strong>{emptyLabel}</strong>
                                      <span>{t('tools.webSearch.emptyKeysHint')}</span>
                                    </div>
                                  </div>
                                ) : (
                                  <ul className="web-search-key-list">
                                    {source.api_keys
                                      .map((key, keyIndex) => ({ key, keyIndex, status: sourceStatuses[keyIndex] }))
                                      .sort((left, right) => (
                                        keyStatusRank(left.status?.status) - keyStatusRank(right.status?.status)
                                        || left.keyIndex - right.keyIndex
                                      ))
                                      .map(({ key, keyIndex, status }) => (
                                        <li
                                          key={`${source.id}-${keyIndex}`}
                                          className={`web-search-key-card ${selected.includes(keyIndex) ? 'is-selected' : ''}`}
                                        >
                                          <input
                                            type="checkbox"
                                            className="web-search-key-card__select"
                                            aria-label={t('tools.webSearch.selectKey', { index: keyIndex + 1 })}
                                            checked={selected.includes(keyIndex)}
                                            onChange={(event) => toggleKeySelection(source.id, keyIndex, event.target.checked)}
                                          />
                                          <div className="web-search-key-card__content">
                                            <div className="web-search-key-card__topline">
                                              <code title={key}>{key}</code>
                                              {status ? (
                                                <span
                                                  className={`web-search-key-status is-${status.status}`}
                                                  title={status.detail}
                                                >
                                                  {keyStatusIcon(status.status)}
                                                  <span>{keyStatusLabel(status.status, t)}</span>
                                                </span>
                                              ) : (
                                                <span className="web-search-key-status is-unchecked">
                                                  <Clock3 size={14} />
                                                  <span>{t('tools.webSearch.unchecked')}</span>
                                                </span>
                                              )}
                                            </div>
                                            {status ? (
                                              <small className="web-search-key-card__status-detail" title={status.detail}>
                                                <time dateTime={status.checked_at}>{formatCheckedAt(status.checked_at)}</time>
                                                <span aria-hidden="true">·</span>
                                                <span>{status.detail}</span>
                                              </small>
                                            ) : null}
                                          </div>
                                          <div className="web-search-key-card__actions">
                                            <IconButton
                                              type="button"
                                              variant="ghost"
                                              className="web-search-key-card__check"
                                              label={t('tools.webSearch.checkOne')}
                                              title={t('tools.webSearch.keyCheckHint')}
                                              onClick={() => checkSourceKeys(source.id, [keyIndex])}
                                              disabled={isChecking || isDeleting}
                                            >
                                              {isChecking && currentCheckProgress?.activeKeyIndex === keyIndex
                                                ? <LoaderCircle size={14} className="is-spinning" />
                                                : <ScanSearch size={15} />}
                                            </IconButton>
                                            <IconButton
                                              type="button"
                                              variant="ghost"
                                              className="web-search-key-card__remove"
                                              label={t('tools.webSearch.removeKey')}
                                              onClick={() => removeKey(source.id, keyIndex)}
                                              disabled={isDeleting || isChecking}
                                            >
                                              {isDeleting ? <LoaderCircle size={14} className="is-spinning" /> : <Trash2 size={14} />}
                                            </IconButton>
                                          </div>
                                        </li>
                                      ))}
                                  </ul>
                                )}
                              </div>
                            ) : null}
                          </section>

                          <section className={`web-search-subsection web-search-grok-extras ${isConnectionExpanded ? 'is-open' : ''}`}>
                              <div className="web-search-subsection__header">
                                <button
                                  type="button"
                                  className="web-search-subsection__toggle"
                                  aria-expanded={isConnectionExpanded}
                                  aria-controls={connectionBodyId}
                                  onClick={() => toggleSourceSection(source.id, 'connection')}
                                >
                                  <span className="web-search-section-icon"><Settings2 size={15} /></span>
                                  <span className="web-search-subsection__copy">
                                    <strong>{t('tools.webSearch.advancedSettings')}</strong>
                                    <small>{t('tools.webSearch.advancedSettingsDesc')}</small>
                                  </span>
                                  <ChevronDown size={16} className="web-search-subsection__chevron" />
                                </button>
                              </div>
                              {isConnectionExpanded ? (
                                <div
                                  className="web-search-subsection__content web-search-grok-extras__content"
                                  id={connectionBodyId}
                                >
                                  <div className="web-search-extra-field">
                                    <div className="web-search-extra-field__label">
                                      <label htmlFor={`extract-base-url-${source.id}`}>{t('tools.webSearch.extractBaseUrl')}</label>
                                      <p className="setting-desc">{t('tools.webSearch.extractBaseUrlDesc')}</p>
                                    </div>
                                    <input
                                      id={`extract-base-url-${source.id}`}
                                      type="text"
                                      className="pattern-input web-search-extra-input"
                                      value={source.extract_base_url ?? ''}
                                      placeholder={t(`tools.webSearch.extractBaseUrlPlaceholder.${source.provider}`)}
                                      onChange={(event) => updateSource(index, {
                                        extract_base_url: event.target.value.trim() ? event.target.value.trim() : null,
                                      })}
                                    />
                                  </div>
                                  {source.provider === 'grok' ? (
                                    <>
                                  <div className="web-search-extra-field">
                                    <div className="web-search-extra-field__label">
                                      <label htmlFor={`grok-flaresolverr-${source.id}`}>{t('tools.webSearch.flaresolverrUrl')}</label>
                                      <p className="setting-desc">{t('tools.webSearch.flaresolverrUrlDesc')}</p>
                                    </div>
                                    <input
                                      id={`grok-flaresolverr-${source.id}`}
                                      type="text"
                                      className="pattern-input web-search-extra-input"
                                      value={source.flaresolverr_url ?? ''}
                                      placeholder={t('tools.webSearch.flaresolverrUrlPlaceholder')}
                                      onChange={(e) => updateSource(index, {
                                        flaresolverr_url: e.target.value.trim() ? e.target.value.trim() : null,
                                      })}
                                    />
                                  </div>
                                  <div className="web-search-extra-field">
                                    <div className="web-search-extra-field__label">
                                      <label htmlFor={`grok-fs-proxy-${source.id}`}>{t('tools.webSearch.flaresolverrProxyUrl')}</label>
                                      <p className="setting-desc">{t('tools.webSearch.flaresolverrProxyUrlDesc')}</p>
                                    </div>
                                    <input
                                      id={`grok-fs-proxy-${source.id}`}
                                      type="text"
                                      className="pattern-input web-search-extra-input"
                                      value={source.flaresolverr_proxy_url ?? ''}
                                      placeholder={t('tools.webSearch.flaresolverrProxyUrlPlaceholder')}
                                      onChange={(e) => updateSource(index, {
                                        flaresolverr_proxy_url: e.target.value.trim() ? e.target.value.trim() : null,
                                      })}
                                    />
                                  </div>
                                  <div className="web-search-extra-field">
                                    <div className="web-search-extra-field__label">
                                      <label htmlFor={`grok-proxy-${source.id}`}>{t('tools.webSearch.proxyUrl')}</label>
                                      <p className="setting-desc">{t('tools.webSearch.proxyUrlDesc')}</p>
                                    </div>
                                    <input
                                      id={`grok-proxy-${source.id}`}
                                      type="text"
                                      className="pattern-input web-search-extra-input"
                                      value={source.proxy_url ?? ''}
                                      placeholder={t('tools.webSearch.proxyUrlPlaceholder')}
                                      onChange={(e) => updateSource(index, {
                                        proxy_url: e.target.value.trim() ? e.target.value.trim() : null,
                                      })}
                                    />
                                  </div>
                                    </>
                                  ) : null}
                                </div>
                              ) : null}
                          </section>
                        </div>
                      ) : null}
                    </div>
                  );
                })}
              </div>
            </div>
          </section>
        ) : (
          <section className="settings-card">
            <div className="card-body">
              <p className="card-description">{t('tools.webSearch.agentSecretsHint')}</p>
            </div>
          </section>
        )}
      </div>

      {addKeySource ? (
        <Dialog
          open
          title={t('tools.webSearch.addKeysDialogTitle', { provider: providerLabel(addKeySource.provider, t) })}
          description={t(addKeySource.provider === 'grok' ? 'tools.webSearch.addGrokKeysHint' : 'tools.webSearch.addKeysHint')}
          icon={<KeyRound size={19} />}
          closeLabel={t('common.close')}
          onClose={() => {
            if (!isAddingKeys) closeAddKeyDialog();
          }}
          closeOnBackdrop={!isAddingKeys}
          closeOnEscape={!isAddingKeys}
          size="sm"
          className="web-search-key-dialog"
          bodyClassName="web-search-key-dialog__body"
          footer={(
            <>
              <Button
                type="button"
                variant="ghost"
                onClick={closeAddKeyDialog}
                disabled={isAddingKeys}
              >
                {t('common.cancel')}
              </Button>
              <Button
                type="button"
                variant="primary"
                icon={isAddingKeys ? <LoaderCircle size={15} className="is-spinning" /> : <Plus size={15} />}
                onClick={() => void addKeys(addKeySource.id)}
                disabled={isAddingKeys || !(draftKeys[addKeySource.id] ?? '').trim()}
              >
                {t('tools.webSearch.addKey')}
              </Button>
            </>
          )}
        >
          <label className="web-search-key-dialog__field" htmlFor={`web-search-key-input-${addKeySource.id}`}>
            <span>{t('tools.webSearch.keysLabel')}</span>
            <textarea
              id={`web-search-key-input-${addKeySource.id}`}
              className="pattern-input web-search-key-dialog__input"
              value={draftKeys[addKeySource.id] ?? ''}
              placeholder={t(addKeySource.provider === 'grok' ? 'tools.webSearch.ssoPlaceholder' : 'tools.webSearch.keyPlaceholder')}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setDraftKeys((current) => ({ ...current, [addKeySource.id]: event.target.value }))}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
                  event.preventDefault();
                  void addKeys(addKeySource.id);
                }
              }}
            />
          </label>
        </Dialog>
      ) : null}
    </div>
  );
}

function InheritanceControl({ inherited, onChange, t }: {
  inherited: boolean;
  onChange: (inherited: boolean) => void;
  t: (path: string) => string;
}) {
  return (
    <label className="inherit-control">
      <input type="checkbox" checked={inherited} onChange={(event) => onChange(event.target.checked)} />
      <span>{t('tools.webSearch.inheritDefault')}</span>
    </label>
  );
}
