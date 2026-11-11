import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, Folder, FolderOpen, LoaderCircle, Pencil, Plus, RefreshCw, Send, ShieldCheck, Square, Trash2, X } from 'lucide-react';

import type { AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings, EnvironmentRuntimeStatus, RuntimeComponentKind, RuntimeComponentStatus, ToolProcessInfo } from '../../toolTypes';
import {
  installEnvironmentComponent,
  interruptTerminalSession,
  listToolProcesses,
  pickDirectory,
  readAgentToolsConfig,
  readEnvironmentRuntime,
  readToolDefaults,
  refreshEnvironmentComponent,
  resizeTerminalSession,
  stopToolProcess,
  updateAgentToolsConfig,
  updateToolDefaults,
  writeTerminalSession,
} from '../../api';
import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { userErrorMessage } from '../../utils/userError';
import { displayPath } from '../../utils/displayPath';
import { updateToolProcessCache, usePageVisibility, useSerializedAutoSave } from '../../hooks';
import { AutoSaveIndicator, Button, IconButton, InheritToggle, useFeedback } from '../../components/ui';
import { PermissionModeControl, type PermissionMode } from './PermissionModeControl';
import { WebSearchToolSettings } from './WebSearchToolSettings';
import { TaskPlanningToolSettings } from './TaskPlanningToolSettings';
import { McpAgentToolSettings, type McpServerSelection } from '../tools/McpAgentToolSettings';
import { mergeToolsConfig } from './web-search/utils';
import {
  RuntimeComponentRow,
  ToolRecordRow,
  agentName,
  configWithClearedGrants,
  normalizeRuntimeComponents,
  serializeDefaultConfig,
} from './agentToolsShared';

interface TerminalToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
  onOpenIpProxies?: () => void;
}

type OverrideField = 'permission_mode' | 'default_timeout_ms' | 'allowed_cwd_roots';
type AgentToolView = 'list' | 'terminal' | 'web_search' | 'task_planning' | 'mcp';
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

const PROCESS_POLL_RETRY_DELAY_MS = 1500;
const PROCESS_POLL_MAX_RETRY_DELAY_MS = 30000;
const RUNTIME_STATUS_POLL_INTERVAL_MS = 60_000;
const MAX_LLM_ITERATIONS = 999;

