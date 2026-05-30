import { useState, useEffect } from 'react';
import { Save, Plus, Trash2 } from 'lucide-react';

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
      .catch((e) => { if (!cancelled) onError(e instanceof Error ? e.message : 'Failed to load tools config'); })
      .finally(() => { if (!cancelled) setIsLoading(false); });
    return () => { cancelled = true; };
  }, [agentId, onError]);

  async function handleSave() {
    if (!config) return;
    setIsSaving(true);
    try {
      const updated = await updateAgentToolsConfig(agentId, config);
      setConfig(updated);
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Failed to save tools config');
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
    return <p className="muted centered">{isLoading ? 'Loading...' : 'No configuration'}</p>;
  }

  return (
    <div className="terminal-tool-settings">
      <div className="config-page-header">
        <div>
          <h2>Terminal Tool Configuration</h2>
          <p>Configure security policies for terminal command execution</p>
        </div>
        <Button variant="primary" onClick={handleSave} disabled={isSaving} icon={<Save size={16} />}>
          {isSaving ? 'Saving...' : 'Save'}
        </Button>
      </div>

      <FormField label="Enabled">
        <label className="mode-toggle">
          <input type="checkbox" checked={config.terminal.enabled} onChange={(e) => setConfig({ ...config, terminal: { ...config.terminal, enabled: e.target.checked } })} />
          <span>{config.terminal.enabled ? 'On' : 'Off'}</span>
        </label>
      </FormField>

      <FormField label="Default Timeout (ms)">
        <input type="number" value={config.terminal.default_timeout_ms} onChange={(e) => setConfig({ ...config, terminal: { ...config.terminal, default_timeout_ms: Number(e.target.value) || 30000 } })} min={1000} step={1000} />
      </FormField>

      <div className="pattern-list-section">
        <label>Blocked Patterns (always rejected)</label>
        <div className="pattern-input-row">
          <input value={blockedInput} onChange={(e) => setBlockedInput(e.target.value)} placeholder="Regex pattern..." onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addBlockedPattern(); } }} />
          <IconButton variant="ghost" onClick={addBlockedPattern} label="Add"><Plus size={16} /></IconButton>
        </div>
        <ul className="pattern-list">
          {config.terminal.blocked_patterns.map((p, i) => (
            <li key={i}><code>{p}</code><button type="button" onClick={() => removeBlockedPattern(i)}><Trash2 size={14} /></button></li>
          ))}
        </ul>
      </div>

      <div className="pattern-list-section">
        <label>Approval Required Patterns (need user confirmation)</label>
        <div className="pattern-input-row">
          <input value={approvalInput} onChange={(e) => setApprovalInput(e.target.value)} placeholder="Regex pattern..." onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addApprovalPattern(); } }} />
          <IconButton variant="ghost" onClick={addApprovalPattern} label="Add"><Plus size={16} /></IconButton>
        </div>
        <ul className="pattern-list">
          {config.terminal.approval_required_patterns.map((p, i) => (
            <li key={i}><code>{p}</code><button type="button" onClick={() => removeApprovalPattern(i)}><Trash2 size={14} /></button></li>
          ))}
        </ul>
      </div>
    </div>
  );
}
