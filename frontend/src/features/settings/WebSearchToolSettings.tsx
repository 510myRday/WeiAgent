import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDown, ArrowLeft, ArrowUp, Plus, RefreshCw, Trash2 } from 'lucide-react';

import type {
  AgentToolsConfig,
  AgentToolsOverrides,
  AgentToolsSettings,
  WebSearchKeySelection,
  WebSearchProviderKind,
  WebSearchSourceConfig,
  WebSearchToolConfig,
} from '../../api';
import {
  readAgentToolsConfig,
  readToolDefaults,
  updateAgentToolsConfig,
  updateToolDefaults,
} from '../../api';
import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { useBeforeUnloadGuard } from '../../hooks';
import { AutoSaveIndicator, Button, IconButton, useFeedback } from '../../components/ui';

interface WebSearchToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
  onBack?: () => void;
}

type AutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

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
const PRESERVE_KEYS_MARKER = '__preserve__';

function defaultWebSearch(): WebSearchToolConfig {
  return {
    enabled: false,
    max_results: 5,
    timeout_ms: 15000,
    key_selection: 'sequential',
    sources: [
      { id: 'tavily', provider: 'tavily', enabled: true, api_keys: [], flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'exa', provider: 'exa', enabled: true, api_keys: [], flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'jina', provider: 'jina', enabled: true, api_keys: [], flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
      { id: 'grok', provider: 'grok', enabled: true, api_keys: [], flaresolverr_url: null, flaresolverr_proxy_url: null, proxy_url: null },
    ],
  };
}

function normalizeWebSearchSources(sources: WebSearchSourceConfig[]): WebSearchSourceConfig[] {
  return sources.map((source) => ({
    ...source,
    api_keys: source.api_keys ?? [],
    flaresolverr_url: source.flaresolverr_url ?? null,
    flaresolverr_proxy_url: source.flaresolverr_proxy_url ?? null,
    proxy_url: source.proxy_url ?? null,
  }));
}

function withWebSearch(config: AgentToolsConfig): AgentToolsConfig {
  const backendSources = config.web_search?.sources ?? [];
  const backendIds = new Set(backendSources.map((s) => s.id));
  const missingDefaults = defaultWebSearch().sources.filter(
    (defaultSource) => !backendIds.has(defaultSource.id),
  );
  return {
    ...config,
    web_search: {
      ...defaultWebSearch(),
      ...config.web_search,
      sources: normalizeWebSearchSources(backendSources).concat(missingDefaults),
    },
  };
}

function mergeToolsConfig(defaults: AgentToolsConfig, overrides: AgentToolsOverrides): AgentToolsConfig {
  return {
    max_llm_iterations: overrides.max_llm_iterations ?? defaults.max_llm_iterations,
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

export function WebSearchToolSettings({
  scope,
  agentId,
  onBack,
}: WebSearchToolSettingsProps) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const contextKey = scope === 'defaults' ? 'defaults' : `agent:${agentId ?? ''}`;
  const [defaults, setDefaults] = useState<AgentToolsConfig | null>(null);
  const [agentSettings, setAgentSettings] = useState<AgentToolsSettings | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [configLoadError, setConfigLoadError] = useState('');
  const [loadRequestKey, setLoadRequestKey] = useState(0);
  const [autoSaveStatus, setAutoSaveStatus] = useState<AutoSaveStatus>('idle');
  const [draftKeys, setDraftKeys] = useState<Record<string, string>>({});
  const [keysDirty, setKeysDirty] = useState<Record<string, boolean>>({});

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
        setKeysDirty({});
        const serialized = JSON.stringify(normalized);
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
      const serialized = JSON.stringify(payload);
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
    updateWebSearch({ sources });
  }

  function addKey(sourceId: string) {
    if (!config || scope !== 'defaults') return;
    const value = (draftKeys[sourceId] ?? '').trim();
    if (!value) return;
    const sources = config.web_search.sources.map((source) => {
      if (source.id !== sourceId) return source;
      return { ...source, api_keys: [...source.api_keys.filter((k) => k !== PRESERVE_KEYS_MARKER), value] };
    });
    setKeysDirty((current) => ({ ...current, [sourceId]: true }));
    setDraftKeys((current) => ({ ...current, [sourceId]: '' }));
    updateWebSearch({ sources });
  }

  function removeKey(sourceId: string, keyIndex: number) {
    if (!config || scope !== 'defaults') return;
    const sources = config.web_search.sources.map((source) => {
      if (source.id !== sourceId) return source;
      return {
        ...source,
        api_keys: source.api_keys.filter((_, index) => index !== keyIndex),
      };
    });
    setKeysDirty((current) => ({ ...current, [sourceId]: true }));
    updateWebSearch({ sources });
  }

  function clearKeys(sourceId: string) {
    if (!config || scope !== 'defaults') return;
    const sources = config.web_search.sources.map((source) => (
      source.id === sourceId ? { ...source, api_keys: [] } : source
    ));
    setKeysDirty((current) => ({ ...current, [sourceId]: true }));
    updateWebSearch({ sources });
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
                  const emptyLabel = t(source.provider === 'grok' ? 'tools.webSearch.noSso' : 'tools.webSearch.noKeys');
                  const statusLabel = keyCount === 0
                    ? emptyLabel
                    : t('tools.webSearch.keyCount', { count: keyCount });

                  return (
                    <div
                      key={source.id}
                      className={`web-search-source ${source.enabled ? 'is-enabled' : 'is-disabled'}`}
                    >
                      <div className="web-search-source__header">
                        <div className="web-search-source__identity">
                          <span
                            className="web-search-source__priority"
                            title={t('tools.webSearch.sourcePriority', { index: String(index + 1) })}
                          >
                            {index + 1}
                          </span>
                          <div className="web-search-source__meta">
                            <strong>{providerLabel(source.provider, t)}</strong>
                            <small>{statusLabel}</small>
                          </div>
                        </div>
                        <div className="web-search-source__actions">
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
                          <label
                            className="toggle-switch web-search-source__toggle"
                            title={source.enabled ? t('tools.enabled') : t('tools.disabled')}
                          >
                            <input
                              type="checkbox"
                              checked={source.enabled}
                              onChange={(e) => updateSource(index, { enabled: e.target.checked })}
                            />
                            <span className="toggle-slider" />
                          </label>
                        </div>
                      </div>

                      <div className="web-search-source__body">
                        <div className="web-search-keys">
                          <div className="web-search-keys__toolbar">
                            <span className="web-search-keys__label">{t('tools.webSearch.keysLabel')}</span>
                            {keyCount > 0 ? (
                              <button
                                type="button"
                                className="web-search-keys__clear"
                                onClick={() => clearKeys(source.id)}
                              >
                                {t('tools.webSearch.clearKeys')}
                              </button>
                            ) : null}
                          </div>

                          {keyCount === 0 ? (
                            <div className="web-search-keys__empty">{emptyLabel}</div>
                          ) : (
                            <ul className="web-search-key-tags">
                              {source.api_keys.map((key, keyIndex) => (
                                <li key={`${source.id}-${keyIndex}`} className="web-search-key-chip">
                                  <code>{key}</code>
                                  <IconButton
                                    type="button"
                                    variant="ghost"
                                    className="web-search-key-chip__remove"
                                    label={t('tools.webSearch.removeKey')}
                                    onClick={() => removeKey(source.id, keyIndex)}
                                  >
                                    <Trash2 size={13} />
                                  </IconButton>
                                </li>
                              ))}
                            </ul>
                          )}

                          <div className="web-search-key-add">
                            <input
                              id={`web-search-key-input-${source.id}`}
                              type="password"
                              className="pattern-input web-search-key-add__input"
                              value={draftKeys[source.id] ?? ''}
                              placeholder={t(source.provider === 'grok' ? 'tools.webSearch.ssoPlaceholder' : 'tools.webSearch.keyPlaceholder')}
                              autoComplete="off"
                              spellCheck={false}
                              onChange={(e) => setDraftKeys((current) => ({ ...current, [source.id]: e.target.value }))}
                              onKeyDown={(e) => {
                                if (e.key === 'Enter') {
                                  e.preventDefault();
                                  addKey(source.id);
                                }
                              }}
                            />
                            <Button
                              type="button"
                              variant="secondary"
                              className="web-search-key-add__btn"
                              icon={<Plus size={15} />}
                              onClick={() => addKey(source.id)}
                              disabled={!(draftKeys[source.id] ?? '').trim()}
                            >
                              {t('tools.webSearch.addKey')}
                            </Button>
                          </div>
                        </div>

                        {source.provider === 'grok' ? (
                          <div className="web-search-grok-extras">
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
                          </div>
                        ) : null}
                      </div>
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