export function TerminalToolSettings({
  scope,
  agentId,
  agents = [],
  onOpenIpProxies,
}: TerminalToolSettingsProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const contextKey = scope === 'defaults' ? 'defaults' : `agent:${agentId ?? ''}`;
  const [defaults, setDefaults] = useState<AgentToolsConfig | null>(null);
  const [agentSettings, setAgentSettings] = useState<AgentToolsSettings | null>(null);
  const [runtime, setRuntime] = useState<EnvironmentRuntimeStatus | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [configLoadError, setConfigLoadError] = useState('');
  const [runtimeLoadError, setRuntimeLoadError] = useState('');
  const [isRetryingRuntime, setIsRetryingRuntime] = useState(false);
  const [loadRequestKey, setLoadRequestKey] = useState(0);
  const [agentToolView, setAgentToolView] = useState<AgentToolView>('list');
  const [activeMcpServer, setActiveMcpServer] = useState<McpServerSelection | null>(null);
  const [installingRuntimeComponent, setInstallingRuntimeComponent] = useState<RuntimeComponentKind | null>(null);
  const [refreshingRuntimeComponent, setRefreshingRuntimeComponent] = useState<RuntimeComponentKind | null>(null);
  const [processes, setProcesses] = useState<ToolProcessInfo[]>([]);
  const [processLoadError, setProcessLoadError] = useState('');
  const [processRequestKey, setProcessRequestKey] = useState(0);
  const [isStoppingProcess, setIsStoppingProcess] = useState('');
  const [isSendingToProcess, setIsSendingToProcess] = useState('');
  const [sessionInputs, setSessionInputs] = useState<Record<string, string>>({});
  const [rootInput, setRootInput] = useState('');
  const [isPickingRoot, setIsPickingRoot] = useState(false);
  const isPageVisible = usePageVisibility();
  const loadedContextRef = useRef('');
  const config = scope === 'defaults' ? defaults : agentSettings?.effective ?? null;
  const overrides = agentSettings?.overrides;
  const autoSaveItem = useMemo<PendingSave | null>(() => {
    if (isLoading || !config || loadedContextRef.current !== contextKey) return null;
    if (scope === 'defaults') {
      return {
        contextKey,
        serialized: serializeDefaultConfig(config),
        scope: 'defaults',
        config: configWithClearedGrants(config),
      };
    }
    if (agentId && overrides) {
      return {
        contextKey,
        serialized: JSON.stringify(overrides),
        scope: 'agent',
        agentId,
        overrides,
      };
    }
    return null;
  }, [agentId, config, contextKey, isLoading, overrides, scope]);
  const {
    status: autoSaveStatus,
    retryNow: retryAutoSaveNow,
    markPersisted,
    resetContext,
  } = useSerializedAutoSave<PendingSave>({
    contextKey,
    enabled: Boolean(autoSaveItem),
    item: autoSaveItem,
    persist: async (item) => {
      if (item.scope === 'defaults') {
        await updateToolDefaults(item.config);
        return;
      }
      await updateAgentToolsConfig(item.agentId, item.overrides);
    },
    onError: (error, attempt) => {
      if (attempt === 1) {
        toast.error(userErrorMessage(error, {
          summary: t('tools.terminal.saveFailed'),
          recovery: t('error.retryConnection'),
        }));
      }
    },
  });
  const isMountedRef = useRef(true);
  const installWatchRef = useRef<{
    kind: RuntimeComponentKind;
    wasUpdate: boolean;
  } | null>(null);

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    loadedContextRef.current = '';
    setIsLoading(true);
    setConfigLoadError('');
    setRuntimeLoadError('');
    setRuntime(null);
    if (scope === 'defaults') {
      setDefaults(null);
    } else {
      setAgentSettings(null);
    }
    resetContext(contextKey);
    setAgentToolView('list');
    const configRequest = scope === 'defaults'
      ? readToolDefaults().then((next) => {
        if (!cancelled) {
          loadedContextRef.current = contextKey;
          markPersisted(contextKey, serializeDefaultConfig(next));
          setDefaults(next);
        }
      })
      : agentId
        ? readAgentToolsConfig(agentId).then((next) => {
          if (!cancelled) {
            loadedContextRef.current = contextKey;
            markPersisted(contextKey, JSON.stringify(next.overrides));
            setAgentSettings(next);
          }
        })
        : Promise.reject(new Error(t('tools.terminal.agentRequired')));
    const load = Promise.allSettled([
      configRequest,
      readEnvironmentRuntime().then((next) => {
        if (!cancelled) setRuntime(next);
      }),
    ]);
    load
      .then(([configResult, runtimeResult]) => {
        if (cancelled) return;
        if (configResult.status === 'rejected') {
          const message = configResult.reason instanceof Error
            ? configResult.reason.message
            : t('tools.terminal.loadFailed');
          setConfigLoadError(message);
          toast.error(message);
        }
        if (runtimeResult.status === 'rejected') {
          const message = runtimeResult.reason instanceof Error
            ? runtimeResult.reason.message
            : t('tools.terminal.runtimeRefreshFailed');
          setRuntimeLoadError(message);
          toast.error(message);
        }
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, contextKey, loadRequestKey, markPersisted, resetContext, scope, t, toast]);

  useEffect(() => {
    if (scope !== 'defaults') return;
    const timer = window.setInterval(() => {
      void readEnvironmentRuntime()
        .then((next) => {
          if (!isMountedRef.current) return;
          setRuntime(next);
        })
        .catch(() => {
          // The initial load and manual retry provide the visible error state.
        });
    }, RUNTIME_STATUS_POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [contextKey, scope]);


  useEffect(() => {
    if (scope !== 'defaults') {
      setProcesses([]);
      setProcessLoadError('');
      return;
    }
    if (!isPageVisible) return;
    let cancelled = false;
    let timer: number | undefined;
    let failureCount = 0;
    const refresh = async () => {
      try {
        const next = await listToolProcesses();
        if (cancelled) return;
        failureCount = 0;
        setProcesses(next);
        setProcessLoadError('');
        timer = window.setTimeout(refresh, next.some((process) => process.status === 'running') ? 1500 : 4000);
      } catch (error) {
        if (cancelled) return;
        failureCount += 1;
        const message = error instanceof Error ? error.message : t('tools.terminal.processLoadFailed');
        setProcessLoadError(message);
        if (failureCount === 1) toast.error(message);
        const retryDelay = Math.min(
          PROCESS_POLL_MAX_RETRY_DELAY_MS,
          PROCESS_POLL_RETRY_DELAY_MS * (2 ** Math.min(failureCount - 1, 4)),
        );
        timer = window.setTimeout(refresh, retryDelay);
      }
    };
    void refresh();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [isPageVisible, processRequestKey, scope, t, toast]);

  const runtimeInstalling = runtime?.installing === true;
  useEffect(() => {
    if (!runtimeInstalling || !isPageVisible) return;
    let cancelled = false;
    let timer: number | undefined;
    let failureCount = 0;
    const refresh = async () => {
      try {
        const next = await readEnvironmentRuntime();
        if (cancelled) return;
        failureCount = 0;
        setRuntime(next);
        setRuntimeLoadError('');
        const watched = installWatchRef.current;
        if (watched) {
          const component = normalizeRuntimeComponents(next)
            .find((item) => item.kind === watched.kind);
          if (component && !component.installing) {
            installWatchRef.current = null;
            const name = t(`tools.terminal.runtimeComponent.${component.kind}`);
            if (component.error) {
              toast.error(component.error);
            } else {
              toast.success(t(
                watched.wasUpdate ? 'notification.runtimeUpdated' : 'notification.runtimeInstalled',
                { name },
              ));
            }
          }
        }
        if (next.installing) {
          timer = window.setTimeout(refresh, 1000);
        }
      } catch {
        if (cancelled) return;
        failureCount += 1;
        const retryDelay = Math.min(10000, 1000 * (2 ** Math.min(failureCount - 1, 3)));
        timer = window.setTimeout(refresh, retryDelay);
      }
    };
    timer = window.setTimeout(refresh, 500);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [isPageVisible, runtimeInstalling, t, toast]);

  async function retryRuntimeLoad() {
    setIsRetryingRuntime(true);
    setRuntimeLoadError('');
    try {
      setRuntime(await readEnvironmentRuntime());
    } catch (error) {
      const message = error instanceof Error ? error.message : t('tools.terminal.runtimeRefreshFailed');
      setRuntimeLoadError(message);
      toast.error(message);
    } finally {
      setIsRetryingRuntime(false);
    }
  }

  async function handleStopProcess(process: ToolProcessInfo) {
    setIsStoppingProcess(process.process_id);
    try {
      const stopped = await stopToolProcess(process.process_id, process.agent_id);
      updateToolProcessCache(stopped);
      setProcesses((current) => current.map((item) => item.process_id === process.process_id ? stopped : item));
    } catch (error) {
      toast.error(userErrorMessage(error, {
        summary: t('tools.terminal.processStopFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      setIsStoppingProcess('');
    }
  }

  async function handleSendToSession(process: ToolProcessInfo, submit = false) {
    const value = sessionInputs[process.process_id] ?? '';
    if (!value && !submit) return;
    setIsSendingToProcess(process.process_id);
    try {
      const next = await writeTerminalSession(
        process.process_id,
        process.agent_id,
        `${value}${submit ? (navigator.userAgent.includes('Windows') ? '\r\n' : '\n') : ''}`,
      );
      updateToolProcessCache(next);
      setProcesses((current) => current.map((item) => item.process_id === process.process_id ? next : item));
      setSessionInputs((current) => ({ ...current, [process.process_id]: '' }));
    } catch (error) {
      toast.error(userErrorMessage(error, {
        summary: t('tools.terminal.sessionSendFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      setIsSendingToProcess('');
    }
  }

  async function handleInterruptSession(process: ToolProcessInfo) {
    try {
      const next = await interruptTerminalSession(process.process_id, process.agent_id);
      updateToolProcessCache(next);
      setProcesses((current) => current.map((item) => item.process_id === process.process_id ? next : item));
    } catch (error) {
      toast.error(userErrorMessage(error, {
        summary: t('tools.terminal.sessionInterruptFailed'),
        recovery: t('error.retryConnection'),
      }));
    }
  }

  async function handleResizeSession(process: ToolProcessInfo, element: HTMLPreElement) {
    const styles = window.getComputedStyle(element);
    const fontSize = Number.parseFloat(styles.fontSize) || 13;
    const lineHeight = Number.parseFloat(styles.lineHeight) || fontSize * 1.4;
    const cols = Math.max(20, Math.floor(element.clientWidth / (fontSize * 0.6)));
    const rows = Math.max(5, Math.floor(element.clientHeight / lineHeight));
    try {
      const next = await resizeTerminalSession(process.process_id, process.agent_id, rows, cols);
      updateToolProcessCache(next);
    } catch {
      // A process may finish while the view is resizing.
    }
  }

  async function handleInstallRuntime(component: RuntimeComponentStatus) {
    const wasUpdate = component.available;
    setInstallingRuntimeComponent(component.kind);
    try {
      const next = await installEnvironmentComponent(component.kind);
      setRuntime(next);
      const nextComponent = normalizeRuntimeComponents(next)
        .find((item) => item.kind === component.kind);
      const name = t(`tools.terminal.runtimeComponent.${component.kind}`);
      if (nextComponent?.installing) {
        installWatchRef.current = { kind: component.kind, wasUpdate };
        toast.info(t(
          wasUpdate ? 'notification.runtimeUpdateStarted' : 'notification.runtimeInstallStarted',
          { name },
        ));
      } else if (nextComponent?.error) {
        toast.error(nextComponent.error);
      } else {
        toast.success(t(
          wasUpdate ? 'notification.runtimeUpdated' : 'notification.runtimeInstalled',
          { name },
        ));
      }
    } catch (error) {
      installWatchRef.current = null;
      toast.error(userErrorMessage(error, {
        summary: t('tools.terminal.runtimeInstallFailed'),
        recovery: t('error.retryConnection'),
      }));
      try {
        setRuntime(await readEnvironmentRuntime());
      } catch {
        // Keep the original installation failure as the visible error.
      }
    } finally {
      setInstallingRuntimeComponent(null);
    }
  }

  async function handleRefreshRuntime(component: RuntimeComponentKind) {
    setRefreshingRuntimeComponent(component);
    try {
      setRuntime(await refreshEnvironmentComponent(component));
    } catch (error) {
      toast.error(userErrorMessage(error, {
        summary: t('tools.terminal.runtimeRefreshFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      setRefreshingRuntimeComponent(null);
    }
  }

  async function selectFullAccess() {
    const confirmed = await confirm({
      title: t('tools.terminal.fullAccessTitle'),
      message: t('tools.terminal.fullAccessConfirm'),
      confirmLabel: t('tools.terminal.fullAccess'),
      tone: 'warning',
    });
    if (!confirmed) return;
    updateTerminal({ permission_mode: 'full_access' });
  }

  function handlePermissionModeChange(mode: PermissionMode) {
    if (mode === 'request_approval') {
      updateTerminal({ permission_mode: mode });
      return;
    }
    void selectFullAccess();
  }

  function updateGlobalConfig(next: AgentToolsConfig) {
    setDefaults(next);
  }

  function updateOverrides(next: AgentToolsOverrides) {
    setAgentSettings((current) => current ? {
      ...current,
      overrides: next,
      effective: mergeToolsConfig(current.defaults, next),
    } : current);
  }

  function updateMaxIterations(value: number) {
    if (!config) return;
    if (scope === 'defaults') {
      updateGlobalConfig({ ...config, max_llm_iterations: value });
    } else if (overrides) {
      updateOverrides({ ...overrides, max_llm_iterations: value });
    }
  }

  function updateTerminal(patch: Partial<AgentToolsConfig['terminal']>) {
    if (!config) return;
    if (scope === 'defaults') {
      updateGlobalConfig({ ...config, terminal: { ...config.terminal, ...patch } });
      return;
    }
    if (!overrides) return;
    const nextTerminal = { ...overrides.terminal };
    for (const [key, value] of Object.entries(patch)) {
      (nextTerminal as Record<string, unknown>)[key] = value;
    }
    updateOverrides({ ...overrides, terminal: nextTerminal });
  }

  function updateTaskPlanning(enabled: boolean) {
    if (!config) return;
    if (scope === 'defaults') {
      updateGlobalConfig({ ...config, task_planning: { ...config.task_planning, enabled } });
    } else if (overrides) {
      updateOverrides({
        ...overrides,
        task_planning: { ...overrides.task_planning, enabled },
      });
    }
  }

  function setTerminalEnabledInherited(inherited: boolean) {
    if (!overrides || !config) return;
    const terminalOverrides = { ...overrides.terminal };
    if (inherited) delete terminalOverrides.enabled;
    else terminalOverrides.enabled = config.terminal.enabled;
    updateOverrides({ ...overrides, terminal: terminalOverrides });
  }

  function setTaskPlanningEnabledInherited(inherited: boolean) {
    if (!overrides || !config) return;
    const taskPlanningOverrides = { ...overrides.task_planning };
    if (inherited) delete taskPlanningOverrides.enabled;
    else taskPlanningOverrides.enabled = config.task_planning.enabled;
    updateOverrides({ ...overrides, task_planning: taskPlanningOverrides });
  }

  function setWebSearchEnabledInherited(inherited: boolean) {
    if (!overrides || !config) return;
    const webSearchOverrides = { ...overrides.web_search };
    if (inherited) delete webSearchOverrides.enabled;
    else webSearchOverrides.enabled = config.web_search.enabled;
    updateOverrides({ ...overrides, web_search: webSearchOverrides });
  }

  function setMaxIterationsInherited(inherited: boolean) {
    if (!overrides || !config) return;
    const next = { ...overrides };
    if (inherited) {
      delete next.max_llm_iterations;
    } else {
      next.max_llm_iterations = config.max_llm_iterations;
    }
    updateOverrides(next);
  }

  function setTerminalInherited(field: OverrideField, inherited: boolean) {
    if (!overrides || !config) return;
    const terminal = { ...overrides.terminal };
    if (inherited) {
      delete terminal[field];
    } else {
      if (field === 'allowed_cwd_roots') terminal.allowed_cwd_roots = [...config.terminal.allowed_cwd_roots];
      if (field === 'permission_mode') terminal.permission_mode = config.terminal.permission_mode;
      if (field === 'default_timeout_ms') terminal.default_timeout_ms = config.terminal.default_timeout_ms;
    }
    updateOverrides({ ...overrides, terminal });
  }

  function addRoot() {
    const value = rootInput.trim();
    if (!value || !config || config.terminal.allowed_cwd_roots.includes(value)) return;
    updateTerminal({ allowed_cwd_roots: [...config.terminal.allowed_cwd_roots, value] });
    setRootInput('');
  }

  async function browseForRoot() {
    if (!config || isPickingRoot) return;
    setIsPickingRoot(true);
    try {
      const roots = config.terminal.allowed_cwd_roots;
      const initialDirectory = rootInput.trim() || roots[roots.length - 1];
      const selection = await pickDirectory(initialDirectory);
      if (selection.path) setRootInput(selection.path);
    } catch (error) {
      toast.error(userErrorMessage(error, {
        summary: t('tools.terminal.browseFailed'),
        recovery: t('error.retryConnection'),
      }));
    } finally {
      setIsPickingRoot(false);
    }
  }

  function removeRoot(index: number) {
    if (!config) return;
    updateTerminal({
      allowed_cwd_roots: config.terminal.allowed_cwd_roots.filter((_, itemIndex) => itemIndex !== index),
    });
  }

  function removeApprovalGrant(index: number) {
    if (!overrides) return;
    updateOverrides({
      ...overrides,
      terminal: {
        ...overrides.terminal,
        approval_grants: overrides.terminal.approval_grants.filter((_, grantIndex) => grantIndex !== index),
      },
    });
  }

  if (isLoading || !config) {
    return (
      <div className="terminal-loading-state">
        {isLoading ? <div className="loading-spinner" /> : null}
        {isLoading ? <p>{t('tools.terminal.loading')}</p> : null}
        {!isLoading && configLoadError ? (
          <div className="terminal-load-error" role="alert">
            <strong>{t('tools.terminal.loadFailed')}</strong>
            <span>{configLoadError}</span>
            <Button variant="secondary" type="button" onClick={() => setLoadRequestKey((current) => current + 1)} icon={<RefreshCw size={15} />}>
              {t('common.retry')}
            </Button>
          </div>
        ) : null}
        {!isLoading && !configLoadError ? <p>{t('tools.terminal.noConfig')}</p> : null}
      </div>
    );
  }

  const terminal = config.terminal;
  const isAgentScope = scope === 'agent';
  const maxIterationsInherited = isAgentScope && overrides?.max_llm_iterations === undefined;
  const terminalEnabledInherited = isAgentScope && overrides?.terminal.enabled === undefined;
  const terminalPermissionInherited = isAgentScope && overrides?.terminal.permission_mode === undefined;
  const webSearchEnabledInherited = isAgentScope && overrides?.web_search?.enabled === undefined;
  const taskPlanningEnabledInherited = isAgentScope && overrides?.task_planning.enabled === undefined;
  const fieldInherited = (field: OverrideField) => isAgentScope && overrides?.terminal[field] === undefined;
  const environmentDetected = runtime?.available === true;
  const runtimeComponents = normalizeRuntimeComponents(runtime);
  const normalizedRootInput = rootInput.trim();
  const rootAlreadyAdded = terminal.allowed_cwd_roots.includes(normalizedRootInput);

  if (isAgentScope && agentToolView === 'list') {
    return (
      <div className="terminal-tool-settings-v2 agent-tools-overview">
        <div className="agent-tools-overview__header">
          <div>
            <h3>{t('tools.agentToolsTitle')}</h3>
            <p>{t('tools.agentToolsDesc')}</p>
          </div>
          <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryAutoSaveNow} />
        </div>
        <div className="agent-tool-list">
          <div className="agent-tool-row">
            <div className="agent-tool-row__body">
              <div className="agent-tool-row__title">
                <strong>{t('tools.terminal.name')}</strong>
                <span className="mini-status-badge">
                  <ShieldCheck size={12} />
                  {t('tools.builtinBadge')}
                </span>
              </div>
              <small>{t('tools.terminal.description')}</small>
              <span className="agent-tool-row__source">
                {t('tools.terminal.agentOverviewInheritance', {
                  enabled: t(terminalEnabledInherited ? 'tools.inheritsDefault' : 'tools.agentOverride'),
                  permission: t(terminalPermissionInherited ? 'tools.inheritsDefault' : 'tools.agentOverride'),
                })}
              </span>
            </div>
            <div className="agent-tool-row__actions">
              <InheritToggle inherited={Boolean(terminalEnabledInherited)} onChange={setTerminalEnabledInherited} label={t('tools.terminal.inheritDefault')} />
              <label className={`toggle-switch${terminalEnabledInherited ? ' is-disabled' : ''}`} title={terminal.enabled ? t('tools.enabled') : t('tools.disabled')}>
                <input
                  type="checkbox"
                  aria-label={t('tools.terminal.enabled')}
                  checked={terminal.enabled}
                  disabled={Boolean(terminalEnabledInherited)}
                  onChange={(event) => updateTerminal({ enabled: event.target.checked })}
                />
                <span className="toggle-slider" />
              </label>
              <IconButton
                type="button"
                variant="ghost"
                className="agent-tool-edit"
                onClick={() => setAgentToolView('terminal')}
                disabled={Boolean(terminalEnabledInherited)}
                label={t('tools.editTool')}
              >
                <Pencil size={17} />
              </IconButton>
            </div>
          </div>
          <div className="agent-tool-row">
            <div className="agent-tool-row__body">
              <div className="agent-tool-row__title">
                <strong>{t('tools.webSearch.name')}</strong>
                <span className="mini-status-badge">
                  <ShieldCheck size={12} />
                  {t('tools.builtinBadge')}
                </span>
              </div>
              <small>{t('tools.webSearch.description')}</small>
              <span className="agent-tool-row__source">{webSearchEnabledInherited ? t('tools.inheritsDefault') : t('tools.agentOverride')}</span>
            </div>
            <div className="agent-tool-row__actions">
              <InheritToggle inherited={Boolean(webSearchEnabledInherited)} onChange={setWebSearchEnabledInherited} label={t('tools.terminal.inheritDefault')} />
              <label className={`toggle-switch${webSearchEnabledInherited ? ' is-disabled' : ''}`} title={config.web_search?.enabled ? t('tools.enabled') : t('tools.disabled')}>
                <input
                  type="checkbox"
                  aria-label={t('tools.webSearch.enabled')}
                  checked={Boolean(config.web_search?.enabled)}
                  disabled={Boolean(webSearchEnabledInherited)}
                  onChange={(event) => {
                    if (!overrides) return;
                    updateOverrides({
                      ...overrides,
                      web_search: {
                        ...overrides.web_search,
                        enabled: event.target.checked,
                      },
                    });
                  }}
                />
                <span className="toggle-slider" />
              </label>
              <IconButton
                type="button"
                variant="ghost"
                className="agent-tool-edit"
                onClick={() => setAgentToolView('web_search')}
                disabled={Boolean(webSearchEnabledInherited)}
                label={t('tools.editTool')}
              >
                <Pencil size={17} />
              </IconButton>
            </div>
          </div>
          <div className="agent-tool-row">
            <div className="agent-tool-row__body">
              <div className="agent-tool-row__title">
                <strong>{t('tools.taskPlanning.name')}</strong>
                <span className="mini-status-badge">
                  <ShieldCheck size={12} />
                  {t('tools.builtinBadge')}
                </span>
              </div>
              <small>{t('tools.taskPlanning.description')}</small>
              <span className="agent-tool-row__source">{taskPlanningEnabledInherited ? t('tools.inheritsDefault') : t('tools.agentOverride')}</span>
            </div>
            <div className="agent-tool-row__actions">
              <InheritToggle inherited={Boolean(taskPlanningEnabledInherited)} onChange={setTaskPlanningEnabledInherited} label={t('tools.terminal.inheritDefault')} />
              <label className={`toggle-switch${taskPlanningEnabledInherited ? ' is-disabled' : ''}`} title={config.task_planning.enabled ? t('tools.enabled') : t('tools.disabled')}>
                <input
                  type="checkbox"
                  aria-label={t('tools.taskPlanning.enabled')}
                  checked={config.task_planning.enabled}
                  disabled={Boolean(taskPlanningEnabledInherited)}
                  onChange={(event) => updateTaskPlanning(event.target.checked)}
                />
                <span className="toggle-slider" />
              </label>
              <IconButton
                type="button"
                variant="ghost"
                className="agent-tool-edit"
                onClick={() => setAgentToolView('task_planning')}
                disabled={Boolean(taskPlanningEnabledInherited)}
                label={t('tools.editTool')}
              >
                <Pencil size={17} />
              </IconButton>
            </div>
          </div>
          <McpAgentToolSettings
            agentId={agentId!}
            view="summary"
            onEdit={(selection) => {
              setActiveMcpServer(selection);
              setAgentToolView('mcp');
            }}
          />
        </div>
      </div>
    );
  }

  if (isAgentScope && agentToolView === 'web_search') {
    return (
      <WebSearchToolSettings
        scope="agent"
        agentId={agentId}
        onBack={() => setAgentToolView('list')}
        onOpenIpProxies={onOpenIpProxies}
      />
    );
  }

  if (isAgentScope && agentToolView === 'task_planning') {
    return (
      <TaskPlanningToolSettings
        scope="agent"
        agentId={agentId}
        onBack={() => setAgentToolView('list')}
      />
    );
  }

  if (isAgentScope && agentToolView === 'mcp') {
    return (
      <McpAgentToolSettings
        agentId={agentId!}
        view="edit"
        selection={activeMcpServer ?? undefined}
        onBack={() => setAgentToolView('list')}
      />
    );
  }

  return (
    <div className={`terminal-tool-settings-v2 ${isAgentScope ? 'is-agent-scope' : 'is-default-scope'}`}>
      {isAgentScope ? (
        <div className="tool-config-heading">
          <div className="tool-config-heading__main">
            <IconButton
              type="button"
              variant="ghost"
              className="tool-config-back"
              onClick={() => setAgentToolView('list')}
              label={t('tools.backToList')}
            >
              <ArrowLeft size={18} />
            </IconButton>
            <div>
            <h2>{isAgentScope ? t('tools.terminal.agentConfigTitle') : t('tools.terminal.defaultsTitle')}</h2>
            <p>{isAgentScope ? t('tools.terminal.agentConfigDesc') : t('tools.terminal.defaultsDesc')}</p>
            </div>
          </div>
          <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryAutoSaveNow} />
        </div>
      ) : null}

      <div className="tool-settings-form terminal-settings-form">
      <section className="settings-card basic-settings-card">
        <div className="card-header">
          <h3>{t('tools.terminal.basicSettings')}</h3>
          {!isAgentScope ? (
            <span className="terminal-settings-save-status">
              <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryAutoSaveNow} />
            </span>
          ) : null}
        </div>
        <div className="card-body">
          {!isAgentScope ? (
            <div className="setting-row">
              <div className="setting-info"><label>{t('tools.terminal.enabled')}</label><p className="setting-desc">{t('tools.terminal.enabledDesc')}</p></div>
              <label className="toggle-switch">
                <input type="checkbox" aria-label={t('tools.terminal.enabled')} checked={terminal.enabled} onChange={(event) => updateTerminal({ enabled: event.target.checked })} />
                <span className="toggle-slider" />
              </label>
            </div>
          ) : null}
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.defaultTimeout')}</label><p className="setting-desc">{t('tools.terminal.timeoutDesc')}</p></div>
            {isAgentScope ? <InheritToggle inherited={fieldInherited('default_timeout_ms')} onChange={(value) => setTerminalInherited('default_timeout_ms', value)} label={t('tools.terminal.inheritDefault')} /> : null}
            <div className="timeout-input">
              <input type="number" min={1} disabled={fieldInherited('default_timeout_ms')} value={Math.max(1, Math.round(terminal.default_timeout_ms / 1000))} onChange={(event) => updateTerminal({ default_timeout_ms: Math.max(1, Number(event.target.value) || 1) * 1000 })} />
              <span>s</span>
            </div>
          </div>
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.maxLlmIterations')}</label><p className="setting-desc">{t('tools.terminal.maxLlmIterationsDesc')}</p></div>
            {isAgentScope ? <InheritToggle inherited={maxIterationsInherited} onChange={setMaxIterationsInherited} label={t('tools.terminal.inheritDefault')} /> : null}
            <div className="timeout-input">
              <input
                type="number"
                min={1}
                max={MAX_LLM_ITERATIONS}
                disabled={maxIterationsInherited}
                value={Math.min(MAX_LLM_ITERATIONS, Math.max(1, config.max_llm_iterations || 1))}
                onChange={(event) => updateMaxIterations(Math.min(MAX_LLM_ITERATIONS, Math.max(1, Number(event.target.value) || 1)))}
              />
            </div>
          </div>
        </div>
      </section>

      <section className="settings-card access-settings-card">
        <div className="card-header"><h3>{t('tools.terminal.permissionMode')}</h3></div>
        <div className="card-body">
          <div className="setting-section-heading">
            <p className="card-description">{t('tools.terminal.permissionModeDesc')}</p>
            {isAgentScope ? <InheritToggle inherited={fieldInherited('permission_mode')} onChange={(value) => setTerminalInherited('permission_mode', value)} label={t('tools.terminal.inheritDefault')} /> : null}
          </div>
          <PermissionModeControl
            value={terminal.permission_mode}
            label={t('tools.terminal.permissionMode')}
            requestApprovalLabel={t('tools.terminal.requestApproval')}
            requestApprovalDescription={t('tools.terminal.requestApprovalDesc')}
            fullAccessLabel={t('tools.terminal.fullAccess')}
            fullAccessDescription={t('tools.terminal.fullAccessDesc')}
            requestApprovalDisabled={fieldInherited('permission_mode')}
            fullAccessDisabled={fieldInherited('permission_mode')}
            onChange={handlePermissionModeChange}
          />
        </div>
      </section>

      <section className="settings-card roots-settings-card">
        <div className="card-header">
          <h3>{t('tools.terminal.allowedRootsLabel')}</h3>
          <span className="badge">{terminal.allowed_cwd_roots.length}</span>
          {isAgentScope ? <InheritToggle inherited={fieldInherited('allowed_cwd_roots')} onChange={(value) => setTerminalInherited('allowed_cwd_roots', value)} label={t('tools.terminal.inheritDefault')} /> : null}
        </div>
        <div className="card-body">
          <p className="card-description">{t('tools.terminal.allowedRootsDesc')}</p>
          <div className="pattern-input-wrapper">
            <input disabled={fieldInherited('allowed_cwd_roots')} value={rootInput} onChange={(event) => setRootInput(event.target.value)} placeholder={t('tools.terminal.pathPlaceholder')} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); addRoot(); } }} className="pattern-input" />
            <Button
              type="button"
              variant="secondary"
              disabled={fieldInherited('allowed_cwd_roots') || isPickingRoot}
              onClick={() => void browseForRoot()}
              icon={isPickingRoot ? <LoaderCircle size={16} className="is-spinning" /> : <FolderOpen size={16} />}
              className="browse-btn"
            >
              {isPickingRoot ? t('tools.terminal.browsing') : t('tools.terminal.browse')}
            </Button>
            <Button
              type="button"
              variant="secondary"
              disabled={fieldInherited('allowed_cwd_roots') || !normalizedRootInput || rootAlreadyAdded}
              onClick={addRoot}
              icon={<Plus size={16} />}
              className="add-btn"
            >
              {t('tools.terminal.add')}
            </Button>
          </div>
          {terminal.allowed_cwd_roots.length > 0 ? (
            <ul className="allowed-root-list">
              {terminal.allowed_cwd_roots.map((value, index) => (
                <li key={`${value}-${index}`} className="allowed-root-item">
                  <Folder size={16} aria-hidden="true" className="allowed-root-icon" />
                  <code title={displayPath(value)}>{displayPath(value)}</code>
                  <IconButton
                    type="button"
                    variant="ghost"
                    disabled={fieldInherited('allowed_cwd_roots')}
                    onClick={() => removeRoot(index)}
                    className="allowed-root-remove"
                    label={t('tools.terminal.remove')}
                  >
                    <Trash2 size={14} />
                  </IconButton>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      </section>

      {scope === 'defaults' ? <section className="settings-card runtime-card">
        <div className="card-header">
          <div className="settings-section-title">
            <h3>{t('tools.terminal.runtimeTitle')}</h3>
            <p>{environmentDetected ? t('tools.terminal.runtimeReadyDesc') : t('tools.terminal.runtimeMissingDesc')}</p>
          </div>
        </div>
        <div className="card-body runtime-card__body">
          {runtimeLoadError ? (
            <div className="terminal-inline-error" role="alert">
              <span>{runtimeLoadError}</span>
              <Button variant="secondary" type="button" disabled={isRetryingRuntime} onClick={() => void retryRuntimeLoad()} icon={<RefreshCw size={15} className={isRetryingRuntime ? 'is-spinning' : ''} />}>
                {t('common.retry')}
              </Button>
            </div>
          ) : (
            <div className="runtime-component-list">
              {runtimeComponents.map((component) => (
                <RuntimeComponentRow
                  key={component.kind}
                  component={component}
                  installing={component.installing || installingRuntimeComponent === component.kind}
                  refreshing={refreshingRuntimeComponent === component.kind}
                  actionsDisabled={runtime?.installing === true || installingRuntimeComponent !== null || refreshingRuntimeComponent !== null || runtime?.supported === false}
                  onInstall={() => handleInstallRuntime(component)}
                  onRefresh={() => handleRefreshRuntime(component.kind)}
                  t={t}
                />
              ))}
            </div>
          )}
          {runtime?.error ? <code className="runtime-card__error">{runtime.error}</code> : null}
        </div>
      </section> : null}

      {isAgentScope && overrides ? (
        <section className="settings-card grants-settings-card">
          <div className="card-header"><h3>{t('tools.terminal.contextGrants')}</h3><span className="badge">{overrides.terminal.approval_grants.length}</span></div>
          <div className="card-body">
            <p className="card-description">{t('tools.terminal.contextGrantsDesc')}</p>
            {overrides.terminal.approval_grants.length === 0 ? <p className="muted">{t('tools.terminal.noContextGrants')}</p> : (
              <div className="tool-process-list">
                {overrides.terminal.approval_grants.map((grant, index) => (
                  <ToolRecordRow
                    key={`${grant.command}-${grant.cwd || ''}-${index}`}
                    command={grant.command}
                    status={t('tools.terminal.fullAccess')}
                    meta={`${grant.cwd || t('toolCard.defaultCwd')} · ${grant.expires_at ? t('tools.terminal.grantExpires', { time: new Date(grant.expires_at).toLocaleString() }) : t('tools.terminal.grantExpired')}`}
                    action={(
                      <IconButton
                        type="button"
                        variant="ghost"
                        className="tool-process-action"
                        onClick={() => removeApprovalGrant(index)}
                        label={t('tools.terminal.remove')}
                      >
                        <Trash2 size={13} />
                      </IconButton>
                    )}
                  />
                ))}
              </div>
            )}
          </div>
        </section>
      ) : null}

      {scope === 'defaults' ? (
        <section className="settings-card processes-settings-card">
          <div className="card-header"><h3>{t('tools.terminal.backgroundProcesses')}</h3><span className="badge">{processes.length}</span></div>
          <div className="card-body">
            <p className="card-description">{t('tools.terminal.backgroundProcessesDesc')}</p>
            {processLoadError ? (
              <div className="terminal-inline-error" role="alert">
                <span>{processLoadError}</span>
                <Button variant="secondary" type="button" onClick={() => setProcessRequestKey((current) => current + 1)} icon={<RefreshCw size={15} />}>
                  {t('common.retry')}
                </Button>
              </div>
            ) : null}
            {processes.length === 0 ? <p className="muted">{t('tools.terminal.noBackgroundProcesses')}</p> : (
              <div className="tool-process-list">
                {processes.map((process) => (
                  <ToolRecordRow
                    key={process.process_id}
                    className={`is-${process.status}`}
                    command={process.command}
                    status={t(`toolCard.processStatus.${process.status}`)}
                    meta={`${agentName(process.agent_id, agents, t)} · ${process.process_id} · ${t('toolCard.duration', { ms: process.duration_ms })}`}
                    action={process.status === 'running' ? (
                      <button type="button" className="tool-process-action" onClick={() => handleStopProcess(process)} disabled={isStoppingProcess === process.process_id}>
                        <Square size={13} /> {isStoppingProcess === process.process_id ? t('toolCard.stopping') : t('toolCard.stop')}
                      </button>
                    ) : null}
                  >
                    {process.stdout ? <pre className="tool-card__output">{process.stdout}</pre> : null}
                    {process.stderr ? <pre className="tool-card__error">{process.stderr}</pre> : null}
                    {process.tty && process.status === 'running' ? (
                      <div className="terminal-session-console">
                        <pre
                          className="terminal-session-console__output tool-card__output"
                          onDoubleClick={(event) => void handleResizeSession(process, event.currentTarget)}
                        >{process.stdout || t('tools.terminal.sessionWaiting')}</pre>
                        <div className="terminal-session-console__input">
                          <input
                            value={sessionInputs[process.process_id] ?? ''}
                            placeholder={t('tools.terminal.sessionInputPlaceholder')}
                            disabled={isSendingToProcess === process.process_id}
                            onChange={(event) => setSessionInputs((current) => ({ ...current, [process.process_id]: event.target.value }))}
                            onKeyDown={(event) => {
                              if (event.key === 'Enter') {
                                event.preventDefault();
                                void handleSendToSession(process, true);
                              }
                            }}
                          />
                          <button
                            type="button"
                            className="tool-process-action"
                            disabled={isSendingToProcess === process.process_id}
                            onClick={() => void handleSendToSession(process)}
                          >
                            <Send size={13} /> {t('tools.terminal.sessionSend')}
                          </button>
                          <button type="button" className="tool-process-action" onClick={() => void handleInterruptSession(process)}>
                            <X size={13} /> {t('tools.terminal.sessionInterrupt')}
                          </button>
                        </div>
                      </div>
                    ) : null}
                  </ToolRecordRow>
                ))}
              </div>
            )}
          </div>
        </section>
      ) : null}
      </div>
    </div>
  );
}

export { TerminalToolSettings as AgentToolsOverview };
