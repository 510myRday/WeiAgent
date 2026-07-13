import { useEffect, useState } from 'react';
import { Save, Plus, Trash2, Shield, Clock, AlertTriangle, CheckCircle, Box, FolderLock, BadgeCheck, Square, RefreshCw } from 'lucide-react';

import type { AgentToolsConfig, ToolProcessInfo } from '../../api';
import { listToolProcesses, readAgentToolsConfig, stopToolProcess, updateAgentToolsConfig } from '../../api';
import { useI18n } from '../../i18n';
import { Button } from '../../components/ui';

interface TerminalToolSettingsProps {
  agentId: string;
  onError: (message: string) => void;
}

type PatternKind = 'blocked_patterns' | 'approval_required_patterns' | 'always_allowed_patterns' | 'allowed_cwd_roots';

export function TerminalToolSettings({ agentId, onError }: TerminalToolSettingsProps) {
  const { t } = useI18n();
  const [config, setConfig] = useState<AgentToolsConfig | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [processes, setProcesses] = useState<ToolProcessInfo[]>([]);
  const [isStoppingProcess, setIsStoppingProcess] = useState('');
  const [inputs, setInputs] = useState<Record<PatternKind, string>>({
    blocked_patterns: '', approval_required_patterns: '', always_allowed_patterns: '', allowed_cwd_roots: '',
  });

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    readAgentToolsConfig(agentId)
      .then((next) => { if (!cancelled) setConfig(next); })
      .catch((error) => { if (!cancelled) onError(error instanceof Error ? error.message : t('tools.terminal.loadFailed')); })
      .finally(() => { if (!cancelled) setIsLoading(false); });
    return () => { cancelled = true; };
  }, [agentId, onError, t]);


  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const next = await listToolProcesses(agentId);
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
  }, [agentId, onError, t]);

  async function handleStopProcess(processId: string) {
    setIsStoppingProcess(processId);
    try {
      const stopped = await stopToolProcess(processId);
      setProcesses((current) => current.map((process) => process.process_id === processId ? stopped : process));
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
      setConfig(await updateAgentToolsConfig(agentId, config));
    } catch (error) {
      onError(error instanceof Error ? error.message : t('tools.terminal.saveFailed'));
    } finally {
      setIsSaving(false);
    }
  }

  function updateTerminal(patch: Partial<AgentToolsConfig['terminal']>) {
    setConfig((current) => current ? { ...current, terminal: { ...current.terminal, ...patch } } : current);
  }

  function addItem(kind: PatternKind) {
    const value = inputs[kind].trim();
    if (!value || !config) return;
    const current = config.terminal[kind];
    if (current.includes(value)) return;
    updateTerminal({ [kind]: [...current, value] });
    setInputs((previous) => ({ ...previous, [kind]: '' }));
  }

  function removeItem(kind: PatternKind, index: number) {
    if (!config) return;
    updateTerminal({ [kind]: config.terminal[kind].filter((_, itemIndex) => itemIndex !== index) });
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
  const sandbox = terminal.sandbox;

  return (
    <div className="terminal-tool-settings-v2">
      <div className="settings-hero">
        <div className="hero-content">
          <div className="hero-icon"><Shield size={32} /></div>
          <div className="hero-text">
            <h2>{t('tools.terminal.configTitle')}</h2>
            <p>{t('tools.terminal.configDesc')}</p>
          </div>
        </div>
        <Button variant="primary" onClick={handleSave} disabled={isSaving} icon={<Save size={18} />} className="save-button">
          {isSaving ? t('tools.terminal.saving') : t('tools.terminal.save')}
        </Button>
      </div>

      <div className="settings-card">
        <div className="card-header"><Clock size={20} /><h3>{t('tools.terminal.basicSettings')}</h3></div>
        <div className="card-body">
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.enabled')}</label><p className="setting-desc">{t('tools.terminal.enabledDesc')}</p></div>
            <label className="toggle-switch">
              <input type="checkbox" checked={terminal.enabled} onChange={(event) => updateTerminal({ enabled: event.target.checked })} />
              <span className="toggle-slider" />
            </label>
          </div>
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.defaultTimeout')}</label><p className="setting-desc">{t('tools.terminal.timeoutDesc')}</p></div>
            <div className="timeout-input">
              <input type="number" min={1} value={Math.max(1, Math.round(terminal.default_timeout_ms / 1000))} onChange={(event) => updateTerminal({ default_timeout_ms: Math.max(1, Number(event.target.value) || 1) * 1000 })} />
              <span>s</span>
            </div>
          </div>
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.maxLlmIterations')}</label><p className="setting-desc">{t('tools.terminal.maxLlmIterationsDesc')}</p></div>
            <div className="timeout-input">
              <input
                type="number"
                min={1}
                value={Math.max(1, config.max_llm_iterations || 1)}
                onChange={(event) => setConfig((current) => current ? { ...current, max_llm_iterations: Math.max(1, Number(event.target.value) || 1) } : current)}
              />
            </div>
          </div>
        </div>
      </div>

      <div className="settings-card">
        <div className="card-header"><Box size={20} /><h3>{t('tools.terminal.sandboxSettings')}</h3></div>
        <div className="card-body">
          <p className="card-description">{t('tools.terminal.dockerHint')}</p>
          <div className="setting-row">
            <div className="setting-info"><label>{t('tools.terminal.sandboxMode')}</label></div>
            <select value={sandbox.mode} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, mode: event.target.value as 'docker' | 'native_restricted' } })}>
              <option value="docker">{t('tools.terminal.dockerMode')}</option>
              <option value="native_restricted">{t('tools.terminal.nativeMode')}</option>
            </select>
          </div>
          {sandbox.mode === 'docker' ? (
            <>
              <div className="setting-row">
                <div className="setting-info"><label>{t('tools.terminal.dockerImage')}</label></div>
                <input value={sandbox.docker_image} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, docker_image: event.target.value } })} />
              </div>
              <div className="sandbox-grid">
                <label>{t('tools.terminal.memoryLimit')}<input type="number" min={64} value={sandbox.memory_limit_mb} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, memory_limit_mb: Math.max(1, Number(event.target.value) || 1) } })} /></label>
                <label>{t('tools.terminal.cpuLimit')}<input type="number" min={0.1} step={0.1} value={sandbox.cpu_limit} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, cpu_limit: Math.max(0.1, Number(event.target.value) || 0.1) } })} /></label>
                <label>{t('tools.terminal.pidsLimit')}<input type="number" min={1} value={sandbox.pids_limit} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, pids_limit: Math.max(1, Number(event.target.value) || 1) } })} /></label>
              </div>
              <div className="setting-row">
                <div className="setting-info"><label>{t('tools.terminal.networkEnabled')}</label></div>
                <label className="toggle-switch"><input type="checkbox" checked={sandbox.network_enabled} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, network_enabled: event.target.checked } })} /><span className="toggle-slider" /></label>
              </div>
              <div className="setting-row">
                <div className="setting-info"><label>{t('tools.terminal.workspaceReadOnly')}</label></div>
                <label className="toggle-switch"><input type="checkbox" checked={sandbox.workspace_read_only} onChange={(event) => updateTerminal({ sandbox: { ...sandbox, workspace_read_only: event.target.checked } })} /><span className="toggle-slider" /></label>
              </div>
            </>
          ) : null}
        </div>
      </div>


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
                      <button type="button" className="tool-process-action" onClick={() => handleStopProcess(process.process_id)} disabled={isStoppingProcess === process.process_id}>
                        <Square size={13} /> {isStoppingProcess === process.process_id ? t('toolCard.stopping') : t('toolCard.stop')}
                      </button>
                    ) : null}
                  </div>
                  {process.stdout ? <pre className="tool-card__output">{process.stdout}</pre> : null}
                  {process.stderr ? <pre className="tool-card__error">{process.stderr}</pre> : null}
                  <small>{process.process_id} · {t('toolCard.duration', { ms: process.duration_ms })}</small>
                </article>
              ))}
            </div>
          )}
        </div>
      </div>
      <PatternCard icon={<FolderLock size={20} />} title={t('tools.terminal.allowedRootsLabel')} description={t('tools.terminal.allowedRootsDesc')} values={terminal.allowed_cwd_roots} input={inputs.allowed_cwd_roots} placeholder={t('tools.terminal.pathPlaceholder')} kind="allowed_cwd_roots" tone="safe" onInput={(value) => setInputs((current) => ({ ...current, allowed_cwd_roots: value }))} onAdd={addItem} onRemove={removeItem} t={t} />
      <PatternCard icon={<AlertTriangle size={20} />} title={t('tools.terminal.blockedPatternsLabel')} description={t('tools.terminal.blockedDesc')} values={terminal.blocked_patterns} input={inputs.blocked_patterns} placeholder={t('tools.terminal.patternPlaceholder')} kind="blocked_patterns" tone="danger" onInput={(value) => setInputs((current) => ({ ...current, blocked_patterns: value }))} onAdd={addItem} onRemove={removeItem} t={t} />
      <PatternCard icon={<CheckCircle size={20} />} title={t('tools.terminal.approvalPatternsLabel')} description={t('tools.terminal.approvalDesc')} values={terminal.approval_required_patterns} input={inputs.approval_required_patterns} placeholder={t('tools.terminal.patternPlaceholder')} kind="approval_required_patterns" tone="warning" onInput={(value) => setInputs((current) => ({ ...current, approval_required_patterns: value }))} onAdd={addItem} onRemove={removeItem} t={t} />
      <PatternCard icon={<BadgeCheck size={20} />} title={t('tools.terminal.alwaysAllowedPatternsLabel')} description={t('tools.terminal.alwaysAllowedDesc')} values={terminal.always_allowed_patterns} input={inputs.always_allowed_patterns} placeholder={t('tools.terminal.patternPlaceholder')} kind="always_allowed_patterns" tone="safe" onInput={(value) => setInputs((current) => ({ ...current, always_allowed_patterns: value }))} onAdd={addItem} onRemove={removeItem} t={t} />
    </div>
  );
}

