import { useEffect, useState } from 'react';
import { BadgeCheck, CircleAlert, CircleCheck, Clock, Download, FolderLock, Hand, Plus, RefreshCw, Save, Shield, ShieldAlert, Square, Trash2 } from 'lucide-react';

import type { AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings, SandboxRuntimeStatus, ToolProcessInfo } from '../../api';
import {
  installSandboxRuntime,
  listToolProcesses,
  readAgentToolsConfig,
  readSandboxRuntime,
  readToolDefaults,
  refreshSandboxRuntime,
  stopToolProcess,
  updateAgentToolsConfig,
  updateToolDefaults,
} from '../../api';
import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { Button } from '../../components/ui';

interface TerminalToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
  onError: (message: string) => void;
}

type OverrideField = 'enabled' | 'permission_mode' | 'default_timeout_ms' | 'allowed_cwd_roots';

export function TerminalToolSettings({ scope, agentId, agents = [], onError }: TerminalToolSettingsProps) {
  const { t } = useI18n();
  const [defaults, setDefaults] = useState<AgentToolsConfig | null>(null);
  const [agentSettings, setAgentSettings] = useState<AgentToolsSettings | null>(null);
  const [runtime, setRuntime] = useState<SandboxRuntimeStatus | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [isInstallingRuntime, setIsInstallingRuntime] = useState(false);
  const [isRefreshingRuntime, setIsRefreshingRuntime] = useState(false);
  const [processes, setProcesses] = useState<ToolProcessInfo[]>([]);
  const [isStoppingProcess, setIsStoppingProcess] = useState('');
  const [rootInput, setRootInput] = useState('');

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    const configRequest = scope === 'defaults'
      ? readToolDefaults().then((next) => {
        if (!cancelled) setDefaults(next);
      })
      : agentId
        ? readAgentToolsConfig(agentId).then((next) => {
          if (!cancelled) setAgentSettings(next);
        })
        : Promise.reject(new Error(t('tools.terminal.agentRequired')));
    const load = Promise.all([
      configRequest,
      readSandboxRuntime().then((next) => {
        if (!cancelled) setRuntime(next);
      }),
    ]);
    load
      .catch((error) => {
        if (!cancelled) onError(error instanceof Error ? error.message : t('tools.terminal.loadFailed'));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, onError, scope, t]);

  useEffect(() => {
    if (scope !== 'defaults') {
      setProcesses([]);
      return;
    }
    let cancelled = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const next = await listToolProcesses();
        if (cancelled) return;
        setProcesses(next);
        timer = window.setTimeout(refresh, next.some((process) => process.status === 'running') ? 1500 : 4000);
      } catch (error) {
        if (!cancelled) onError(error instanceof Error ? error.message : t('tools.terminal.processLoadFailed'));
      }
    };
    void refresh();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [onError, scope, t]);

  const config = scope === 'defaults' ? defaults : agentSettings?.effective ?? null;
  const overrides = agentSettings?.overrides;

  async function handleStopProcess(process: ToolProcessInfo) {
    setIsStoppingProcess(process.process_id);
    try {
      const stopped = await stopToolProcess(process.process_id, process.agent_id);
      setProcesses((current) => current.map((item) => item.process_id === process.process_id ? stopped : item));
    } catch (error) {
      onError(error instanceof Error ? error.message : t('tools.terminal.processStopFailed'));
    } finally {
      setIsStoppingProcess('');
    }
  }

  async function handleSave() {
    if (!config) return;
    setIsSaving(true);
    try {
      if (scope === 'defaults') {
        setDefaults(await updateToolDefaults({ ...config, terminal: { ...config.terminal, approval_grants: [] } }));
      } else if (agentId && overrides) {
        setAgentSettings(await updateAgentToolsConfig(agentId, overrides));
      }
    } catch (error) {
      onError(error instanceof Error ? error.message : t('tools.terminal.saveFailed'));
    } finally {
      setIsSaving(false);
    }
  }

  async function handleInstallRuntime() {
    setIsInstallingRuntime(true);
    try {
      setRuntime(await installSandboxRuntime());
    } catch (error) {
      onError(error instanceof Error ? error.message : t('tools.terminal.runtimeInstallFailed'));
      try {
        setRuntime(await readSandboxRuntime());
      } catch {
        // Keep the original installation failure as the visible error.
      }
    } finally {
      setIsInstallingRuntime(false);
    }
  }

  async function handleRefreshRuntime() {
    setIsRefreshingRuntime(true);
    try {
      setRuntime(await refreshSandboxRuntime());
    } catch (error) {
      onError(error instanceof Error ? error.message : t('tools.terminal.runtimeRefreshFailed'));
    } finally {
      setIsRefreshingRuntime(false);
    }
  }

  function selectFullAccess() {
    if (!window.confirm(t('tools.terminal.fullAccessConfirm'))) return;
    updateTerminal({ permission_mode: 'full_access' });
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
      if (field === 'enabled') terminal.enabled = config.terminal.enabled;
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
        <div className="loading-spinner" />
        <p>{isLoading ? t('tools.terminal.loading') : t('tools.terminal.noConfig')}</p>
      </div>
    );
  }

  const terminal = config.terminal;
  const isAgentScope = scope === 'agent';
  const maxIterationsInherited = isAgentScope && overrides?.max_llm_iterations === undefined;
  const fieldInherited = (field: OverrideField) => isAgentScope && overrides?.terminal[field] === undefined;
  const sandboxAvailable = runtime?.available === true;
  const terminalUnavailable = terminal.permission_mode === 'request_approval' && !sandboxAvailable;

  return (
    <div className="terminal-tool-settings-v2">
      <div className="settings-hero">
        <div className="hero-content">
          <div className="hero-icon"><Shield size={32} /></div>
          <div className="hero-text">
            <h2>{isAgentScope ? t('tools.terminal.agentConfigTitle') : t('tools.terminal.defaultsTitle')}</h2>
            <p>{isAgentScope ? t('tools.terminal.agentConfigDesc') : t('tools.terminal.defaultsDesc')}</p>
          </div>
        </div>
        <Button variant="primary" onClick={handleSave} disabled={isSaving} icon={<Save size={18} />} className="save-button">
          {isSaving ? t('tools.terminal.saving') : t('tools.terminal.save')}
        </Button>
      </div>

      {scope === 'defaults' ? <div className={`settings-card runtime-card ${sandboxAvailable ? 'runtime-card--ready' : 'warning-card'}`}>
        <div className="card-header">
          {sandboxAvailable ? <CircleCheck size={20} /> : <CircleAlert size={20} />}
          <h3>{t('tools.terminal.runtimeTitle')}</h3>
          <span className={`runtime-status ${sandboxAvailable ? 'is-ready' : 'is-missing'}`}>
            {t(sandboxAvailable ? 'tools.terminal.runtimeReady' : 'tools.terminal.runtimeMissing')}
          </span>
        </div>
        <div className="card-body runtime-card__body">
          <div className="runtime-card__details">
            <p>{t(sandboxAvailable ? 'tools.terminal.runtimeReadyDesc' : 'tools.terminal.runtimeMissingDesc')}</p>
            <small>
              {runtime?.detected_version || runtime?.target_version || ''}
              {runtime?.source ? ` · ${t(`tools.terminal.runtimeSource.${runtime.source}`)}` : ''}
              {runtime?.platform ? ` · ${runtime.platform}` : ''}
            </small>
            {runtime?.error ? <code className="runtime-card__error">{runtime.error}</code> : null}
          </div>
          <div className="runtime-card__actions">
            <button
              type="button"
              className="terminal-output-modal__icon-button"
              onClick={handleRefreshRuntime}
              disabled={isRefreshingRuntime || isInstallingRuntime}
              aria-label={t('tools.terminal.runtimeRefresh')}
              title={t('tools.terminal.runtimeRefresh')}
            >
              <RefreshCw size={17} className={isRefreshingRuntime ? 'is-spinning' : ''} />
            </button>
            {!sandboxAvailable ? (
              <Button
                variant="primary"
                onClick={handleInstallRuntime}
                disabled={isInstallingRuntime || runtime?.supported === false}
                icon={<Download size={17} />}
              >
                {isInstallingRuntime ? t('tools.terminal.runtimeInstalling') : t('tools.terminal.runtimeInstall')}
              </Button>
            ) : null}
          </div>
        </div>
      </div> : null}

      <div className="settings-card">
        <div className="card-header"><Clock size={20} /><h3>{t('tools.terminal.basicSettings')}</h3></div>
        <div className="card-body">
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.enabled')}</label><p className="setting-desc">{t('tools.terminal.enabledDesc')}</p></div>
            {isAgentScope ? <InheritanceControl inherited={fieldInherited('enabled')} onChange={(value) => setTerminalInherited('enabled', value)} t={t} /> : null}
            <label className="toggle-switch">
              <input type="checkbox" checked={terminal.enabled} disabled={fieldInherited('enabled') || terminalUnavailable} onChange={(event) => updateTerminal({ enabled: event.target.checked })} />
              <span className="toggle-slider" />
            </label>
          </div>
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.defaultTimeout')}</label><p className="setting-desc">{t('tools.terminal.timeoutDesc')}</p></div>
            {isAgentScope ? <InheritanceControl inherited={fieldInherited('default_timeout_ms')} onChange={(value) => setTerminalInherited('default_timeout_ms', value)} t={t} /> : null}
            <div className="timeout-input">
              <input type="number" min={1} disabled={fieldInherited('default_timeout_ms')} value={Math.max(1, Math.round(terminal.default_timeout_ms / 1000))} onChange={(event) => updateTerminal({ default_timeout_ms: Math.max(1, Number(event.target.value) || 1) * 1000 })} />
              <span>s</span>
            </div>
          </div>
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.maxLlmIterations')}</label><p className="setting-desc">{t('tools.terminal.maxLlmIterationsDesc')}</p></div>
            {isAgentScope ? <InheritanceControl inherited={maxIterationsInherited} onChange={setMaxIterationsInherited} t={t} /> : null}
            <div className="timeout-input">
              <input
                type="number"
                min={1}
                max={50}
                disabled={maxIterationsInherited}
                value={Math.min(50, Math.max(1, config.max_llm_iterations || 1))}
                onChange={(event) => updateMaxIterations(Math.min(50, Math.max(1, Number(event.target.value) || 1)))}
              />
            </div>
          </div>
        </div>
      </div>

      <div className="settings-card">
        <div className="card-header"><Shield size={20} /><h3>{t('tools.terminal.permissionMode')}</h3></div>
        <div className="card-body">
          <div className="setting-section-heading">
            <p className="card-description">{t('tools.terminal.permissionModeDesc')}</p>
            {isAgentScope ? <InheritanceControl inherited={fieldInherited('permission_mode')} onChange={(value) => setTerminalInherited('permission_mode', value)} t={t} /> : null}
          </div>
          <div className="permission-mode-control" role="radiogroup" aria-label={t('tools.terminal.permissionMode')}>
            <button type="button" role="radio" disabled={fieldInherited('permission_mode') || !sandboxAvailable} aria-checked={terminal.permission_mode === 'request_approval'} className={`${terminal.permission_mode === 'request_approval' ? 'is-active' : ''}${!sandboxAvailable ? ' is-disabled' : ''}`} onClick={() => updateTerminal({ permission_mode: 'request_approval' })}>
              <Hand size={20} />
              <span><strong>{t('tools.terminal.requestApproval')}</strong><small>{t('tools.terminal.requestApprovalDesc')}</small></span>
            </button>
            <button type="button" role="radio" disabled={fieldInherited('permission_mode')} aria-checked={terminal.permission_mode === 'full_access'} className={terminal.permission_mode === 'full_access' ? 'is-active is-danger' : ''} onClick={selectFullAccess}>
              <ShieldAlert size={20} />
              <span><strong>{t('tools.terminal.fullAccess')}</strong><small>{t('tools.terminal.fullAccessDesc')}</small></span>
            </button>
          </div>
        </div>
      </div>

      <div className="settings-card">
        <div className="card-header">
          <FolderLock size={20} />
          <h3>{t('tools.terminal.allowedRootsLabel')}</h3>
          <span className="badge">{terminal.allowed_cwd_roots.length}</span>
          {isAgentScope ? <InheritanceControl inherited={fieldInherited('allowed_cwd_roots')} onChange={(value) => setTerminalInherited('allowed_cwd_roots', value)} t={t} /> : null}
        </div>
        <div className="card-body">
          <p className="card-description">{t('tools.terminal.allowedRootsDesc')}</p>
          <div className="pattern-input-wrapper">
            <input disabled={fieldInherited('allowed_cwd_roots')} value={rootInput} onChange={(event) => setRootInput(event.target.value)} placeholder={t('tools.terminal.pathPlaceholder')} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); addRoot(); } }} className="pattern-input" />
            <Button variant="secondary" disabled={fieldInherited('allowed_cwd_roots')} onClick={addRoot} icon={<Plus size={16} />} className="add-btn">{t('tools.terminal.add')}</Button>
          </div>
          {terminal.allowed_cwd_roots.length > 0 ? (
            <ul className="pattern-tags">
              {terminal.allowed_cwd_roots.map((value, index) => (
                <li key={`${value}-${index}`} className="pattern-tag safe">
                  <code>{value}</code>
                  <button type="button" disabled={fieldInherited('allowed_cwd_roots')} onClick={() => removeRoot(index)} className="remove-btn" title={t('tools.terminal.remove')}><Trash2 size={14} /></button>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      </div>

      {isAgentScope && overrides ? (
        <div className="settings-card">
          <div className="card-header"><BadgeCheck size={20} /><h3>{t('tools.terminal.contextGrants')}</h3><span className="badge">{overrides.terminal.approval_grants.length}</span></div>
          <div className="card-body">
            <p className="card-description">{t('tools.terminal.contextGrantsDesc')}</p>
            {overrides.terminal.approval_grants.length === 0 ? <p className="muted">{t('tools.terminal.noContextGrants')}</p> : (
              <div className="tool-process-list">
                {overrides.terminal.approval_grants.map((grant, index) => (
                  <article key={`${grant.command}-${grant.cwd || ''}-${index}`} className="tool-process-row">
                    <div className="tool-process-row__header">
                      <code>{grant.command}</code>
                      <span>{t('tools.terminal.fullAccess')}</span>
                      <button type="button" className="tool-process-action" onClick={() => removeApprovalGrant(index)} title={t('tools.terminal.remove')}><Trash2 size={13} /></button>
                    </div>
                    <small>{grant.cwd || t('toolCard.defaultCwd')} · {grant.expires_at ? t('tools.terminal.grantExpires', { time: new Date(grant.expires_at).toLocaleString() }) : t('tools.terminal.grantExpired')}</small>
                  </article>
                ))}
              </div>
            )}
          </div>
        </div>
      ) : null}

      {scope === 'defaults' ? (
        <div className="settings-card">
          <div className="card-header"><RefreshCw size={20} /><h3>{t('tools.terminal.backgroundProcesses')}</h3><span className="badge">{processes.length}</span></div>
          <div className="card-body">
            <p className="card-description">{t('tools.terminal.backgroundProcessesDesc')}</p>
            {processes.length === 0 ? <p className="muted">{t('tools.terminal.noBackgroundProcesses')}</p> : (
              <div className="tool-process-list">
                {processes.map((process) => (
                  <article key={process.process_id} className={`tool-process-row is-${process.status}`}>
                    <div className="tool-process-row__header">
                      <code>{process.command}</code>
                      <span>{t(`toolCard.processStatus.${process.status}`)}</span>
                      {process.status === 'running' ? (
                        <button type="button" className="tool-process-action" onClick={() => handleStopProcess(process)} disabled={isStoppingProcess === process.process_id}>
                          <Square size={13} /> {isStoppingProcess === process.process_id ? t('toolCard.stopping') : t('toolCard.stop')}
                        </button>
                      ) : null}
                    </div>
                    <small>{agentName(process.agent_id, agents, t)} · {process.process_id} · {t('toolCard.duration', { ms: process.duration_ms })}</small>
                    {process.stdout ? <pre className="tool-card__output">{process.stdout}</pre> : null}
                    {process.stderr ? <pre className="tool-card__error">{process.stderr}</pre> : null}
                  </article>
                ))}
              </div>
            )}
          </div>
        </div>
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
      <span>{t('tools.terminal.inheritDefault')}</span>
    </label>
  );
}

function mergeToolsConfig(defaults: AgentToolsConfig, overrides: AgentToolsOverrides): AgentToolsConfig {
  return {
    max_llm_iterations: overrides.max_llm_iterations ?? defaults.max_llm_iterations,
    terminal: {
      enabled: overrides.terminal.enabled ?? defaults.terminal.enabled,
      permission_mode: overrides.terminal.permission_mode ?? defaults.terminal.permission_mode,
      approval_grants: overrides.terminal.approval_grants,
      default_timeout_ms: overrides.terminal.default_timeout_ms ?? defaults.terminal.default_timeout_ms,
      allowed_cwd_roots: overrides.terminal.allowed_cwd_roots ?? defaults.terminal.allowed_cwd_roots,
    },
  };
}

function agentName(agentId: string, agents: Agent[], t: (path: string) => string): string {
  const agent = agents.find((item) => item.id === agentId);
  if (!agent) return agentId;
  return agent.is_builtin ? t('agentConfig.builtinMasterName') : agent.name;
}
