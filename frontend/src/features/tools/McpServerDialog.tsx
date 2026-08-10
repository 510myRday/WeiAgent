import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, Braces, Cable, CheckCircle2, ChevronDown, ChevronUp, Code2, Eye, EyeOff, HelpCircle, Plus, Save, TestTube2, X } from 'lucide-react';

import type { McpServer, McpServerDraft, McpTransport } from '../../api';
import { useI18n } from '../../i18n';
import { Button, Dialog, FormField, IconButton } from '../../components/ui';

type EditorMode = 'visual' | 'json';
type EnvironmentRow = { id: string; key: string; value: string; preserved: boolean; visible: boolean };

const EMPTY_DRAFT: McpServerDraft = {
  name: '',
  description: '',
  enabled: true,
  provider: null,
  provider_url: null,
  logo_url: null,
  tags: [],
  transport: { type: 'stdio', command: '', args: [], cwd: null, env: {}, long_running: false, timeout_ms: 60000 },
};

function redactTransport(transport: McpTransport): McpTransport {
  if (transport.type === 'stdio') {
    return { ...transport, env: Object.fromEntries(Object.entries(transport.env).map(([key, value]) => [key, value ? '__preserve__' : value])) };
  }
  return {
    ...transport,
    bearer_token: transport.bearer_token ? '__preserve__' : transport.bearer_token,
    headers: Object.fromEntries(Object.entries(transport.headers).map(([key, value]) => [key, value ? '__preserve__' : value])),
  };
}

function argsToText(args: string[]): string {
  return args.map((arg) => (arg.includes(' ') ? `"${arg}"` : arg)).join(' ');
}

