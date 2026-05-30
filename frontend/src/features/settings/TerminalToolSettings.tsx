import { useState, useEffect } from 'react';
import { Save, Plus, Trash2, Shield, Clock, AlertTriangle, CheckCircle } from 'lucide-react';

import type { AgentToolsConfig } from '../../api';
import { readAgentToolsConfig, updateAgentToolsConfig } from '../../api';
import { useI18n } from '../../i18n';
import { Button, FormField, IconButton } from '../../components/ui';

interface TerminalToolSettingsProps {
  agentId: string;
  onError: (message: string) => void;
}

export function TerminalToolSettings({ agentId, onError }: TerminalToolSettingsProps) {
  const { t } = useI18n();
  const [config, setConfig] = useState<AgentToolsConfig | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [blockedInput, setBlockedInput] = useState('');
  const [approvalInput, setApprovalInput] = useState('');

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    readAgentToolsConfig(agentId)
      .then((c) => { if (!cancelled) setConfig(c); })
      .catch((e) => { if (!cancelled) onError(e instanceof Error ? e.message : t('tools.terminal.loadFailed')); })
      .finally(() => { if (!cancelled) setIsLoading(false); });
    return () => { cancelled = true; };
  }, [agentId, onError, t]);

  async function handleSave() {
    if (!config) return;
    setIsSaving(true);
    try {
      const updated = await updateAgentToolsConfig(agentId, config);
      setConfig(updated);
    } catch (e) {
      onError(e instanceof Error ? e.message : t('tools.terminal.saveFailed'));
    } finally {
      setIsSaving(false);
    }
  }

  function addBlockedPattern() {
    const p = blockedInput.trim();
    if (!p || !config) return;
    setConfig({ ...config, terminal: { ...config.terminal, blocked_patterns: [...config.terminal.blocked_patterns, p] } });
    setBlockedInput('');
  }

  function removeBlockedPattern(index: number) {
    if (!config) return;
    setConfig({ ...config, terminal: { ...config.terminal, blocked_patterns: config.terminal.blocked_patterns.filter((_, i) => i !== index) } });
  }

  function addApprovalPattern() {
    const p = approvalInput.trim();
    if (!p || !config) return;
    setConfig({ ...config, terminal: { ...config.terminal, approval_required_patterns: [...config.terminal.approval_required_patterns, p] } });
    setApprovalInput('');
  }

  function removeApprovalPattern(index: number) {
    if (!config) return;
    setConfig({ ...config, terminal: { ...config.terminal, approval_required_patterns: config.terminal.approval_required_patterns.filter((_, i) => i !== index) } });
  }

  if (isLoading || !config) {
    return (
      <div className="terminal-loading-state">
        <div className="loading-spinner"></div>
        <p>{isLoading ? t('tools.terminal.loading') : t('tools.terminal.noConfig')}</p>
      </div>
    );
  }

  return (
    <div className="terminal-tool-settings-v2">
      {/* Header Section */}
      <div className="settings-hero">
        <div className="hero-content">
          <div className="hero-icon">
            <Shield size={32} />
          </div>
          <div className="hero-text">
            <h2>{t('tools.terminal.configTitle')}</h2>
            <p>{t('tools.terminal.configDesc')}</p>
          </div>
        </div>
        <Button
          variant="primary"
          onClick={handleSave}
          disabled={isSaving}
          icon={<Save size={18} />}
          className="save-button"
        >
          {isSaving ? t('tools.terminal.saving') : t('tools.terminal.save')}
        </Button>
      </div>

      {/* Basic Settings Card */}
      <div className="settings-card">
        <div className="card-header">
          <Clock size={20} />
          <h3>{t('tools.terminal.basicSettings')}</h3>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="setting-info">
              <label>{t('tools.terminal.enabled')}</label>
              <p className="setting-desc">{t('tools.terminal.enabledDesc')}</p>
            </div>
            <label className="toggle-switch">
              <input
                type="checkbox"
                checked={config.terminal.enabled}
                onChange={(e) => setConfig({ ...config, terminal: { ...config.terminal, enabled: e.target.checked } })}
              />
              <span className="toggle-slider"></span>
            </label>
          </div>

          <div className="setting-row">
            <div className="setting-info">
              <label>{t('tools.terminal.defaultTimeout')}</label>
              <p className="setting-desc">{t('tools.terminal.timeoutDesc')}</p>
            </div>
            <div className="timeout-input">
              <input
                type="number"
                value={config.terminal.default_timeout_ms / 1000}
                onChange={(e) => setConfig({
                  ...config,
                  terminal: {
                    ...config.terminal,
                    default_timeout_ms: (Number(e.target.value) || 30) * 1000
                  }
                })}
                min={1}
                step={1}
              />
              <span className="unit">秒</span>
            </div>
          </div>
        </div>
      </div>

      {/* Blocked Patterns Card */}
      <div className="settings-card danger-card">
        <div className="card-header">
          <AlertTriangle size={20} />
          <h3>{t('tools.terminal.blockedPatternsLabel')}</h3>
          <span className="badge">{config.terminal.blocked_patterns.length}</span>
        </div>
        <div className="card-body">
          <p className="card-description">{t('tools.terminal.blockedDesc')}</p>
          <div className="pattern-input-wrapper">
            <input
              value={blockedInput}
              onChange={(e) => setBlockedInput(e.target.value)}
              placeholder={t('tools.terminal.patternPlaceholder')}
              onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addBlockedPattern(); } }}
              className="pattern-input"
            />
            <Button
              variant="secondary"
              onClick={addBlockedPattern}
              icon={<Plus size={16} />}
              className="add-btn"
            >
              {t('tools.terminal.add')}
            </Button>
          </div>
          {config.terminal.blocked_patterns.length > 0 && (
            <ul className="pattern-tags">
              {config.terminal.blocked_patterns.map((p, i) => (
                <li key={i} className="pattern-tag danger">
                  <code>{p}</code>
                  <button
                    type="button"
                    onClick={() => removeBlockedPattern(i)}
                    className="remove-btn"
                    title={t('tools.terminal.remove')}
                  >
                    <Trash2 size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      {/* Approval Required Patterns Card */}
      <div className="settings-card warning-card">
        <div className="card-header">
          <CheckCircle size={20} />
          <h3>{t('tools.terminal.approvalPatternsLabel')}</h3>
          <span className="badge">{config.terminal.approval_required_patterns.length}</span>
        </div>
        <div className="card-body">
          <p className="card-description">{t('tools.terminal.approvalDesc')}</p>
          <div className="pattern-input-wrapper">
            <input
              value={approvalInput}
              onChange={(e) => setApprovalInput(e.target.value)}
              placeholder={t('tools.terminal.patternPlaceholder')}
              onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addApprovalPattern(); } }}
              className="pattern-input"
            />
            <Button
              variant="secondary"
              onClick={addApprovalPattern}
              icon={<Plus size={16} />}
              className="add-btn"
            >
              {t('tools.terminal.add')}
            </Button>
          </div>
          {config.terminal.approval_required_patterns.length > 0 && (
            <ul className="pattern-tags">
              {config.terminal.approval_required_patterns.map((p, i) => (
                <li key={i} className="pattern-tag warning">
                  <code>{p}</code>
                  <button
                    type="button"
                    onClick={() => removeApprovalPattern(i)}
                    className="remove-btn"
                    title={t('tools.terminal.remove')}
                  >
                    <Trash2 size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
