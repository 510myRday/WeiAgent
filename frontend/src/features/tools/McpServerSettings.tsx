import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, ArrowLeft, CheckCheck, CircleCheck, CircleOff, Clock3, ExternalLink, LoaderCircle, LogIn, Pencil, RefreshCw, Search, ShieldCheck, Trash2, XCircle } from 'lucide-react';

import type { McpAuthState, McpServer } from '../../api';
import { updateMcpServerEnabled, updateMcpToolEnabled, updateMcpToolsEnabled } from '../../api';
import { useI18n } from '../../i18n';
import { Button, IconButton, PaneHeader, useFeedback } from '../../components/ui';
import { McpToolDetails } from './McpToolDetails';

function AuthStatusIcon({ status }: { status: McpAuthState['status'] }) {
  if (status === 'connected' || status === 'authorized') return <CircleCheck size={15} />;
  if (status === 'starting' || status === 'auth_pending' || status === 'browser_opened' || status === 'waiting_callback') return <LoaderCircle className="mcp-auth-status-spin" size={15} />;
  if (status === 'auth_timeout' || status === 'auth_failed' || status === 'launch_failed' || status === 'init_failed') return <AlertCircle size={15} />;
  if (status === 'unsupported') return <CircleOff size={15} />;
  return <Clock3 size={15} />;
}

