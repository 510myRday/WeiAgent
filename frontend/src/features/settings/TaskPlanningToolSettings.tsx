import { useEffect, useState } from 'react';
import { RefreshCw, ShieldCheck } from 'lucide-react';

import type { Agent } from '../../types';
import type { AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings } from '../../api';
import {
  readAgentToolsConfig,
  readToolDefaults,
  updateAgentToolsConfig,
  updateToolDefaults,
} from '../../api';
import { useI18n } from '../../i18n';
import { Button } from '../../components/ui';

interface TaskPlanningToolSettingsProps {
  scope: 'defaults' | 'agent';
  agentId?: string;
  agents?: Agent[];
}

export function TaskPlanningToolSettings({ scope, agentId }: TaskPlanningToolSettingsProps) {
  const { t } = useI18n();
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

  async function toggle(nextEnabled: boolean) {
    if (!effective || saving) return;
    setSaving(true);
    setError('');
    try {
      if (scope === 'defaults') {
        const saved = await updateToolDefaults({
          ...config!,
          task_planning: { enabled: nextEnabled },
        });
        setConfig(saved);
      } else if (agentId && settings) {
        const overrides: AgentToolsOverrides = {
          ...settings.overrides,
          task_planning: {
            ...settings.overrides.task_planning,
            enabled: nextEnabled,
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

  if (loading) return <div className="terminal-loading-state"><div className="loading-spinner" /><p>{t('tools.taskPlanning.loading')}</p></div>;
  if (!effective) return <div className="terminal-loading-state"><p>{error || t('tools.taskPlanning.loadFailed')}</p><Button variant="secondary" type="button" onClick={() => window.location.reload()} icon={<RefreshCw size={15} />}>{t('common.retry')}</Button></div>;

  return (
    <div className="terminal-tool-settings-v2">
      <div className="tool-config-heading">
        <div>
          <h2>{scope === 'defaults' ? t('tools.taskPlanning.defaultsTitle') : t('tools.taskPlanning.agentConfigTitle')}</h2>
          <p>{scope === 'defaults' ? t('tools.taskPlanning.defaultsDesc') : t('tools.taskPlanning.agentConfigDesc')}</p>
        </div>
      </div>
      <section className="settings-section">
        <div className="card-header">
          <h3>{t('tools.taskPlanning.name')}</h3>
          <span className="mini-status-badge"><ShieldCheck size={12} />{t('tools.builtinBadge')}</span>
        </div>
        <p className="card-description">{t('tools.taskPlanning.description')}</p>
        <div className="setting-row">
          <div className="setting-info">
            <label>{t('tools.taskPlanning.enabled')}</label>
            <p className="setting-desc">{t('tools.taskPlanning.enabledDesc')}</p>
          </div>
          <label className="toggle-switch" title={enabled ? t('tools.enabled') : t('tools.disabled')}>
            <input type="checkbox" checked={enabled} disabled={saving} onChange={(event) => void toggle(event.target.checked)} aria-label={t('tools.taskPlanning.enabled')} />
            <span className="toggle-slider" />
          </label>
        </div>
        {error ? <p className="error-text" role="alert">{error}</p> : null}
      </section>
    </div>
  );
}
