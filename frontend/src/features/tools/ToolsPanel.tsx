import { useEffect, useState } from 'react';
import { AlertCircle, ArrowLeft, Cable, CircleCheck, CircleOff, Globe, ListChecks, LoaderCircle, Plus, Power, ShieldCheck, Terminal } from 'lucide-react';

import type { Agent } from '../../types';
import type { McpAuthState, McpServer, McpServerDraft, McpTransport } from '../../toolTypes';
import { clearMcpAuth, createMcpServer, deleteMcpServer, getMcpAuthState, getMcpServerSecrets, listMcpServers, reauthorizeMcpServer, refreshMcpServer, testMcpServer, updateMcpServer } from '../../api';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { AgentToolsOverview } from '../settings/TerminalToolSettings';
import { WebSearchToolSettings } from '../settings/WebSearchToolSettings';
import { TaskPlanningToolSettings } from '../settings/TaskPlanningToolSettings';
import { McpServerDialog } from './McpServerDialog';
import { McpServerIcon } from './McpServerIcon';
import { McpServerSettings } from './McpServerSettings';

interface ToolsPanelProps {
  agents: Agent[];
  onOpenMain: () => void;
  onBackToList: () => void;
  onOpenIpProxies: () => void;
}
type BuiltinToolKey = 'terminal' | 'web_search' | 'task_planning';
type ActiveTool = { kind: 'builtin'; key: BuiltinToolKey } | { kind: 'mcp'; id: string };

const toolDefs: {
  key: BuiltinToolKey;
  icon: typeof Terminal;
  getLabel: (t: (p: string) => string) => string;
  getDesc: (t: (p: string) => string) => string;
}[] = [
  { key: 'terminal', icon: Terminal, getLabel: (t) => t('tools.terminal.name'), getDesc: (t) => t('tools.terminal.description') },
  { key: 'web_search', icon: Globe, getLabel: (t) => t('tools.webSearch.name'), getDesc: (t) => t('tools.webSearch.description') },
  { key: 'task_planning', icon: ListChecks, getLabel: (t) => t('tools.taskPlanning.name'), getDesc: (t) => t('tools.taskPlanning.description') },
];