export function McpServerSettings({
  server,
  authState,
  authBusy,
  onAuthAction,
  onClearAuth,
  onChanged,
  onRefresh,
  onEdit,
  onDelete,
  onBackToList,
}: {
  server: McpServer;
  authState: McpAuthState | null;
  authBusy: boolean;
  onAuthAction: () => void;
  onClearAuth: () => void;
  onChanged: (server: McpServer) => void;
  onRefresh: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onBackToList: () => void;
}) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const [search, setSearch] = useState('');
  const [saving, setSaving] = useState(false);
  const [selectedToolNames, setSelectedToolNames] = useState<Set<string>>(new Set());
  const selectAllRef = useRef<HTMLInputElement>(null);
  const normalized = search.trim().toLocaleLowerCase();
  const tools = useMemo(() => server.tools.filter((tool) => !normalized || `${tool.name} ${tool.description}`.toLocaleLowerCase().includes(normalized)), [normalized, server.tools]);
  const enabledCount = server.tools.filter((tool) => tool.enabled).length;
  const allVisibleSelected = tools.length > 0 && tools.every((tool) => selectedToolNames.has(tool.name));
  const someVisibleSelected = tools.some((tool) => selectedToolNames.has(tool.name));
  const selectedToolCount = tools.filter((tool) => selectedToolNames.has(tool.name)).length;

  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = someVisibleSelected && !allVisibleSelected;
    }
  }, [allVisibleSelected, someVisibleSelected]);

  useEffect(() => {
    const visibleToolNames = new Set(tools.map((tool) => tool.name));
    setSelectedToolNames((current) => {
      const next = new Set(Array.from(current).filter((name) => visibleToolNames.has(name)));
      return next.size === current.size ? current : next;
    });
  }, [tools]);

  async function toggleServer(enabled: boolean) {
    const previous = server;
    onChanged({ ...server, enabled });
    setSaving(true);
    try {
      const saved = await updateMcpServerEnabled(server.id, enabled);
      onChanged(saved);
    } catch (error) {
      onChanged(previous);
      toast.error(error instanceof Error ? error.message : t('tools.mcp.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  async function toggleTool(toolName: string, enabled: boolean) {
    const previous = server;
    onChanged({
      ...server,
      tools: server.tools.map((tool) => tool.name === toolName ? { ...tool, enabled } : tool),
    });
    setSaving(true);
    try {
      onChanged(await updateMcpToolEnabled(server.id, toolName, enabled));
    } catch (error) {
      onChanged(previous);
      toast.error(error instanceof Error ? error.message : t('tools.mcp.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  function toggleToolSelection(toolName: string, selected: boolean) {
    setSelectedToolNames((current) => {
      const next = new Set(current);
      if (selected) next.add(toolName);
      else next.delete(toolName);
      return next;
    });
  }

  function toggleVisibleSelection(selected: boolean) {
    setSelectedToolNames(selected ? new Set(tools.map((tool) => tool.name)) : new Set());
  }

  async function setSelectedTools(enabled: boolean) {
    const selectedTools = tools.filter((tool) => selectedToolNames.has(tool.name));
    if (selectedTools.length === 0) return;
    const previous = server;
    setSaving(true);
    try {
      const next = await updateMcpToolsEnabled(server.id, selectedTools.map((tool) => tool.name), enabled);
      onChanged(next);
      setSelectedToolNames(new Set());
    } catch (error) {
      onChanged(previous);
      toast.error(error instanceof Error ? error.message : t('tools.mcp.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  async function removeServer() {
    const accepted = await confirm({ title: t('tools.mcp.deleteTitle'), message: t('tools.mcp.deleteConfirm'), confirmLabel: t('common.delete'), tone: 'danger' });
    if (accepted) onDelete();
  }

  async function clearAuth() {
    const accepted = await confirm({ title: t('tools.mcp.authClearTitle'), message: t('tools.mcp.authClearConfirm'), confirmLabel: t('tools.mcp.clearAuth'), tone: 'danger' });
    if (accepted) onClearAuth();
  }

  const authStatus = authState?.status ?? 'idle';
  const authBusyOrSaving = authBusy || saving;
  const authClearDisabled = authBusyOrSaving || !authState || authStatus === 'unsupported';
  const authActionLabel = authStatus === 'connected' || authStatus === 'authorized'
    ? t('tools.mcp.reauthorize')
    : t('tools.mcp.startLogin');
  const authSupported = server.transport.type === 'stdio' || ('oauth' in server.transport && Boolean(server.transport.oauth));
  const lastOperation = server.diagnostics?.last_operation;
  const lastOperationValue = lastOperation
    ? `${lastOperation}${server.diagnostics?.last_duration_ms != null ? ` (${server.diagnostics.last_duration_ms} ms)` : ''}; ${t('tools.mcp.retryCount')}: ${server.diagnostics?.retry_count ?? 0}`
    : t('tools.mcp.never');

  return (
    <div className="tool-settings-page mcp-server-settings">
      <PaneHeader
        className="config-page-header"
        leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
        eyebrow={t('tools.mcp.eyebrow')}
        title={server.name}
        description={server.description || t('tools.mcp.noDescription')}
        actions={(
          <div className="mcp-page-actions">
            <IconButton variant="default" type="button" onClick={onRefresh} label={t('tools.mcp.refresh')}>
              <RefreshCw size={15} />
            </IconButton>
            <IconButton type="button" label={t('tools.mcp.edit')} onClick={onEdit}><Pencil size={17} /></IconButton>
            <IconButton variant="danger" type="button" label={t('tools.mcp.delete')} onClick={() => void removeServer()}><Trash2 size={17} /></IconButton>
          </div>
        )}
      />
      <div className="tool-settings-form">
        <section className="settings-card mcp-summary-section">
          <div className="card-header">
            <div className="settings-section-title">
              <h3>{t('tools.mcp.connectionOverview')}</h3>
              <p>{t('tools.mcp.connectionOverviewHint')}</p>
            </div>
            <label className="mcp-summary-toggle">
              <span>{t('tools.mcp.enabled')}</span>
              <span className="toggle-switch">
                <input type="checkbox" checked={server.enabled} disabled={saving} onChange={(event) => void toggleServer(event.target.checked)} />
                <span className="toggle-slider" />
              </span>
            </label>
          </div>
          <div className="card-body">
            <div className="mcp-summary-grid">
              <div className="mcp-summary-item mcp-summary-status-item">
                <span>{t('tools.mcp.status')}</span>
                <strong className={`mcp-status mcp-status-${server.status}`}>
                  {server.status === 'ready' ? <CircleCheck size={15} /> : server.status === 'error' || server.status === 'offline' ? <AlertCircle size={15} /> : <CircleOff size={15} />}
                  {t(`tools.mcp.statuses.${server.status}`)}
                </strong>
              </div>
              <div className="mcp-summary-item"><span>{t('tools.mcp.toolCount')}</span><strong>{enabledCount}/{server.tools.length}</strong></div>
              <div className="mcp-summary-item"><span>{t('tools.mcp.lastSynced')}</span><strong>{server.last_synced_at ? new Date(server.last_synced_at).toLocaleString() : t('tools.mcp.never')}</strong></div>
              <div className="mcp-summary-item"><span>{t('tools.mcp.parallelToolCalls')}</span><strong>{server.supports_parallel_tool_calls ? t('tools.enabled') : t('tools.disabled')}</strong></div>
              <div className="mcp-summary-item"><span>{t('tools.mcp.protocolVersion')}</span><strong>{server.diagnostics?.protocol_version || t('tools.mcp.unknown')}</strong></div>
              <div className="mcp-summary-item mcp-summary-diagnostics-item"><span>{t('tools.mcp.lastOperation')}</span><strong title={lastOperationValue}>{lastOperationValue}</strong></div>
            </div>
            {server.last_error ? <p className="mcp-error-line"><AlertCircle size={15} />{server.last_error}</p> : null}
          </div>
        </section>
        {authSupported ? (
          <section className="settings-card mcp-auth-section">
            <div className="card-header">
              <div className="settings-section-title">
                <h3><span className="mcp-auth-heading"><ShieldCheck size={17} />{t('tools.mcp.authentication')}</span></h3>
                <p>{t('tools.mcp.authenticationHint')}</p>
              </div>
              <strong className={`mcp-auth-status mcp-auth-status-${authStatus}`}><AuthStatusIcon status={authStatus} />{t(`tools.mcp.authStatuses.${authStatus}`)}</strong>
            </div>
            <div className="card-body mcp-auth-body">
              <div className="mcp-auth-details">
                {authState?.last_success_at ? <span>{t('tools.mcp.lastAuthenticated')}: {new Date(authState.last_success_at).toLocaleString()}</span> : null}
                <span>{t('tools.mcp.authAutoRefresh')}</span>
              </div>
              <div className="mcp-auth-actions">
                <Button variant="secondary" type="button" disabled={authBusyOrSaving} onClick={onAuthAction} icon={<LogIn size={15} />}>{authBusy ? t('tools.mcp.authWorking') : authActionLabel}</Button>
                {authState?.authorize_url ? <Button variant="ghost" type="button" onClick={() => window.open(authState.authorize_url ?? '', '_blank', 'noopener,noreferrer')} icon={<ExternalLink size={15} />}>{t('tools.mcp.openAuthPage')}</Button> : null}
                <Button variant="ghost" type="button" disabled={authClearDisabled} onClick={() => void clearAuth()} icon={<Trash2 size={15} />}>{t('tools.mcp.clearAuth')}</Button>
              </div>
              {authState?.last_error ? <p className="mcp-error-line"><AlertCircle size={15} />{authState.last_error}</p> : null}
            </div>
          </section>
        ) : null}
        <section className="settings-card mcp-tools-section">
          <div className="card-header mcp-tools-toolbar">
            <div className="settings-section-title"><h3>{t('tools.mcp.discoveredTools')}</h3><p>{t('tools.mcp.discoveredToolsHint')}</p></div>
            <div className="mcp-tools-toolbar-actions">
              <div className="mcp-search-input">
                <Search size={15} />
                <input id="mcp-tool-search" aria-label={t('tools.mcp.searchTools')} value={search} onChange={(event) => setSearch(event.target.value)} placeholder={t('tools.mcp.searchPlaceholder')} />
              </div>
            </div>
          </div>
          {tools.length > 0 ? (
            <div className={`mcp-tool-selection-bar${selectedToolCount > 0 ? ' has-selection' : ''}`}>
              <label className="mcp-select-all-control">
                <input ref={selectAllRef} type="checkbox" checked={allVisibleSelected} onChange={(event) => toggleVisibleSelection(event.target.checked)} aria-label={t('tools.mcp.selectAllTools')} />
                <span>{t('tools.mcp.selectAllTools')}</span>
              </label>
              {selectedToolCount > 0 ? (
                <div className="mcp-tool-selection-controls">
                  <span className="mcp-selected-count">{t('tools.mcp.selectedTools', { count: selectedToolCount })}</span>
                  <span className="mcp-tool-action-divider" aria-hidden="true" />
                  <Button className="mcp-tool-bulk-action mcp-tool-bulk-action-enable" variant="ghost" type="button" disabled={saving || !server.enabled} onClick={() => void setSelectedTools(true)} icon={<CheckCheck size={15} />}>{t('tools.mcp.enableSelected')}</Button>
                  <Button className="mcp-tool-bulk-action mcp-tool-bulk-action-disable" variant="ghost" type="button" disabled={saving || !server.enabled} onClick={() => void setSelectedTools(false)} icon={<XCircle size={15} />}>{t('tools.mcp.disableSelected')}</Button>
                </div>
              ) : null}
            </div>
          ) : null}
          <div className="card-body">
            {tools.length > 0 ? <div className="mcp-tool-list">{tools.map((tool) => (
              <div className={`mcp-tool-row${tool.enabled ? '' : ' is-disabled'}${selectedToolNames.has(tool.name) ? ' is-selected' : ''}`} key={tool.tool_id}>
                <label className="mcp-tool-selection" aria-label={t('tools.mcp.selectTool', { name: tool.title || tool.name })}>
                  <input type="checkbox" checked={selectedToolNames.has(tool.name)} disabled={saving} onChange={(event) => toggleToolSelection(tool.name, event.target.checked)} />
                </label>
                <div className="mcp-tool-copy">
                  <strong>{tool.title || tool.name}</strong>
                  <McpToolDetails description={tool.description || t('tools.mcp.noToolDescription')} inputSchema={tool.input_schema} />
                </div>
                <label className="toggle-switch mcp-tool-toggle" title={tool.enabled ? t('tools.enabled') : t('tools.disabled')}>
                  <input type="checkbox" aria-label={`${tool.name} ${tool.enabled ? t('tools.enabled') : t('tools.disabled')}`} checked={tool.enabled} disabled={saving || !server.enabled} onChange={(event) => void toggleTool(tool.name, event.target.checked)} />
                  <span className="toggle-slider" />
                </label>
              </div>
            ))}</div> : <div className="mcp-empty-tools"><XCircle size={22} /><span>{server.tools.length === 0 ? t('tools.mcp.noTools') : t('tools.mcp.noMatchingTools')}</span></div>}
          </div>
        </section>
      </div>
    </div>
  );
}
