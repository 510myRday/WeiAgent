import { useEffect, useMemo, useState } from 'react';
import { ArrowLeft, Pencil, RefreshCw } from 'lucide-react';

import type { McpAgentOverrides, McpAgentSettings, McpServer, McpTool } from '../../api';
import { readAgentMcpConfig, updateAgentMcpConfig } from '../../api';
import { useI18n } from '../../i18n';
import { Button, IconButton } from '../../components/ui';

export type McpToolSelection = {
  serverId: string;
  toolName: string;
};

type McpAgentToolView = 'summary' | 'edit';

interface McpAgentToolSettingsProps {
  agentId: string;
  view?: McpAgentToolView;
  selection?: McpToolSelection;
  onEdit?: (selection: McpToolSelection) => void;
  onBack?: () => void;
}

type McpToolEntry = {
  server: McpServer;
  tool: McpTool;
};

function toolLabel(tool: McpTool) {
  return tool.title || tool.name;
}

export function McpAgentToolSettings({
  agentId,
  view = 'edit',
  selection,
  onEdit,
  onBack,
}: McpAgentToolSettingsProps) {
  const { t } = useI18n();
  const [settings, setSettings] = useState<McpAgentSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError('');
    readAgentMcpConfig(agentId).then((value) => {
      if (!cancelled) setSettings(value);
    }).catch((cause: unknown) => {
      if (!cancelled) setError(cause instanceof Error ? cause.message : t('tools.mcp.loadFailed'));
    }).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => { cancelled = true; };
  }, [agentId, t]);

  async function save(next: McpAgentOverrides) {
    setSaving(true);
    setError('');
    try {
      setSettings(await updateAgentMcpConfig(agentId, next));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t('tools.mcp.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  function patchTool(serverId: string, toolName: string, enabled: boolean) {
    if (!settings) return;
    const tools = settings.overrides.tools
      .filter((item) => !(item.server_id === serverId && item.tool_name === toolName))
      .concat({ server_id: serverId, tool_name: toolName, enabled });
    void save({ ...settings.overrides, tools });
  }

  function setToolInherited(serverId: string, toolName: string, inherited: boolean, enabled: boolean) {
    if (!settings) return;
    const tools = settings.overrides.tools.filter((item) => !(item.server_id === serverId && item.tool_name === toolName));
    if (!inherited) tools.push({ server_id: serverId, tool_name: toolName, enabled });
    void save({ ...settings.overrides, tools });
  }

  const entries = useMemo<McpToolEntry[]>(() => {
    if (!settings) return [];
    return settings.effective.flatMap((server) => server.tools.map((tool) => ({ server, tool })));
  }, [settings]);

  if (view === 'summary') {
    if (loading) return <div className="mcp-agent-tools-loading">{t('tools.mcp.loading')}</div>;
    if (!settings) return <div className="agent-tool-empty error-text" role="alert">{error || t('tools.mcp.loadFailed')}</div>;
    if (entries.length === 0) return <div className="agent-tool-empty">{t('tools.mcp.noTools')}</div>;

    return (
      <>
        {entries.map(({ server, tool }) => {
          const override = settings.overrides.tools.find((item) => item.server_id === server.id && item.tool_name === tool.name);
          const inherited = override?.enabled === undefined;
          const enabled = server.enabled && tool.enabled;
          const currentSelection = { serverId: server.id, toolName: tool.name };
          return (
            <div className="agent-tool-row mcp-agent-tool-row" key={tool.qualified_name}>
              <div className="agent-tool-row__body">
                <div className="agent-tool-row__title">
                  <strong>{toolLabel(tool)}</strong>
                  <span className="mini-status-badge mcp-badge">MCP</span>
                </div>
                <small>{tool.description || t('tools.mcp.noToolDescription')}</small>
                <span className="agent-tool-row__source">
                  {inherited ? t('tools.inheritsDefault') : t('tools.agentOverride')}
                </span>
              </div>
              <div className="agent-tool-row__actions">
                <label className="toggle-switch" title={enabled ? t('tools.enabled') : t('tools.disabled')}>
                  <input
                    type="checkbox"
                    aria-label={`${toolLabel(tool)} ${t('tools.enabled')}`}
                    checked={enabled}
                    disabled={saving || !server.enabled}
                    onChange={(event) => patchTool(server.id, tool.name, event.target.checked)}
                  />
                  <span className="toggle-slider" />
                </label>
                <IconButton
                  type="button"
                  variant="ghost"
                  className="agent-tool-edit"
                  onClick={() => onEdit?.(currentSelection)}
                  label={t('tools.editTool')}
                >
                  <Pencil size={17} />
                </IconButton>
              </div>
            </div>
          );
        })}
      </>
    );
  }

  if (loading) {
    return <div className="terminal-loading-state"><div className="loading-spinner" /><p>{t('tools.mcp.loading')}</p></div>;
  }
  if (!settings) {
    return (
      <div className="terminal-loading-state">
        <p className="error-text">{error || t('tools.mcp.loadFailed')}</p>
        <Button variant="secondary" type="button" onClick={() => window.location.reload()} icon={<RefreshCw size={15} />}>
          {t('common.retry')}
        </Button>
      </div>
    );
  }

  const selectedEntry = selection
    ? entries.find(({ server, tool }) => server.id === selection.serverId && tool.name === selection.toolName)
    : undefined;
  if (!selectedEntry) {
    return <div className="terminal-loading-state"><p>{t('tools.mcp.noTools')}</p></div>;
  }

  const { server, tool } = selectedEntry;
  const override = settings.overrides.tools.find((item) => item.server_id === server.id && item.tool_name === tool.name);
  const inherited = override?.enabled === undefined;

  return (
    <div className="terminal-tool-settings-v2 is-agent-scope mcp-agent-tool-editor">
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
            <div className="mcp-agent-tool-heading-title">
              <h2>{toolLabel(tool)}</h2>
              <span className="mini-status-badge mcp-badge">MCP</span>
            </div>
            <p>{server.name} · {tool.description || t('tools.mcp.noToolDescription')}</p>
          </div>
        </div>
      </div>

      <div className="tool-settings-form terminal-settings-form">
        <section className="settings-card basic-settings-card">
          <div className="card-header">
            <h3>{t('tools.mcp.toolBasicSettings')}</h3>
          </div>
          <div className="card-body">
            <div className="setting-row">
              <div className="setting-info">
                <label>{t('tools.mcp.toolEnabled')}</label>
                <p className="setting-desc">{t('tools.mcp.toolEnabledDesc')}</p>
              </div>
              <label className="toggle-switch" title={tool.enabled ? t('tools.enabled') : t('tools.disabled')}>
                <input
                  type="checkbox"
                  checked={tool.enabled}
                  disabled={saving || !server.enabled}
                  onChange={(event) => patchTool(server.id, tool.name, event.target.checked)}
                  aria-label={`${toolLabel(tool)} ${t('tools.mcp.toolEnabled')}`}
                />
                <span className="toggle-slider" />
              </label>
            </div>
            <div className="setting-row">
              <div className="setting-info">
                <label>{t('tools.inheritsDefault')}</label>
                <p className="setting-desc">{t('tools.mcp.toolInheritanceDesc')}</p>
              </div>
              <label className="inherit-control">
                <input
                  type="checkbox"
                  checked={inherited}
                  disabled={saving}
                  onChange={(event) => setToolInherited(server.id, tool.name, event.target.checked, tool.enabled)}
                />
                <span>{t('tools.mcp.inheritTool')}</span>
              </label>
            </div>
            {error ? <p className="error-text" role="alert">{error}</p> : null}
          </div>
        </section>
      </div>
    </div>
  );
}
