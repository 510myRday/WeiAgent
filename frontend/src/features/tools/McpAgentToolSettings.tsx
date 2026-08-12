import { useEffect, useMemo, useState } from 'react';
import { ArrowLeft, Cable, Pencil, RefreshCw, Search } from 'lucide-react';

import type { McpAgentOverrides, McpAgentSettings, McpTool } from '../../api';
import { readAgentMcpConfig, updateAgentMcpConfig } from '../../api';
import { useI18n } from '../../i18n';
import { Button, IconButton } from '../../components/ui';
import { McpToolDetails } from './McpToolDetails';

export type McpServerSelection = {
  serverId: string;
};

type McpAgentToolView = 'summary' | 'edit';

interface McpAgentToolSettingsProps {
  agentId: string;
  view?: McpAgentToolView;
  selection?: McpServerSelection;
  onEdit?: (selection: McpServerSelection) => void;
  onBack?: () => void;
}

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
  const [search, setSearch] = useState('');

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
    const server = settings.effective.find((item) => item.id === serverId);
    const serverOverride = settings.overrides.servers.find((item) => item.server_id === serverId);
    const tools = settings.overrides.tools
      .filter((item) => !(item.server_id === serverId && item.tool_name === toolName))
      .concat({ server_id: serverId, tool_name: toolName, enabled });
    const servers = serverOverride
      ? settings.overrides.servers.map((item) => item.server_id === serverId ? { ...item, customized: true } : item)
      : settings.overrides.servers.concat({ server_id: serverId, enabled: server?.enabled ?? true, customized: true });
    void save({ ...settings.overrides, servers, tools });
  }

  function setServerInherited(serverId: string, inherited: boolean) {
    if (!settings) return;
    if (inherited) {
      const servers = settings.overrides.servers.filter((item) => item.server_id !== serverId);
      const tools = settings.overrides.tools.filter((item) => item.server_id !== serverId);
      void save({ ...settings.overrides, servers, tools });
      return;
    }
    if (settings.overrides.servers.some((item) => item.server_id === serverId)) return;
    const server = settings.effective.find((item) => item.id === serverId);
    const servers = settings.overrides.servers.concat({ server_id: serverId, enabled: server?.enabled ?? true, customized: true });
    void save({ ...settings.overrides, servers });
  }

  function setServerEnabled(serverId: string, enabled: boolean) {
    if (!settings) return;
    const existing = settings.overrides.servers.find((item) => item.server_id === serverId);
    const servers = existing
      ? settings.overrides.servers.map((item) => item.server_id === serverId ? { ...item, enabled, customized: true } : item)
      : settings.overrides.servers.concat({ server_id: serverId, enabled, customized: true });
    void save({ ...settings.overrides, servers });
  }

  const servers = useMemo(() => settings?.effective ?? [], [settings]);
  const server = selection ? servers.find((item) => item.id === selection.serverId) : undefined;
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const tools = useMemo(
    () => server?.tools.filter((tool) => !normalizedSearch || `${tool.title || ''} ${tool.name} ${tool.description}`.toLocaleLowerCase().includes(normalizedSearch)) ?? [],
    [normalizedSearch, server],
  );

  if (view === 'summary') {
    if (loading) return <div className="mcp-agent-tools-loading">{t('tools.mcp.loading')}</div>;
    if (!settings) return <div className="agent-tool-empty error-text" role="alert">{error || t('tools.mcp.loadFailed')}</div>;
    if (servers.length === 0) return <div className="agent-tool-empty">{t('tools.mcp.noServers')}</div>;

    return (
      <>
        {servers.map((server) => {
          const inherited = !settings.overrides.servers.some((item) => item.server_id === server.id)
            && !settings.overrides.tools.some((item) => item.server_id === server.id);
          const currentSelection = { serverId: server.id };
          return (
            <div className="agent-tool-row mcp-agent-tool-row" key={server.id}>
              <div className="agent-tool-row__body">
                <div className="agent-tool-row__title">
                  <strong>{server.name}</strong>
                  <span className="mini-status-badge mcp-badge"><Cable size={12} aria-hidden="true" />MCP</span>
                </div>
                <small>{server.description || t('tools.mcp.noDescription')}</small>
                <span className="agent-tool-row__source">
                  {inherited ? t('tools.inheritsDefault') : t('tools.agentOverride')}
                </span>
              </div>
              <div className="agent-tool-row__actions">
                <label className="inherit-control" title={t('tools.mcp.serverInheritanceDesc')}>
                  <input
                    type="checkbox"
                    aria-label={`${server.name} ${t('tools.inheritsDefault')}`}
                    checked={inherited}
                    disabled={saving}
                    onChange={(event) => setServerInherited(server.id, event.target.checked)}
                  />
                  <span>{t('tools.mcp.inheritServer')}</span>
                </label>
                <label className={`toggle-switch${inherited ? ' is-disabled' : ''}`} title={server.enabled ? t('tools.enabled') : t('tools.disabled')}>
                  <input
                    type="checkbox"
                    aria-label={`${server.name} ${server.enabled ? t('tools.enabled') : t('tools.disabled')}`}
                    checked={server.enabled}
                    disabled={saving || inherited}
                    onChange={(event) => setServerEnabled(server.id, event.target.checked)}
                  />
                  <span className="toggle-slider" />
                </label>
                <IconButton
                  type="button"
                  variant="ghost"
                  className="agent-tool-edit"
                  onClick={() => onEdit?.(currentSelection)}
                  disabled={saving || inherited}
                  label={t('tools.mcp.editServerTools')}
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

  if (!server) {
    return <div className="terminal-loading-state"><p>{t('tools.mcp.noServers')}</p></div>;
  }

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
            <h2>{server.name}</h2>
            <p>{server.description || t('tools.mcp.noDescription')}</p>
          </div>
        </div>
      </div>

      <div className="tool-settings-form terminal-settings-form">
        <section className="settings-card basic-settings-card">
          <div className="card-header mcp-tools-toolbar">
            <div className="settings-section-title">
              <h3>{t('tools.mcp.discoveredTools')}</h3>
              <p>{t('tools.mcp.discoveredToolsHint')}</p>
            </div>
            <div className="mcp-tools-toolbar-actions">
              <div className="mcp-search-input">
                <Search size={15} />
                <input
                  id="mcp-agent-tool-search"
                  aria-label={t('tools.mcp.searchTools')}
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder={t('tools.mcp.searchPlaceholder')}
                />
              </div>
            </div>
          </div>
          <div className="card-body">
            {tools.length > 0 ? (
              <div className="mcp-agent-tool-list">
                {tools.map((tool) => (
                  <div className={`setting-row mcp-agent-tool-row${tool.enabled ? '' : ' is-disabled'}`} key={tool.tool_id}>
                    <div className="setting-info mcp-agent-tool-copy">
                      <label>{toolLabel(tool)}</label>
                      <code>{tool.name}</code>
                      <McpToolDetails description={tool.description || t('tools.mcp.noToolDescription')} inputSchema={tool.input_schema} />
                    </div>
                    <label className="toggle-switch mcp-tool-toggle" title={tool.enabled ? t('tools.enabled') : t('tools.disabled')}>
                      <input
                        type="checkbox"
                        aria-label={`${toolLabel(tool)} ${t('tools.enabled')}`}
                        checked={tool.enabled}
                        disabled={saving || !server.enabled}
                        onChange={(event) => patchTool(server.id, tool.name, event.target.checked)}
                      />
                      <span className="toggle-slider" />
                    </label>
                  </div>
                ))}
              </div>
            ) : <div className="mcp-empty-tools">{server.tools.length === 0 ? t('tools.mcp.noTools') : t('tools.mcp.noMatchingTools')}</div>}
            {error ? <p className="error-text" role="alert">{error}</p> : null}
          </div>
        </section>
      </div>
    </div>
  );
}
