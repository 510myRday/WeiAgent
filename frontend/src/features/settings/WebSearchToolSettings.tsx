import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { ArrowLeft, RefreshCw } from 'lucide-react';

import type {
  AgentToolsConfig,
  AgentToolsOverrides,
  AgentToolsSettings,
  WebSearchKeySelection,
  WebSearchSourceOverrides,
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
  listIpProxies,
  readToolDefaults,
  updateAgentToolsConfig,
  updateToolDefaults,
} from '../../api';
import type { Agent, IpProxy } from '../../types';
import { useI18n } from '../../i18n';
import { useBeforeUnloadGuard } from '../../hooks';
import { userErrorMessage } from '../../utils/userError';
import { AutoSaveIndicator, Button, IconButton, useFeedback } from '../../components/ui';
import {
  AUTO_SAVE_DELAY_MS,
  AUTO_SAVE_MAX_RETRY_DELAY_MS,
  AUTO_SAVE_RETRY_DELAY_MS,
  MAX_EVIDENCE_RETENTION_DAYS,
  MAX_EVIDENCE_TOTAL_MB,
  MAX_RESULTS,
} from './web-search/types';
import type {
  AutoSaveStatus,
  ExpandedSourceSections,
  PendingSave,
  ProxyPickerField,
  SourceSection,
} from './web-search/types';
import {
  hydrateKeyStatuses,
  mergeCheckedStatuses,
  mergeToolsConfig,
  providerLabel,
  serializeDefaultsForAutosave,
  withWebSearch,
} from './web-search/utils';
import { initialKeysState, keysReducer } from './web-search/keysReducer';
import { SourceCard } from './web-search/SourceCard';
import { ProxyPickerDialog } from './web-search/ProxyPickerDialog';
import { AddKeysDialog } from './web-search/AddKeysDialog';

interface WebSearchToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
  onBack?: () => void;
  onOpenIpProxies?: () => void;
}

