import { useMemo, useState } from 'react';
import { AlertCircle, ArrowLeft, CheckCheck, CircleCheck, CircleOff, Clock3, ExternalLink, LoaderCircle, LogIn, Pencil, RefreshCw, Search, ShieldCheck, Trash2, XCircle } from 'lucide-react';

import type { McpAuthState, McpServer, McpServerDraft } from '../../api';
import { updateMcpServer, updateMcpToolEnabled } from '../../api';
import { useI18n } from '../../i18n';
import { Button, FormField, IconButton, PaneHeader, useFeedback } from '../../components/ui';

function draftForServer(server: McpServer): McpServerDraft {
  return { id: server.id, name: server.name, description: server.description, enabled: server.enabled, provider: server.provider, provider_url: server.provider_url, logo_url: server.logo_url, tags: server.tags ?? [], transport: server.transport };
}

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
  const { confirm } = useFeedback();
  const [search, setSearch] = useState('');
  const [saving, setSaving] = useState(false);
  const normalized = search.trim().toLocaleLowerCase();
  const tools = useMemo(() => server.tools.filter((tool) => !normalized || `${tool.name} ${tool.description}`.toLocaleLowerCase().includes(normalized)), [normalized, server.tools]);
  const enabledCount = server.tools.filter((tool) => tool.enabled).length;

  async function toggleServer(enabled: boolean) {
    setSaving(true);
    try {
      const saved = await updateMcpServer(server.id, { ...draftForServer(server), enabled });
      onChanged(saved);
    } finally {
      setSaving(false);
    }
  }

  async function toggleTool(toolName: string, enabled: boolean) {
    setSaving(true);
    try {
      onChanged(await updateMcpToolEnabled(server.id, toolName, enabled));
    } finally {
      setSaving(false);
    }
  }

  async function setAllTools(enabled: boolean) {
    setSaving(true);
    try {
      let next = server;
      for (const tool of server.tools) {
        next = await updateMcpToolEnabled(server.id, tool.name, enabled);
        onChanged(next);
      }
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
            <Button variant="secondary" type="button" onClick={onRefresh} icon={<RefreshCw size={15} />}>{t('tools.mcp.refresh')}</Button>
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
              <div className="mcp-summary-item">
                <span>{t('tools.mcp.status')}</span>
                <strong className={`mcp-status mcp-status-${server.status}`}>
                  {server.status === 'ready' ? <CircleCheck size={15} /> : server.status === 'error' || server.status === 'offline' ? <AlertCircle size={15} /> : <CircleOff size={15} />}
                  {t(`tools.mcp.statuses.${server.status}`)}
                </strong>
              </div>
              <div className="mcp-summary-item"><span>{t('tools.mcp.toolCount')}</span><strong>{enabledCount}/{server.tools.length}</strong></div>
              <div className="mcp-summary-item"><span>{t('tools.mcp.lastSynced')}</span><strong>{server.last_synced_at ? new Date(server.last_synced_at).toLocaleString() : t('tools.mcp.never')}</strong></div>
            </div>
            {server.last_error ? <p className="mcp-error-line"><AlertCircle size={15} />{server.last_error}</p> : null}
          </div>
        </section>
        {server.transport.type === 'stdio' ? (
          <section className="settings-card mcp-auth-section">
            <div className="card-header">
              <div className="settings-section-title">
                <h3><span className="mcp-auth-heading"><ShieldCheck size={17} />{t('tools.mcp.authentication')}</span></h3>
                <p>{t('tools.mcp.authenticationHint')}</p>
              </div>
              <strong className={`mcp-auth-status mcp-auth-status-${authStatus}`}><AuthStatusIcon status={authStatus} />{t(`tools.mcp.authStatuses.${authStatus}`)}</strong>
            </div>
            <div className="card-body mcp-auth-body">
              <div className="mcp-auth-actions">
                <Button variant="secondary" type="button" disabled={authBusyOrSaving} onClick={onAuthAction} icon={<LogIn size={15} />}>{authBusy ? t('tools.mcp.authWorking') : authActionLabel}</Button>
                {authState?.authorize_url ? <Button variant="ghost" type="button" onClick={() => window.open(authState.authorize_url ?? '', '_blank', 'noopener,noreferrer')} icon={<ExternalLink size={15} />}>{t('tools.mcp.openAuthPage')}</Button> : null}
                <Button variant="ghost" type="button" disabled={authClearDisabled} onClick={() => void clearAuth()} icon={<Trash2 size={15} />}>{t('tools.mcp.clearAuth')}</Button>
              </div>
              <div className="mcp-auth-details">
                {authState?.last_success_at ? <span>{t('tools.mcp.lastAuthenticated')}: {new Date(authState.last_success_at).toLocaleString()}</span> : null}
                <span>{t('tools.mcp.authAutoRefresh')}</span>
              </div>
              {authState?.last_error ? <p className="mcp-error-line"><AlertCircle size={15} />{authState.last_error}</p> : null}
            </div>
          </section>
        ) : null}
        <section className="settings-card mcp-tools-section">
          <div className="card-header mcp-tools-toolbar">
            <div className="settings-section-title"><h3>{t('tools.mcp.discoveredTools')}</h3><p>{t('tools.mcp.discoveredToolsHint')}</p></div>
            <div className="mcp-tools-toolbar-actions">
              <FormField className="mcp-search-field" label={t('tools.mcp.searchTools')} htmlFor="mcp-tool-search">
                <div className="mcp-search-input"><Search size={15} /><input id="mcp-tool-search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder={t('tools.mcp.searchPlaceholder')} /></div>
              </FormField>
              <Button variant="ghost" type="button" disabled={saving || server.tools.length === 0} onClick={() => void setAllTools(true)} icon={<CheckCheck size={15} />}>{t('tools.mcp.enableAll')}</Button>
              <Button variant="ghost" type="button" disabled={saving || server.tools.length === 0} onClick={() => void setAllTools(false)} icon={<XCircle size={15} />}>{t('tools.mcp.disableAll')}</Button>
            </div>
          </div>
          <div className="card-body">
            {tools.length > 0 ? <div className="mcp-tool-list">{tools.map((tool) => (
              <div className={`mcp-tool-row${tool.enabled ? '' : ' is-disabled'}`} key={tool.qualified_name}>
                <div className="mcp-tool-copy">
                  <strong>{tool.title || tool.name}</strong>
                  <code>{tool.name}</code>
                  <small>{tool.description || t('tools.mcp.noToolDescription')}</small>
                  <details>
                    <summary>{t('tools.mcp.showSchema')}</summary>
                    <pre>{JSON.stringify(tool.input_schema, null, 2)}</pre>
                  </details>
                </div>
                <div className="toggle-switch" title={tool.enabled ? t('tools.enabled') : t('tools.disabled')}>
                  <input type="checkbox" aria-label={`${tool.name} ${tool.enabled ? t('tools.enabled') : t('tools.disabled')}`} checked={tool.enabled} disabled={saving || !server.enabled} onChange={(event) => void toggleTool(tool.name, event.target.checked)} />
                  <span className="toggle-slider" />
                </div>
              </div>
            ))}</div> : <div className="mcp-empty-tools"><XCircle size={22} /><span>{server.tools.length === 0 ? t('tools.mcp.noTools') : t('tools.mcp.noMatchingTools')}</span></div>}
          </div>
        </section>
      </div>
    </div>
  );
}