function PatternCard({ icon, title, description, values, input, placeholder, kind, tone, onInput, onAdd, onRemove, t }: {
  icon: React.ReactNode;
  title: string;
  description: string;
  values: string[];
  input: string;
  placeholder: string;
  kind: PatternKind;
  tone: 'danger' | 'warning' | 'safe';
  onInput: (value: string) => void;
  onAdd: (kind: PatternKind) => void;
  onRemove: (kind: PatternKind, index: number) => void;
  t: (path: string) => string;
}) {
  return (
    <div className={`settings-card ${tone === 'danger' ? 'danger-card' : tone === 'warning' ? 'warning-card' : ''}`}>
      <div className="card-header">{icon}<h3>{title}</h3><span className="badge">{values.length}</span></div>
      <div className="card-body">
        <p className="card-description">{description}</p>
        <div className="pattern-input-wrapper">
          <input value={input} onChange={(event) => onInput(event.target.value)} placeholder={placeholder} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); onAdd(kind); } }} className="pattern-input" />
          <Button variant="secondary" onClick={() => onAdd(kind)} icon={<Plus size={16} />} className="add-btn">{t('tools.terminal.add')}</Button>
        </div>
        {values.length > 0 ? (
          <ul className="pattern-tags">
            {values.map((value, index) => (
              <li key={`${value}-${index}`} className={`pattern-tag ${tone}`}>
                <code>{value}</code>
                <button type="button" onClick={() => onRemove(kind, index)} className="remove-btn" title={t('tools.terminal.remove')}><Trash2 size={14} /></button>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    </div>
  );
}