export function WebSearchToolSettings({
  scope,
  agentId,
  onBack,
  onOpenIpProxies,
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
  const [keysState, dispatchKeys] = useReducer(keysReducer, undefined, initialKeysState);
  const {
    keysDirty,
    selectedKeyIndices,
    keyStatuses,
    checkingKeys,
    checkProgress,
    importingKeys,
    deletingKeys,
  } = keysState;
  const [expandedSources, setExpandedSources] = useState<Record<string, boolean>>({});
  const [expandedSourceSections, setExpandedSourceSections] = useState<ExpandedSourceSections>({});
  const [addKeySourceId, setAddKeySourceId] = useState<string | null>(null);
  const [ipProxies, setIpProxies] = useState<IpProxy[]>([]);
  const [isLoadingIpProxies, setIsLoadingIpProxies] = useState(true);
  const [hasLoadedIpProxies, setHasLoadedIpProxies] = useState(false);
  const [proxyPicker, setProxyPicker] = useState<{ sourceIndex: number; field: ProxyPickerField } | null>(null);

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
  const proxyPickerSource = proxyPicker && config
    ? config.web_search.sources[proxyPicker.sourceIndex]
    : null;
  const proxyPickerValue = proxyPickerSource && proxyPicker
    ? proxyPickerSource[proxyPicker.field]
    : null;

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
    setIsLoadingIpProxies(true);
    listIpProxies()
      .then((next) => {
        if (!cancelled) {
          setIpProxies(next);
          setHasLoadedIpProxies(true);
        }
      })
      .catch(() => {
        if (!cancelled) toast.warning(t('tools.webSearch.ipProxyLoadFailed'));
      })
      .finally(() => {
        if (!cancelled) setIsLoadingIpProxies(false);
      });
    return () => {
      cancelled = true;
    };
  }, [t, toast]);

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
        setAddKeySourceId(null);
        dispatchKeys({ type: 'setKeysDirtyAll', keysDirty: {} });
        dispatchKeys({ type: 'setSelectedAll', selectedKeyIndices: {} });
        setExpandedSources({});
        setExpandedSourceSections({});
        dispatchKeys({ type: 'setAllKeyStatuses', keyStatuses: hydrateKeyStatuses(normalized.web_search.sources) });
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
              web_search: {
                ...next.overrides.web_search,
                source_overrides: next.overrides.web_search?.source_overrides ?? [],
              },
            },
          };
          setAgentSettings(normalized);
          setDefaults(normalized.defaults);
          setAddKeySourceId(null);
          dispatchKeys({ type: 'setSelectedAll', selectedKeyIndices: {} });
          setExpandedSources({});
          setExpandedSourceSections({});
          dispatchKeys({ type: 'setAllKeyStatuses', keyStatuses: hydrateKeyStatuses(normalized.effective.web_search.sources) });
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
          dispatchKeys({ type: 'setAllKeyStatuses', keyStatuses: hydrateKeyStatuses(saved.web_search.sources) });
          dispatchKeys({ type: 'setKeysDirtyAll', keysDirty: {} });
        }
      } else {
        const saved = await updateAgentToolsConfig(nextSave.agentId, nextSave.overrides);
        if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
          setAgentSettings({
            ...saved,
            defaults: withWebSearch(saved.defaults),
            effective: withWebSearch(saved.effective),
          });
          dispatchKeys({ type: 'setAllKeyStatuses', keyStatuses: hydrateKeyStatuses(saved.effective.web_search.sources) });
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
          toast.error(userErrorMessage(error, {
            summary: t('tools.webSearch.saveFailed'),
            recovery: t('error.retryConnection'),
          }));
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

  const updateGlobalConfig = useCallback((next: AgentToolsConfig) => {
    setDefaults(withWebSearch(next));
  }, []);

  const updateOverrides = useCallback((next: AgentToolsOverrides) => {
    setAgentSettings((current) => (current ? {
      ...current,
      overrides: next,
      effective: mergeToolsConfig(current.defaults, next),
    } : current));
  }, []);

  const updateWebSearch = useCallback((patch: Partial<WebSearchToolConfig>) => {
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
  }, [config, overrides, scope, updateGlobalConfig, updateOverrides]);

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

  const updateSource = useCallback((index: number, patch: Partial<WebSearchSourceConfig>) => {
    if (!config) return;
    const source = config.web_search.sources[index];
    if (!source) return;
    if (scope === 'defaults') {
      const sources = config.web_search.sources.map((item, i) => (i === index ? { ...item, ...patch } : item));
      updateWebSearch({ sources });
      return;
    }
    if (!overrides) return;
    const existing = overrides.web_search.source_overrides.find((item) => item.source_id === source.id);
    const nextSourceOverride: WebSearchSourceOverrides = {
      source_id: source.id,
      enabled: patch.enabled ?? existing?.enabled ?? source.enabled,
      deep_enabled: patch.deep_enabled ?? existing?.deep_enabled ?? source.deep_enabled,
      extract_enabled: patch.extract_enabled ?? existing?.extract_enabled ?? source.extract_enabled,
      priority: patch.priority ?? existing?.priority ?? source.priority,
      roles: patch.roles ?? existing?.roles ?? source.roles,
      proxy_url: Object.prototype.hasOwnProperty.call(patch, 'proxy_url')
        ? patch.proxy_url ?? null
        : existing?.proxy_url_override
          ? existing.proxy_url ?? null
          : source.proxy_url,
      proxy_url_override: existing?.proxy_url_override ?? true,
    };
    updateOverrides({
      ...overrides,
      web_search: {
        ...overrides.web_search,
        source_overrides: [
          ...overrides.web_search.source_overrides.filter((item) => item.source_id !== source.id),
          nextSourceOverride,
        ],
      },
    });
  }, [config, overrides, scope, updateOverrides, updateWebSearch]);

  function resetSource(sourceId: string) {
    if (!overrides) return;
    updateOverrides({
      ...overrides,
      web_search: {
        ...overrides.web_search,
        source_overrides: overrides.web_search.source_overrides.filter((item) => item.source_id !== sourceId),
      },
    });
  }

  useEffect(() => {
    if (scope !== 'defaults' || !hasLoadedIpProxies || !config) return;
    const validProxyUrls = new Set(ipProxies.map((proxy) => proxy.url));
    config.web_search.sources.forEach((source, index) => {
      if (source.proxy_url && !validProxyUrls.has(source.proxy_url)) {
        updateSource(index, { proxy_url: null });
      }
    });
  }, [config, hasLoadedIpProxies, ipProxies, scope, updateSource]);

  function openProxyPicker(sourceIndex: number, field: ProxyPickerField) {
    setProxyPicker({ sourceIndex, field });
  }

  function closeProxyPicker() {
    setProxyPicker(null);
  }

  function selectProxy(url: string | null) {
    if (!proxyPicker) return;
    updateSource(proxyPicker.sourceIndex, { [proxyPicker.field]: url });
    closeProxyPicker();
  }

  function moveSource(index: number, direction: -1 | 1) {
    if (!config) return;
    const target = index + direction;
    if (target < 0 || target >= config.web_search.sources.length) return;
    const sources = [...config.web_search.sources];
    const [item] = sources.splice(index, 1);
    sources.splice(target, 0, item);
    if (scope === 'defaults') {
      updateWebSearch({
        sources: sources.map((source, sourceIndex) => ({
          ...source,
          priority: (sourceIndex + 1) * 10,
        })),
      });
      return;
    }
    if (!overrides) return;
    const nextSourceOverrides = sources.map((source, sourceIndex) => {
      const existing = overrides.web_search.source_overrides.find((item) => item.source_id === source.id);
      return {
        source_id: source.id,
        enabled: existing?.enabled ?? source.enabled,
        deep_enabled: existing?.deep_enabled ?? source.deep_enabled,
        extract_enabled: existing?.extract_enabled ?? source.extract_enabled,
        priority: (sourceIndex + 1) * 10,
        roles: existing?.roles ?? source.roles,
        proxy_url: existing?.proxy_url_override ? existing.proxy_url ?? null : source.proxy_url,
        proxy_url_override: true,
      };
    });
    updateOverrides({
      ...overrides,
      web_search: { ...overrides.web_search, source_overrides: nextSourceOverrides },
    });
  }

  function toggleSourceRole(index: number, role: WebSearchSourceRole, checked: boolean) {
    if (!config) return;
    const source = config.web_search.sources[index];
    const roles = checked
      ? Array.from(new Set([...source.roles, role]))
      : source.roles.filter((value) => value !== role);
    updateSource(index, { roles });
  }

  async function addKeys(sourceId: string, rawText: string) {
    if (!config || scope !== 'defaults') return;
    const values = rawText
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

    dispatchKeys({ type: 'setFlag', map: 'importingKeys', sourceId, value: true });
    try {
      const response = await importWebSearchKeys(sourceId, valuesToImport);
      const sources = config.web_search.sources.map((item) => (
        item.id === sourceId ? response.source : item
      ));
      dispatchKeys({ type: 'markDirty', sourceId });
      updateWebSearch({ sources });
      dispatchKeys({
        type: 'setSourceStatuses',
        sourceId,
        statuses: hydrateKeyStatuses([response.source])[sourceId] ?? {},
      });
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
      toast.error(userErrorMessage(error, {
        summary: t('tools.webSearch.saveFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      dispatchKeys({ type: 'setFlag', map: 'importingKeys', sourceId, value: false });
    }
  }

  function closeAddKeyDialog() {
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

    dispatchKeys({ type: 'setFlag', map: 'deletingKeys', sourceId, value: true });
    try {
      const response = await deleteWebSearchKey(sourceId, keyIndex, keyHint);
      const sources = config.web_search.sources.map((item) => (
        item.id === sourceId ? response.source : item
      ));
      dispatchKeys({ type: 'markDirty', sourceId });
      dispatchKeys({ type: 'setSelected', sourceId, indices: [] });
      dispatchKeys({
        type: 'setSourceStatuses',
        sourceId,
        statuses: hydrateKeyStatuses([response.source])[sourceId] ?? {},
      });
      updateWebSearch({ sources });
    } catch (error) {
      toast.error(userErrorMessage(error, {
        summary: t('tools.webSearch.saveFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      dispatchKeys({ type: 'setFlag', map: 'deletingKeys', sourceId, value: false });
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

    dispatchKeys({ type: 'setFlag', map: 'deletingKeys', sourceId, value: true });
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
      toast.error(userErrorMessage(error, {
        summary: t('tools.webSearch.saveFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      if (deletedCount > 0) {
        const sources = config.web_search.sources.map((item) => (
          item.id === sourceId ? latestSource : item
        ));
        dispatchKeys({ type: 'markDirty', sourceId });
        dispatchKeys({ type: 'setSelected', sourceId, indices: [] });
        dispatchKeys({
          type: 'setSourceStatuses',
          sourceId,
          statuses: hydrateKeyStatuses([latestSource])[sourceId] ?? {},
        });
        updateWebSearch({ sources });
      }
      dispatchKeys({ type: 'setFlag', map: 'deletingKeys', sourceId, value: false });
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
    dispatchKeys({ type: 'markDirty', sourceId });
    dispatchKeys({ type: 'setSelected', sourceId, indices: [] });
    dispatchKeys({ type: 'setSourceStatuses', sourceId, statuses: {} });
    updateWebSearch({ sources: nextSources });
  }

  function toggleKeySelection(sourceId: string, keyIndex: number, selected: boolean) {
    dispatchKeys({ type: 'toggleSelected', sourceId, keyIndex, selected });
  }

  function selectAllKeys(sourceId: string, count: number) {
    dispatchKeys({
      type: 'setSelected',
      sourceId,
      indices: Array.from({ length: count }, (_, index) => index),
    });
  }

  function requestPauseSourceKeys(sourceId: string) {
    if (!checkingKeys[sourceId]) return;
    checkPauseRequestedRef.current[sourceId] = true;
    dispatchKeys({
      type: 'patchCheckProgress',
      sourceId,
      patch: (progress) => ({ ...progress, pauseRequested: true }),
    });
  }

  async function checkSourceKeys(sourceId: string, indices: number[]) {
    if (checkingKeys[sourceId] || indices.length === 0) return;
    const queue = Array.from(new Set(indices.filter((index) => Number.isInteger(index) && index >= 0)));
    if (queue.length === 0) return;

    checkPendingIndicesRef.current[sourceId] = queue;
    checkPauseRequestedRef.current[sourceId] = false;
    dispatchKeys({
      type: 'setCheckProgress',
      sourceId,
      progress: {
        total: queue.length,
        completed: 0,
        healthy: 0,
        issues: 0,
        activeKeyIndex: queue[0] ?? null,
        paused: false,
        pauseRequested: false,
      },
    });
    dispatchKeys({ type: 'setFlag', map: 'checkingKeys', sourceId, value: true });

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
            dispatchKeys({ type: 'mergeKeyStatus', sourceId, result });
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
            dispatchKeys({
              type: 'patchCheckProgress',
              sourceId,
              patch: (progress) => ({
                ...progress,
                completed: progress.completed + 1,
                healthy: progress.healthy + (result.status === 'normal' ? 1 : 0),
                issues: progress.issues + (result.status === 'normal' ? 0 : 1),
              }),
            });
          }
        } catch (error) {
          if (!firstRequestError) {
            firstRequestError = error instanceof Error ? error.message : t('tools.webSearch.checkFailed');
          }
          dispatchKeys({
            type: 'patchCheckProgress',
            sourceId,
            patch: (progress) => ({
              ...progress,
              completed: progress.completed + 1,
              issues: progress.issues + 1,
            }),
          });
        } finally {
          checkPendingIndicesRef.current[sourceId] = (checkPendingIndicesRef.current[sourceId] ?? [])
            .slice(1);
          const remaining = checkPendingIndicesRef.current[sourceId] ?? [];
          dispatchKeys({
            type: 'patchCheckProgress',
            sourceId,
            patch: (progress) => ({
              ...progress,
              activeKeyIndex: checkPauseRequestedRef.current[sourceId] ? null : (remaining[0] ?? null),
            }),
          });
        }
      }
    } finally {
      const remaining = checkPendingIndicesRef.current[sourceId]?.length ?? 0;
      const paused = checkPauseRequestedRef.current[sourceId] && remaining > 0;
      dispatchKeys({
        type: 'patchCheckProgress',
        sourceId,
        patch: (progress) => ({ ...progress, activeKeyIndex: null, paused, pauseRequested: false }),
      });
      dispatchKeys({ type: 'setFlag', map: 'checkingKeys', sourceId, value: false });
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

      <div className="tool-settings-form terminal-settings-form">
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
                {webSearch.sources.map((source, index) => (
                  <SourceCard
                    key={source.id}
                    source={source}
                    index={index}
                    total={webSearch.sources.length}
                    isExpanded={Boolean(expandedSources[source.id])}
                    expandedSections={expandedSourceSections[source.id]}
                    selectedKeyIndices={selectedKeyIndices[source.id] ?? []}
                    sourceStatuses={keyStatuses[source.id] ?? {}}
                    checkProgress={checkProgress[source.id]}
                    hasPausedChecks={Boolean(
                      checkProgress[source.id]?.paused
                      && (checkPendingIndicesRef.current[source.id]?.length ?? 0) > 0,
                    )}
                    isChecking={Boolean(checkingKeys[source.id])}
                    isDeleting={Boolean(deletingKeys[source.id])}
                    isImporting={Boolean(importingKeys[source.id])}
                    showKeyManagement={!isAgentScope}
                    isOverridden={isAgentScope && Boolean(overrides?.web_search.source_overrides.some((item) => item.source_id === source.id))}
                    onResetSource={resetSource}
                    ipProxies={ipProxies}
                    onToggleExpanded={toggleSource}
                    onToggleSection={toggleSourceSection}
                    onUpdateSource={updateSource}
                    onMoveSource={moveSource}
                    onToggleRole={toggleSourceRole}
                    onSelectAllKeys={selectAllKeys}
                    onToggleKeySelection={toggleKeySelection}
                    onCheckKeys={(sourceId, indices) => void checkSourceKeys(sourceId, indices)}
                    onPauseCheck={requestPauseSourceKeys}
                    onResumeCheck={(sourceId) => void checkSourceKeys(
                      sourceId,
                      checkPendingIndicesRef.current[sourceId] ?? [],
                    )}
                    onDeleteSelectedKeys={(sourceId) => void deleteSelectedKeys(sourceId)}
                    onClearKeys={(sourceId) => void clearKeys(sourceId)}
                    onRemoveKey={(sourceId, keyIndex) => void removeKey(sourceId, keyIndex)}
                    onAddKey={setAddKeySourceId}
                    onOpenProxyPicker={openProxyPicker}
                  />
                ))}
            </div>
          </div>
        </section>
      </div>

      {proxyPicker ? (
        <ProxyPickerDialog
          value={proxyPickerValue ?? null}
          ipProxies={ipProxies}
          isLoading={isLoadingIpProxies}
          onSelect={selectProxy}
          onClose={closeProxyPicker}
          onOpenIpProxies={onOpenIpProxies}
        />
      ) : null}

      {addKeySource ? (
        <AddKeysDialog
          source={addKeySource}
          isAdding={isAddingKeys}
          onSubmit={(rawText) => void addKeys(addKeySource.id, rawText)}
          onClose={closeAddKeyDialog}
        />
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