export function ToolsPanel({ agents, onOpenMain, onBackToList, onOpenIpProxies }: ToolsPanelProps) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const [activeTool, setActiveTool] = useState<ActiveTool>({ kind: 'builtin', key: 'terminal' });
  const [servers, setServers] = useState<McpServer[]>([]);
  const [loading, setLoading] = useState(true);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingServer, setEditingServer] = useState<McpServer | undefined>();
  const [editingSecrets, setEditingSecrets] = useState<McpTransport | undefined>();
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [authState, setAuthState] = useState<McpAuthState | null>(null);
  const [authBusy, setAuthBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listMcpServers().then((items) => { if (!cancelled) setServers(items); }).catch((error: unknown) => {
      if (!cancelled) toast.error(error instanceof Error ? error.message : t('tools.mcp.loadFailed'));
    }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [t, toast]);

  const builtin = activeTool.kind === 'builtin' ? toolDefs.find((item) => item.key === activeTool.key) : undefined;
  const activeServer = activeTool.kind === 'mcp' ? servers.find((server) => server.id === activeTool.id) : undefined;
  const activeServerId = activeTool.kind === 'mcp' ? activeTool.id : undefined;

  useEffect(() => {
    if (!activeServerId) {
      setAuthState(null);
      return;
    }
    let cancelled = false;
    const loadAuthState = () => {
      void getMcpAuthState(activeServerId).then((state) => {
        if (!cancelled) setAuthState(state);
      }).catch(() => {
        if (!cancelled) setAuthState(null);
      });
    };
    loadAuthState();
    const timer = window.setInterval(loadAuthState, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeServerId]);

  function selectBuiltin(key: BuiltinToolKey) {
    setActiveTool({ kind: 'builtin', key });
    onOpenMain();
  }

  function selectServer(id: string) {
    setActiveTool({ kind: 'mcp', id });
    onOpenMain();
  }

  function followBackgroundRefresh(serverId: string) {
    let attempts = 0;
    const poll = () => {
      attempts += 1;
      void listMcpServers().then((items) => {
        setServers(items);
        const server = items.find((item) => item.id === serverId);
        if (server?.status === 'connecting' && attempts < 30) {
          window.setTimeout(poll, 1000);
        }
      }).catch(() => {
        if (attempts < 30) window.setTimeout(poll, 1000);
      });
    };
    window.setTimeout(poll, 350);
  }

  function openCreateDialog() {
    setEditingServer(undefined);
    setEditingSecrets(undefined);
    setDialogOpen(true);
  }

  function openEditDialog() {
    if (!activeServer) return;
    setEditingServer(activeServer);
    setEditingSecrets(undefined);
    setDialogOpen(true);
    void getMcpServerSecrets(activeServer.id).then(setEditingSecrets).catch(() => {
      // The redacted server remains editable when the protected value lookup
      // is unavailable; unchanged secrets still use the preserve marker.
    });
  }

  async function saveServer(draft: McpServerDraft) {
    setSaving(true);
    try {
      const saved = draft.id ? await updateMcpServer(draft.id, draft) : await createMcpServer(draft);
      setServers((current) => current.some((server) => server.id === saved.id) ? current.map((server) => server.id === saved.id ? saved : server) : [...current, saved]);
      setActiveTool({ kind: 'mcp', id: saved.id });
      setDialogOpen(false);
      onOpenMain();
      followBackgroundRefresh(saved.id);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.mcp.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  async function testServer(draft: McpServerDraft) {
    setTesting(true);
    let savedId: string | undefined;
    try {
      const saved = draft.id ? await updateMcpServer(draft.id, draft) : await createMcpServer(draft);
      savedId = saved.id;
      setServers((current) => current.some((server) => server.id === saved.id) ? current.map((server) => server.id === saved.id ? saved : server) : [...current, saved]);
      const tested = await testMcpServer(saved.id);
      setServers((current) => current.map((server) => server.id === tested.id ? tested : server));
      setEditingServer(tested);
      toast.success(t('tools.mcp.testPassed'));
    } catch (error) {
      const items = await listMcpServers().catch(() => []);
      if (items.length > 0) {
        setServers(items);
        const failed = items.find((server) => server.id === savedId);
        if (failed) setEditingServer(failed);
      }
      toast.error(error instanceof Error ? error.message : t('tools.mcp.testFailed'));
    } finally {
      setTesting(false);
    }
  }

  async function refreshServer() {
    if (!activeServer) return;
    try {
      const refreshed = await refreshMcpServer(activeServer.id);
      setServers((current) => current.map((server) => server.id === refreshed.id ? refreshed : server));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.mcp.refreshFailed'));
      const items = await listMcpServers();
      setServers(items);
    }
  }

  async function reauthorizeServer() {
    if (!activeServer) return;
    setAuthBusy(true);
    try {
      const nextAuthState = await reauthorizeMcpServer(activeServer.id);
      setAuthState(nextAuthState);
      const items = await listMcpServers();
      setServers(items);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.mcp.authFailed'));
      const nextState = await getMcpAuthState(activeServer.id).catch(() => null);
      if (nextState) setAuthState(nextState);
    } finally {
      setAuthBusy(false);
    }
  }

  async function clearServerAuth() {
    if (!activeServer) return;
    setAuthBusy(true);
    try {
      setAuthState(await clearMcpAuth(activeServer.id));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.mcp.authClearFailed'));
    } finally {
      setAuthBusy(false);
    }
  }

  async function removeServer() {
    if (!activeServer) return;
    try {
      await deleteMcpServer(activeServer.id);
      setServers((current) => current.filter((server) => server.id !== activeServer.id));
      setActiveTool({ kind: 'builtin', key: 'terminal' });
      onOpenMain();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('tools.mcp.deleteFailed'));
    }
  }

  return (
    <>
      <PanelShell
        className="tools-panel"
        sidebar={(
          <div className="context-pane tools-nav">
            <PaneHeader
              className="mobile-pane-header"
              title={t('tools.areaTitle')}
              actions={<IconButton variant="ghost" type="button" label={t('tools.mcp.add')} onClick={openCreateDialog}><Plus size={18} /></IconButton>}
            />
            <div className="list-stack">
              {toolDefs.map((def) => (
                <ListItem
                  key={def.key}
                  type="button"
                  active={activeTool.kind === 'builtin' && activeTool.key === def.key}
                  title={<span className="tool-list-title"><span>{def.getLabel(t)}</span><span className="mini-status-badge"><ShieldCheck size={12} />{t('tools.builtinBadge')}</span></span>}
                  subtitle={def.getDesc(t)}
                  leading={<def.icon size={18} />}
                  onClick={() => selectBuiltin(def.key)}
                />
              ))}
              {loading ? <div className="mcp-list-loading">{t('tools.mcp.loading')}</div> : null}
              {servers.map((server) => (
                <ListItem
                  key={server.id}
                  type="button"
                  active={activeTool.kind === 'mcp' && activeTool.id === server.id}
                  title={<span className="tool-list-title"><span>{server.name}</span><span className="mini-status-badge mcp-badge"><Cable size={12} />MCP</span></span>}
                  subtitle={(
                    <span className="mcp-list-subtitle">
                      <span className="mcp-list-tools-count">{server.tools.filter((tool) => tool.enabled).length}/{server.tools.length} {t('tools.mcp.toolsLabel')}</span>
                      <span className="mcp-list-statuses">
                        <span
                          className={`mcp-list-status-icon mcp-list-connection mcp-list-connection-${server.status}`}
                          role="img"
                          aria-label={`${t('tools.mcp.status')}: ${t(`tools.mcp.statuses.${server.status}`)}`}
                          title={`${t('tools.mcp.status')}: ${t(`tools.mcp.statuses.${server.status}`)}`}
                        >
                          {server.status === 'ready' ? <CircleCheck size={15} aria-hidden="true" /> : server.status === 'connecting' ? <LoaderCircle size={15} aria-hidden="true" /> : server.status === 'error' ? <AlertCircle size={15} aria-hidden="true" /> : <CircleOff size={15} aria-hidden="true" />}
                        </span>
                        <span
                          className={`mcp-list-status-icon mcp-list-enabled-${server.enabled ? 'on' : 'off'}`}
                          role="img"
                          aria-label={`${t('tools.enabled')}: ${t(server.enabled ? 'tools.enabled' : 'tools.disabled')}`}
                          title={`${t('tools.enabled')}: ${t(server.enabled ? 'tools.enabled' : 'tools.disabled')}`}
                        >
                          <Power size={15} aria-hidden="true" />
                        </span>
                      </span>
                    </span>
                  )}
                  leading={<McpServerIcon logoUrl={server.logo_url} />}
                  onClick={() => selectServer(server.id)}
                />
              ))}
            </div>
          </div>
        )}
        main={(
          <div className="main-pane config-main">
            {activeServer ? (
              <McpServerSettings server={activeServer} authState={authState} authBusy={authBusy} onAuthAction={() => void reauthorizeServer()} onClearAuth={() => void clearServerAuth()} onChanged={(server) => setServers((current) => current.map((item) => item.id === server.id ? server : item))} onRefresh={() => void refreshServer()} onEdit={openEditDialog} onDelete={() => void removeServer()} onBackToList={onBackToList} />
            ) : (
              <>
                <PaneHeader
                  className="config-page-header"
                  leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
                  eyebrow={t('tools.areaTitle')}
                  title={builtin?.getLabel(t) ?? t('tools.title')}
                  description={builtin?.getDesc(t) ?? t('tools.description')}
                />
                {activeTool.kind === 'builtin' && activeTool.key === 'terminal' ? <AgentToolsOverview scope="defaults" agents={agents} onOpenIpProxies={onOpenIpProxies} /> : null}
                {activeTool.kind === 'builtin' && activeTool.key === 'web_search' ? <WebSearchToolSettings scope="defaults" agents={agents} onOpenIpProxies={onOpenIpProxies} /> : null}
                {activeTool.kind === 'builtin' && activeTool.key === 'task_planning' ? <TaskPlanningToolSettings scope="defaults" agents={agents} /> : null}
              </>
            )}
          </div>
        )}
      />
      <McpServerDialog open={dialogOpen} server={editingServer} secrets={editingSecrets} existingServers={servers} saving={saving} testing={testing} onClose={() => setDialogOpen(false)} onSave={saveServer} onTest={testServer} />
    </>
  );
}
