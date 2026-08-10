import { useEffect, useState } from 'react';
import { ArrowLeft, RefreshCw } from 'lucide-react';

import type { Agent } from '../../types';
import type { AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings } from '../../api';
import {
  readAgentToolsConfig,
  readToolDefaults,
  updateAgentToolsConfig,
  updateToolDefaults,
} from '../../api';
import { useI18n } from '../../i18n';
import { Button, IconButton, useFeedback } from '../../components/ui';

interface TaskPlanningToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
  onBack?: () => void;
}

export function TaskPlanningToolSettings({ scope, agentId, onBack }: TaskPlanningToolSettingsProps) {
  const { t } = useI18n();
  const { confirm } = useFeedback();
  const [config, setConfig] = useState<AgentToolsConfig | null>(null);
  const [settings, setSettings] = useState<AgentToolsSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError('');
    const request = scope === 'defaults'
      ? readToolDefaults().then((value) => {
        if (!cancelled) setConfig(value);
      })
      : agentId
        ? readAgentToolsConfig(agentId).then((value) => {
          if (!cancelled) setSettings(value);
        })
        : Promise.reject(new Error(t('tools.taskPlanning.agentRequired')));
    request.catch((requestError: unknown) => {
      if (!cancelled) setError(requestError instanceof Error ? requestError.message : t('tools.taskPlanning.loadFailed'));
    }).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => { cancelled = true; };
  }, [agentId, scope, t]);

  const effective = scope === 'defaults' ? config : settings?.effective;
  const enabled = Boolean(effective?.task_planning?.enabled);
  const progressInterval = Math.min(
    999,
    Math.max(1, effective?.task_planning?.progress_interval || 5),
  );
  const progressIntervalInherited = scope === 'agent'
    && settings?.overrides.task_planning.progress_interval === undefined;

  async function updateTaskPlanning(patch: { enabled?: boolean; progress_interval?: number }) {
    if (!effective || saving) return;
    setSaving(true);
    setError('');
    try {
      if (scope === 'defaults') {
        const saved = await updateToolDefaults({
          ...config!,
          task_planning: { ...config!.task_planning, ...patch },
        });
        setConfig(saved);
      } else if (agentId && settings) {
        const overrides: AgentToolsOverrides = {
          ...settings.overrides,
          task_planning: {
            ...settings.overrides.task_planning,
            ...patch,
          },
        };
        const saved = await updateAgentToolsConfig(agentId, overrides);
        setSettings(saved);
      }
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : t('tools.taskPlanning.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  async function handleEnabledChange(nextEnabled: boolean) {
    if (nextEnabled) {
      await updateTaskPlanning({ enabled: true });
      return;
    }

    const confirmed = await confirm({
      title: t('tools.taskPlanning.disableTitle'),
      message: t('tools.taskPlanning.disableConfirm'),
      confirmLabel: t('tools.taskPlanning.disableConfirmLabel'),
      tone: 'warning',
    });
    if (confirmed) await updateTaskPlanning({ enabled: false });
  }

  function setProgressIntervalInherited(inherited: boolean) {
    if (!settings || !effective || scope !== 'agent' || saving) return;
    const taskPlanning = { ...settings.overrides.task_planning };
    if (inherited) {
      delete taskPlanning.progress_interval;
    } else {
      taskPlanning.progress_interval = progressInterval;
    }
    void updateTaskPlanning(taskPlanning);
  }

  if (loading) return <div className="terminal-loading-state"><div className="loading-spinner" /><p>{t('tools.taskPlanning.loading')}</p></div>;
  if (!effective) return <div className="terminal-loading-state"><p>{error || t('tools.taskPlanning.loadFailed')}</p><Button variant="secondary" type="button" onClick={() => window.location.reload()} icon={<RefreshCw size={15} />}>{t('common.retry')}</Button></div>;

  return (
    <div className={`terminal-tool-settings-v2 ${scope === 'agent' ? 'is-agent-scope' : 'is-default-scope'}`}>
      {scope === 'agent' ? (
        <div className="tool-config-heading">
          <div className="tool-config-heading__main">
            {onBack ? (
              <IconButton
                type="button"
                variant="ghost"
                className="tool-config-back"
                onClick={onBack}
                label={t('tools.backToList')}
              >
                <ArrowLeft size={18} />
              </IconButton>
            ) : null}
            <div>
              <h2>{t('tools.taskPlanning.agentConfigTitle')}</h2>
              <p>{t('tools.taskPlanning.agentConfigDesc')}</p>
            </div>
          </div>
        </div>
      ) : null}
      <div className="tool-settings-form terminal-settings-form">
        <section className="settings-card basic-settings-card">
          <div className="card-header">
            <h3>{t('tools.taskPlanning.basicSettings')}</h3>
          </div>
          <div className="card-body">
            <div className="setting-row">
              <div className="setting-info">
                <label>{t('tools.taskPlanning.enabled')}</label>
                <p className="setting-desc">{t('tools.taskPlanning.enabledDesc')}</p>
              </div>
              <label className="toggle-switch" title={enabled ? t('tools.enabled') : t('tools.disabled')}>
                <input type="checkbox" checked={enabled} disabled={saving} onChange={(event) => void handleEnabledChange(event.target.checked)} aria-label={t('tools.taskPlanning.enabled')} />
                <span className="toggle-slider" />
              </label>
            </div>
            <div className="setting-row">
              <div className="setting-info">
                <label>{t('tools.taskPlanning.progressInterval')}</label>
                <p className="setting-desc">{t('tools.taskPlanning.progressIntervalDesc')}</p>
              </div>
              {scope === 'agent' ? (
                <label className="inherit-control">
                  <input type="checkbox" checked={progressIntervalInherited} disabled={saving} onChange={(event) => setProgressIntervalInherited(event.target.checked)} />
                  <span>{t('tools.terminal.inheritDefault')}</span>
                </label>
              ) : null}
              <div className="timeout-input">
                <input
                  type="number"
                  min={1}
                  max={999}
                  disabled={saving || progressIntervalInherited}
                  value={progressInterval}
                  onChange={(event) => void updateTaskPlanning({
                    progress_interval: Math.min(999, Math.max(1, Number(event.target.value) || 1)),
                  })}
                />
                <span>{t('tools.taskPlanning.rounds')}</span>
              </div>
            </div>
            {error ? <p className="error-text" role="alert">{error}</p> : null}
          </div>
        </section>
      </div>
    </div>
  );
}