function textToArgs(value: string): string[] {
  return value.match(/(?:[^\s"]+|"[^"]*")+/g)?.map((arg) => arg.replace(/^"|"$/g, '')) ?? [];
}

function normalizeStdioArgs(args: string[]): string[] {
  if (args.length !== 1 || !args[0].trim().startsWith('-') || !/[\s]/.test(args[0])) return args;
  return textToArgs(args[0]);
}

function draftFromServer(server?: McpServer, secrets?: McpTransport): McpServerDraft {
  if (!server) return structuredClone(EMPTY_DRAFT);
  const transport = secrets ?? redactTransport(server.transport);
  return {
    id: server.id,
    name: server.name,
    description: server.description,
    enabled: server.enabled,
    provider: server.provider ?? null,
    provider_url: server.provider_url ?? null,
    logo_url: server.logo_url ?? null,
    tags: server.tags ?? [],
    transport: transport.type === 'stdio'
      ? { ...transport, args: normalizeStdioArgs(transport.args) }
      : transport,
  };
}

function parseLines(value: string): string[] {
  return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

function parseTags(value: string): string[] {
  return value.split(/[,，]/).map((tag) => tag.trim()).filter(Boolean);
}

function mapEntries(value: string): Record<string, string> {
  return Object.fromEntries(parseLines(value).map((line) => {
    const separator = line.indexOf('=');
    return separator < 0 ? [line, ''] : [line.slice(0, separator).trim(), line.slice(separator + 1).trim()];
  }).filter(([key]) => key));
}

function entriesText(entries: Record<string, string>): string {
  return Object.entries(entries).map(([key, value]) => `${key}=${value}`).join('\n');
}

function serializeJsonDraft(draft: McpServerDraft): string {
  const jsonDraft = { ...draft };
  Reflect.deleteProperty(jsonDraft, 'id');
  jsonDraft.transport = redactTransport(draft.transport);
  return JSON.stringify(jsonDraft, null, 2);
}

function getStdioArgsText(transport: McpTransport): string {
  return transport.type === 'stdio' ? argsToText(normalizeStdioArgs(transport.args ?? [])) : '';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function validateMcpDraft(value: unknown, t: (key: string) => string): string {
  if (!isRecord(value) || typeof value.name !== 'string' || !value.name.trim()) return t('tools.mcp.jsonNameError');
  if (!isRecord(value.transport) || typeof value.transport.type !== 'string') return t('tools.mcp.jsonTransportError');

  const transport = value.transport;
  const transportType = typeof transport.type === 'string' ? transport.type : '';
  if (!['stdio', 'streamable_http', 'sse'].includes(transportType)) return t('tools.mcp.jsonTransportError');
  if (value.tags !== undefined && (!Array.isArray(value.tags) || value.tags.some((item) => typeof item !== 'string'))) return t('tools.mcp.jsonTagsError');
  if (transportType === 'stdio') {
    if (typeof transport.command !== 'string' || !transport.command.trim()) return t('tools.mcp.jsonCommandError');
    if (transport.args !== undefined && (!Array.isArray(transport.args) || transport.args.some((item) => typeof item !== 'string'))) return t('tools.mcp.jsonArrayError');
    if (transport.timeout_ms !== undefined && (typeof transport.timeout_ms !== 'number' || !Number.isFinite(transport.timeout_ms))) return t('tools.mcp.jsonTimeoutError');
  } else {
    if (typeof transport.url !== 'string' || !transport.url.trim()) return t('tools.mcp.jsonUrlError');
    if (transport.timeout_ms !== undefined && (typeof transport.timeout_ms !== 'number' || !Number.isFinite(transport.timeout_ms))) return t('tools.mcp.jsonTimeoutError');
  }
  return '';
}

export function McpServerDialog({
  open,
  server,
  secrets,
  existingServers = [],
  saving,
  testing,
  onClose,
  onSave,
  onTest,
}: {
  open: boolean;
  server?: McpServer;
  secrets?: McpTransport;
  existingServers?: McpServer[];
  saving: boolean;
  testing: boolean;
  onClose: () => void;
  onSave: (draft: McpServerDraft) => Promise<void>;
  onTest: (draft: McpServerDraft) => Promise<void>;
}) {
  const { t } = useI18n();
  const [mode, setMode] = useState<EditorMode>('visual');
  const [draft, setDraft] = useState<McpServerDraft>(EMPTY_DRAFT);
  const [jsonText, setJsonText] = useState('');
  const [jsonError, setJsonError] = useState('');
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [stdioArgsText, setStdioArgsText] = useState('');
  const [environmentRows, setEnvironmentRows] = useState<EnvironmentRow[]>([]);
  const environmentRowSequence = useRef(0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const next = draftFromServer(server, secrets);
    setDraft(next);
    setJsonText(serializeJsonDraft(next));
    setJsonError('');
    setMode('visual');
    setAdvancedOpen(Boolean(next.provider || next.provider_url || next.logo_url || next.tags.length || (next.transport.type === 'stdio' && next.transport.cwd)));
    setStdioArgsText(getStdioArgsText(next.transport));
    setEnvironmentRows(createEnvironmentRows(next.transport));
  }, [open, server, secrets]);

  const transportType = draft.transport.type;
  const stdioTransport = draft.transport.type === 'stdio' ? draft.transport : undefined;
  const httpTransport = draft.transport.type !== 'stdio' ? draft.transport : undefined;
  const headerText = httpTransport ? entriesText(httpTransport.headers) : '';
  const timeoutSeconds = stdioTransport
    ? Math.round((stdioTransport.timeout_ms ?? 60000) / 1000)
    : Math.round((httpTransport?.timeout_ms ?? 30000) / 1000);

  function updateDraft(patch: Partial<McpServerDraft>) {
    const next = { ...draft, ...patch };
    setDraft(next);
    setJsonText(serializeJsonDraft(next));
    setJsonError('');
  }

  function updateTransport(next: McpTransport) {
    updateDraft({ transport: next });
    setStdioArgsText(getStdioArgsText(next));
    setEnvironmentRows(createEnvironmentRows(next));
  }

  function updateTimeoutSeconds(value: string) {
    const fallbackSeconds = stdioTransport ? 60 : 30;
    const seconds = Number(value) || fallbackSeconds;
    updateTransport({
      ...draft.transport,
      timeout_ms: Math.min(120000, Math.max(1000, seconds * 1000)),
    });
  }

  function renderTimeoutField() {
    return (
      <FormField
        className={stdioTransport ? undefined : 'mcp-advanced-timeout-wide'}
        label={t('tools.mcp.timeoutLabel')}
        htmlFor="mcp-timeout"
      >
        <div className="mcp-number-input">
          <input id="mcp-timeout" type="number" min={1} max={120} value={timeoutSeconds} onChange={(event) => updateTimeoutSeconds(event.target.value)} />
        </div>
      </FormField>
    );
  }

  function updateStdioArgs(value: string) {
    if (!stdioTransport) return;
    setStdioArgsText(value);
    updateDraft({ transport: { ...stdioTransport, args: textToArgs(value) } });
  }

  function createEnvironmentRows(transport: McpTransport): EnvironmentRow[] {
    if (transport.type !== 'stdio') return [];
    return Object.entries(transport.env ?? {}).map(([key, value]) => ({
      id: `mcp-env-${environmentRowSequence.current++}`,
      key,
      value,
      preserved: value === '__preserve__',
      visible: false,
    }));
  }

  function environmentFromRows(rows: EnvironmentRow[]): Record<string, string> {
    return Object.fromEntries(rows
      .map((row) => [row.key.trim(), row.preserved ? '__preserve__' : row.value.trim()] as const)
      .filter(([key]) => key));
  }

  function updateEnvironmentRows(nextRows: EnvironmentRow[]) {
    if (!stdioTransport) return;
    setEnvironmentRows(nextRows);
    updateDraft({ transport: { ...stdioTransport, env: environmentFromRows(nextRows) } });
  }

  function addEnvironmentRow() {
    updateEnvironmentRows([
      ...environmentRows,
      { id: `mcp-env-${environmentRowSequence.current++}`, key: '', value: '', preserved: false, visible: false },
    ]);
  }

  function updateEnvironmentRow(id: string, patch: Partial<EnvironmentRow>) {
    updateEnvironmentRows(environmentRows.map((row) => row.id === id ? { ...row, ...patch } : row));
  }

  function removeEnvironmentRow(id: string) {
    updateEnvironmentRows(environmentRows.filter((row) => row.id !== id));
  }

  function syncStdioEditorText(transport: McpTransport) {
    setStdioArgsText(getStdioArgsText(transport));
    setEnvironmentRows(createEnvironmentRows(transport));
  }

  function handleJsonChange(value: string) {
    setJsonText(value);
    try {
      const parsed = JSON.parse(value) as McpServerDraft;
      const validationError = validateMcpDraft(parsed, t);
      if (validationError) throw new Error(validationError);
      const nextDraft = {
        ...parsed,
        id: draft.id,
        provider: parsed.provider ?? null,
        provider_url: parsed.provider_url ?? null,
        logo_url: parsed.logo_url ?? null,
        tags: parsed.tags ?? [],
      };
      setDraft(nextDraft);
      syncStdioEditorText(nextDraft.transport);
      setJsonError('');
    } catch (error) {
      setJsonError(error instanceof Error ? error.message : t('tools.mcp.jsonInvalid'));
    }
  }

  function formatJson() {
    try {
      const parsed = JSON.parse(jsonText) as McpServerDraft;
      const validationError = validateMcpDraft(parsed, t);
      const nextDraft = { ...parsed, id: draft.id, provider: parsed.provider ?? null, provider_url: parsed.provider_url ?? null, logo_url: parsed.logo_url ?? null, tags: parsed.tags ?? [] };
      const formatted = serializeJsonDraft(nextDraft);
      setJsonText(formatted);
      if (!validationError) {
        setDraft(nextDraft);
        syncStdioEditorText(nextDraft.transport);
      }
      setJsonError(validationError);
    } catch (error) {
      setJsonError(error instanceof Error ? error.message : t('tools.mcp.jsonInvalid'));
    }
  }

  function switchMode(nextMode: EditorMode) {
    setMode(nextMode);
  }

  function handleJsonKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      formatJson();
      return;
    }
    if (event.key !== 'Tab') return;
    event.preventDefault();
    const textarea = event.currentTarget;
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const next = `${jsonText.slice(0, start)}  ${jsonText.slice(end)}`;
    handleJsonChange(next);
    requestAnimationFrame(() => {
      textarea.selectionStart = start + 2;
      textarea.selectionEnd = start + 2;
    });
  }

  function syncEditorScroll(event: React.UIEvent<HTMLTextAreaElement>) {
    if (gutterRef.current) gutterRef.current.scrollTop = event.currentTarget.scrollTop;
  }

  const draftError = validateMcpDraft(draft, t);
  const nameConflict = existingServers.some((item) => item.id !== server?.id && item.name.trim().toLocaleLowerCase() === draft.name.trim().toLocaleLowerCase());
  const nameConflictError = nameConflict ? t('tools.mcp.nameConflict') : '';
  const effectiveJsonError = jsonError || draftError || nameConflictError;
  const canSubmit = useMemo(() => !effectiveJsonError && !saving && !testing, [effectiveJsonError, saving, testing]);
  const jsonLineCount = Math.max(1, jsonText.split(/\r?\n/).length);

  return (
    <Dialog
      open={open}
      title={server ? t('tools.mcp.editTitle') : t('tools.mcp.addTitle')}
      description={t('tools.mcp.dialogDescription')}
      icon={<Cable size={21} />}
      closeLabel={t('common.close')}
      onClose={onClose}
      size="xl"
      className="mcp-server-dialog"
      bodyClassName="mcp-server-dialog__body"
      footer={(
        <>
          <Button variant="secondary" type="button" disabled={!canSubmit} onClick={() => void onTest(draft)} icon={<TestTube2 size={15} />}>
            {testing ? t('tools.mcp.testing') : t('tools.mcp.testConnection')}
          </Button>
          <Button variant="primary" type="button" disabled={!canSubmit} onClick={() => void onSave(draft)} icon={<Save size={15} />}>
            {saving ? t('common.saving') : t('common.save')}
          </Button>
        </>
      )}
    >
      <div className="mcp-editor-toolbar">
        <div className="mcp-editor-mode" role="tablist" aria-label={t('tools.mcp.editorMode')}>
          <button type="button" className={mode === 'visual' ? 'is-active' : ''} onClick={() => switchMode('visual')} role="tab" aria-selected={mode === 'visual'}><Eye size={15} />{t('tools.mcp.visualMode')}</button>
          <button type="button" className={mode === 'json' ? 'is-active' : ''} onClick={() => switchMode('json')} role="tab" aria-selected={mode === 'json'}><Code2 size={15} />{t('tools.mcp.jsonMode')}</button>
        </div>
        <label className="mcp-enable-row">
          <span>{t('tools.mcp.enabled')}</span>
          <span className="toggle-switch mcp-enable-toggle">
            <input type="checkbox" checked={draft.enabled} onChange={(event) => updateDraft({ enabled: event.target.checked })} />
            <span className="toggle-slider" />
          </span>
        </label>
      </div>
      {mode === 'json' ? (
        <FormField label={t('tools.mcp.jsonConfig')} htmlFor="mcp-json-config" hint={effectiveJsonError || t('tools.mcp.jsonHint')}>
          <div className={`mcp-json-editor-shell${effectiveJsonError ? ' has-error' : ''}`}>
            <div ref={gutterRef} className="mcp-json-gutter" aria-hidden="true">
              {Array.from({ length: jsonLineCount }, (_, index) => <span key={index}>{index + 1}</span>)}
            </div>
            <textarea
              ref={textareaRef}
              id="mcp-json-config"
              className="mcp-json-editor"
              value={jsonText}
              onChange={(event) => handleJsonChange(event.target.value)}
              onKeyDown={handleJsonKeyDown}
              onScroll={syncEditorScroll}
              rows={22}
              spellCheck={false}
              aria-invalid={Boolean(effectiveJsonError)}
            />
          </div>
          <div className="mcp-json-toolbar">
            <span className={`mcp-json-status${effectiveJsonError ? ' is-error' : ' is-valid'}`} role={effectiveJsonError ? 'alert' : 'status'}>
              {effectiveJsonError ? <AlertCircle size={14} /> : <CheckCircle2 size={14} />}
              {effectiveJsonError || t('tools.mcp.jsonValid')}
            </span>
            <Button variant="ghost" type="button" disabled={saving || testing} onClick={formatJson} icon={<Braces size={15} />}>
              {t('tools.mcp.formatJson')}
            </Button>
          </div>
        </FormField>
      ) : (
        <div className="mcp-visual-form">
          <section className="mcp-form-section">
            <FormField label={t('tools.mcp.name')} htmlFor="mcp-name" required hint={nameConflictError || undefined}>
              <input id="mcp-name" className={nameConflict ? 'mcp-name-conflict' : undefined} value={draft.name} placeholder={t('tools.mcp.namePlaceholder')} onChange={(event) => updateDraft({ name: event.target.value })} required aria-invalid={nameConflict} />
            </FormField>
            <FormField label={t('tools.mcp.description')} htmlFor="mcp-description" hint={t('tools.mcp.descriptionHint')}>
              <textarea id="mcp-description" value={draft.description} placeholder={t('tools.mcp.descriptionPlaceholder')} onChange={(event) => updateDraft({ description: event.target.value })} rows={3} />
            </FormField>
            <FormField label={t('tools.mcp.transport')} htmlFor="mcp-transport" hint={t('tools.mcp.transportHint')}>
              <select id="mcp-transport" value={transportType} onChange={(event) => {
                const type = event.target.value as McpTransport['type'];
                updateTransport(type === 'stdio'
                  ? { type: 'stdio', command: '', args: [], cwd: null, env: {}, long_running: false, timeout_ms: 60000 }
                  : { type, url: '', bearer_token: null, headers: {}, timeout_ms: 30000 });
              }}>
                <option value="stdio">{t('tools.mcp.transportStdio')}</option>
                <option value="streamable_http">{t('tools.mcp.transportHttp')}</option>
                <option value="sse">{t('tools.mcp.transportSse')}</option>
              </select>
            </FormField>
          </section>
          {stdioTransport ? (
            <section className="mcp-form-section">
              <FormField label={t('tools.mcp.command')} htmlFor="mcp-command" required hint={t('tools.mcp.commandHint')}><input id="mcp-command" value={stdioTransport.command} placeholder={t('tools.mcp.commandPlaceholder')} onChange={(event) => updateTransport({ ...stdioTransport, command: event.target.value })} /></FormField>
              <FormField label={<span className="mcp-label-with-help">{t('tools.mcp.args')}<span title={t('tools.mcp.argsHelp')}><HelpCircle size={14} /></span></span>} htmlFor="mcp-args" hint={t('tools.mcp.argsHint')}><input id="mcp-args" value={stdioArgsText} placeholder={t('tools.mcp.argsPlaceholder')} onChange={(event) => updateStdioArgs(event.target.value)} /></FormField>
              <FormField
                label={<span className="mcp-label-with-help">{t('tools.mcp.environment')}<span title={t('tools.mcp.environmentHelp')}><HelpCircle size={14} /></span></span>}
                action={<IconButton variant="ghost" type="button" label={t('tools.mcp.addEnvironment')} onClick={addEnvironmentRow}><Plus size={15} /></IconButton>}
                hint={t('tools.mcp.keyValueHint')}
              >
                <div className="mcp-environment-editor">
                  {environmentRows.map((row) => (
                    <div className="mcp-environment-row" key={row.id}>
                      <input
                        className="mcp-environment-key"
                        value={row.key}
                        placeholder={t('tools.mcp.environmentKeyPlaceholder')}
                        aria-label={t('tools.mcp.environmentKeyPlaceholder')}
                        onChange={(event) => updateEnvironmentRow(row.id, { key: event.target.value })}
                      />
                      <span className="mcp-environment-equals" aria-hidden="true">=</span>
                      <input
                        className="mcp-environment-value"
                        type={row.visible ? 'text' : 'password'}
                        value={row.preserved ? (row.visible ? '__preserve__' : '••••••••') : row.value}
                        placeholder={t('tools.mcp.environmentValuePlaceholder')}
                        aria-label={t('tools.mcp.environmentValuePlaceholder')}
                        onFocus={(event) => {
                          if (row.preserved) event.currentTarget.select();
                        }}
                        onChange={(event) => updateEnvironmentRow(row.id, { value: event.target.value, preserved: false })}
                      />
                      <IconButton
                        variant="ghost"
                        type="button"
                        label={row.visible ? t('tools.mcp.hideEnvironmentValue') : t('tools.mcp.showEnvironmentValue')}
                        onClick={() => updateEnvironmentRow(row.id, { visible: !row.visible })}
                      >
                        {row.visible ? <EyeOff size={15} /> : <Eye size={15} />}
                      </IconButton>
                      <IconButton variant="ghost" type="button" label={t('tools.mcp.removeEnvironment')} onClick={() => removeEnvironmentRow(row.id)}>
                        <X size={15} />
                      </IconButton>
                    </div>
                  ))}
                </div>
              </FormField>
              <div className="mcp-long-running-setting">
                <div className="mcp-setting-toggle"><div><strong>{t('tools.mcp.longRunning')}</strong><small>{t('tools.mcp.longRunningHint')}</small></div><span className="toggle-switch"><input id="mcp-long-running" aria-label={t('tools.mcp.longRunning')} type="checkbox" checked={stdioTransport.long_running ?? false} onChange={(event) => updateTransport({ ...stdioTransport, long_running: event.target.checked })} /><span className="toggle-slider" /></span></div>
              </div>
            </section>
          ) : (
            <section className="mcp-form-section">
              <FormField label={t('tools.mcp.url')} htmlFor="mcp-url" required hint={t('tools.mcp.urlHint')}><input id="mcp-url" value={httpTransport?.url ?? ''} placeholder={t('tools.mcp.urlPlaceholder')} onChange={(event) => httpTransport && updateTransport({ ...httpTransport, url: event.target.value })} /></FormField>
              <FormField label={t('tools.mcp.bearerToken')} htmlFor="mcp-bearer-token" hint={t('tools.mcp.secretHint')}>
                <input id="mcp-bearer-token" type="password" value={httpTransport?.bearer_token === '__preserve__' ? '' : httpTransport?.bearer_token ?? ''} placeholder={httpTransport?.bearer_token === '__preserve__' ? t('tools.mcp.secretPreserved') : t('tools.mcp.tokenPlaceholder')} onChange={(event) => httpTransport && updateTransport({ ...httpTransport, bearer_token: event.target.value || null })} />
              </FormField>
              <FormField label={<span className="mcp-label-with-help">{t('tools.mcp.headers')}<span title={t('tools.mcp.headersHelp')}><HelpCircle size={14} /></span></span>} htmlFor="mcp-headers" hint={t('tools.mcp.keyValueHint')}><textarea id="mcp-headers" value={headerText} placeholder={t('tools.mcp.headersPlaceholder')} onChange={(event) => httpTransport && updateTransport({ ...httpTransport, headers: mapEntries(event.target.value) })} rows={6} /></FormField>
            </section>
          )}
          <section className={`mcp-advanced-section${advancedOpen ? ' is-open' : ''}`}>
            <button type="button" className="mcp-advanced-toggle" onClick={() => setAdvancedOpen((value) => !value)} aria-expanded={advancedOpen}>
              <span><strong>{t('tools.mcp.advancedSettings')}</strong><small>{t('tools.mcp.advancedSettingsHint')}</small></span>{advancedOpen ? <ChevronUp size={17} /> : <ChevronDown size={17} />}
            </button>
            {advancedOpen ? <div className="mcp-advanced-content">
              <div className="form-grid two-column">
                {stdioTransport ? <FormField label={t('tools.mcp.cwd')} htmlFor="mcp-cwd"><input id="mcp-cwd" value={stdioTransport.cwd ?? ''} placeholder={t('tools.mcp.cwdPlaceholder')} onChange={(event) => updateTransport({ ...stdioTransport, cwd: event.target.value || null })} /></FormField> : null}
                {renderTimeoutField()}
                <FormField label={t('tools.mcp.provider')} htmlFor="mcp-provider"><input id="mcp-provider" value={draft.provider ?? ''} placeholder={t('tools.mcp.providerPlaceholder')} onChange={(event) => updateDraft({ provider: event.target.value || null })} /></FormField>
                <FormField label={t('tools.mcp.providerUrl')} htmlFor="mcp-provider-url"><input id="mcp-provider-url" type="url" value={draft.provider_url ?? ''} placeholder={t('tools.mcp.providerUrlPlaceholder')} onChange={(event) => updateDraft({ provider_url: event.target.value || null })} /></FormField>
                <FormField label={t('tools.mcp.logoUrl')} htmlFor="mcp-logo-url"><input id="mcp-logo-url" type="url" value={draft.logo_url ?? ''} placeholder={t('tools.mcp.logoUrlPlaceholder')} onChange={(event) => updateDraft({ logo_url: event.target.value || null })} /></FormField>
                <FormField label={t('tools.mcp.tags')} htmlFor="mcp-tags"><input id="mcp-tags" value={(draft.tags ?? []).join(', ')} placeholder={t('tools.mcp.tagsPlaceholder')} onChange={(event) => updateDraft({ tags: parseTags(event.target.value) })} /></FormField>
              </div>
            </div> : null}
          </section>
        </div>
      )}
    </Dialog>
  );
}
