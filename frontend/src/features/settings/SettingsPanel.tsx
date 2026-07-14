import { useState, useEffect, useRef, FormEvent } from 'react';
import { Download, FileText, KeyRound, Plus, Save, Server, SlidersHorizontal, Trash2 } from 'lucide-react';

import type { AppSettings, ModelProvider, ModelProviderPayload } from '../../types';
import { fetchModels, fetchModelsForProvider } from '../../api';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { AutoSaveIndicator, Button, FormField, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { LlmLogsModal } from './LlmLogsModal';

interface SettingsPanelProps {
  settingsForm: AppSettings;
  onSettingsFormChange: (form: AppSettings) => void;
  onSettingsSave: (payload: AppSettings) => Promise<AppSettings>;
  providers: ModelProvider[];
  activeProviderId: string;
  initialSection: SettingsSection;
  onProviderChange: (id: string) => void;
  onProviderAdd: (payload: ModelProviderPayload) => Promise<ModelProvider>;
  onProviderEdit: (id: string, payload: Partial<ModelProviderPayload>) => Promise<ModelProvider>;
  onProviderRemove: (id: string) => Promise<void>;
  onOpenMain: () => void;
}

type SettingsSection = 'runtime' | 'providers';
type RuntimeAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
type PendingRuntimeSave = {
  serialized: string;
  payload: AppSettings;
};

const defaultProviderForm = {
  name: '',
  base_url: '',
  api_key: '',
  modelsText: '',
  extraBodyText: '',
};

const VALID_HOSTS = new Set(['127.0.0.1', '::1']);
const VALID_LOG_LEVELS = new Set(['trace', 'debug', 'info', 'warn', 'error']);
const RUNTIME_AUTO_SAVE_DELAY_MS = 600;
const RUNTIME_AUTO_SAVE_RETRY_DELAY_MS = 1200;
const RUNTIME_AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;

function parseModels(value: string): string[] {
  return Array.from(new Set(value.split(/\r?\n|,/).map((model) => model.trim()).filter(Boolean))).sort();
}

function isValidRuntimeSettings(settings: AppSettings): boolean {
  if (!VALID_HOSTS.has(settings.host)) return false;
  if (!Number.isInteger(settings.port) || settings.port < 1 || settings.port > 65535) return false;
  if (!VALID_LOG_LEVELS.has(settings.log_level)) return false;
  if (
    !Number.isInteger(settings.request_timeout_seconds)
    || settings.request_timeout_seconds < 1
    || settings.request_timeout_seconds > 600
  ) {
    return false;
  }
  if (
    !Number.isInteger(settings.request_retry_attempts)
    || settings.request_retry_attempts < 1
    || settings.request_retry_attempts > 10
  ) {
    return false;
  }
  return true;
}

function serializeRuntimeSettings(settings: AppSettings): string {
  return JSON.stringify({
    host: settings.host,
    port: settings.port,
    log_level: settings.log_level,
    request_timeout_seconds: settings.request_timeout_seconds,
    request_retry_attempts: settings.request_retry_attempts,
    llm_log_enabled: settings.llm_log_enabled,
    // Keep avatar/language in the payload for saves, but treat them as owned elsewhere
    // so external Sidebar updates do not thrash auto-save status.
    avatar_icon: settings.avatar_icon,
    avatar_color: settings.avatar_color,
    language: settings.language,
  });
}

export function SettingsPanel({
  settingsForm,
  onSettingsFormChange,
  onSettingsSave,
  providers,
  activeProviderId,
  initialSection,
  onProviderChange,
  onProviderAdd,
  onProviderEdit,
  onProviderRemove,
  onOpenMain,
}: SettingsPanelProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const [activeSection, setActiveSection] = useState<SettingsSection>('runtime');
  const [providerForm, setProviderForm] = useState(defaultProviderForm);
  const [isSubmittingProvider, setIsSubmittingProvider] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [showLlmLogs, setShowLlmLogs] = useState(false);
  const [runtimeDirty, setRuntimeDirty] = useState(false);
  const [autoSaveStatus, setAutoSaveStatus] = useState<RuntimeAutoSaveStatus>('idle');

  const latestSerializedRef = useRef('');
  const persistedSerializedRef = useRef('');
  const pendingSaveRef = useRef<PendingRuntimeSave | null>(null);
  const saveInFlightRef = useRef(false);
  const saveRetryCountRef = useRef(0);
  const saveTimerRef = useRef<number | undefined>(undefined);
  const retryTimerRef = useRef<number | undefined>(undefined);
  const isMountedRef = useRef(true);
  const onSettingsSaveRef = useRef(onSettingsSave);

  onSettingsSaveRef.current = onSettingsSave;

  const activeProvider = providers.find((provider) => provider.id === activeProviderId);

  useEffect(() => () => {
    isMountedRef.current = false;
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
    }
    if (retryTimerRef.current !== undefined) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = undefined;
    }
    void flushRuntimeSaveQueue();
  }, []);

  useEffect(() => {
    setActiveSection(initialSection);
  }, [initialSection]);

  useEffect(() => {
    if (!activeProvider) {
      setProviderForm(defaultProviderForm);
      return;
    }
    setProviderForm({
      name: activeProvider.name,
      base_url: activeProvider.base_url,
      api_key: '',
      modelsText: activeProvider.models.join('\n'),
      extraBodyText: activeProvider.extra_body && Object.keys(activeProvider.extra_body).length > 0
        ? JSON.stringify(activeProvider.extra_body, null, 2)
        : '',
    });
  }, [activeProvider]);

  // Seed persisted snapshot once settings become valid (initial load / external refresh).
  useEffect(() => {
    if (!isValidRuntimeSettings(settingsForm)) return;
    const serialized = serializeRuntimeSettings(settingsForm);
    if (!persistedSerializedRef.current) {
      persistedSerializedRef.current = serialized;
      latestSerializedRef.current = serialized;
      setAutoSaveStatus('idle');
      return;
    }
    // External updates (avatar/language via Sidebar, successful save response) keep snapshot in sync
    // when the user is not actively editing runtime fields.
    if (!runtimeDirty && pendingSaveRef.current === null && !saveInFlightRef.current) {
      persistedSerializedRef.current = serialized;
      latestSerializedRef.current = serialized;
    }
  }, [runtimeDirty, settingsForm]);

  useEffect(() => {
    if (!runtimeDirty) return;

    if (!isValidRuntimeSettings(settingsForm)) {
      setAutoSaveStatus('pending');
      if (saveTimerRef.current !== undefined) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = undefined;
      }
      pendingSaveRef.current = null;
      return;
    }

    const serialized = serializeRuntimeSettings(settingsForm);
    if (latestSerializedRef.current !== serialized) {
      saveRetryCountRef.current = 0;
    }
    latestSerializedRef.current = serialized;

    if (persistedSerializedRef.current === serialized) {
      pendingSaveRef.current = null;
      setRuntimeDirty(false);
      setAutoSaveStatus('saved');
      return;
    }

    pendingSaveRef.current = {
      serialized,
      payload: { ...settingsForm },
    };
    setAutoSaveStatus('pending');
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
    }
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = undefined;
      void flushRuntimeSaveQueue();
    }, RUNTIME_AUTO_SAVE_DELAY_MS);
  }, [runtimeDirty, settingsForm]);

  async function flushRuntimeSaveQueue() {
    if (saveInFlightRef.current || !pendingSaveRef.current) return;
    const pending = pendingSaveRef.current;
    pendingSaveRef.current = null;
    saveInFlightRef.current = true;
    if (isMountedRef.current) {
      setAutoSaveStatus('saving');
    }

    let retryDelay = 0;
    try {
      const updated = await onSettingsSaveRef.current(pending.payload);
      const updatedSerialized = serializeRuntimeSettings(updated);
      persistedSerializedRef.current = updatedSerialized;
      saveRetryCountRef.current = 0;
      if (isMountedRef.current && latestSerializedRef.current === pending.serialized) {
        latestSerializedRef.current = updatedSerialized;
        const queuedSave = pendingSaveRef.current as PendingRuntimeSave | null;
        if (queuedSave?.serialized === pending.serialized) {
          pendingSaveRef.current = null;
        }
        if (saveTimerRef.current !== undefined) {
          window.clearTimeout(saveTimerRef.current);
          saveTimerRef.current = undefined;
        }
        setRuntimeDirty(false);
        setAutoSaveStatus('saved');
      }
    } catch (cause) {
      const attempt = saveRetryCountRef.current + 1;
      saveRetryCountRef.current = attempt;
      if (latestSerializedRef.current === pending.serialized) {
        pendingSaveRef.current = pending;
        retryDelay = Math.min(
          RUNTIME_AUTO_SAVE_MAX_RETRY_DELAY_MS,
          RUNTIME_AUTO_SAVE_RETRY_DELAY_MS * (2 ** Math.min(attempt - 1, 5)),
        );
      }
      if (isMountedRef.current) {
        setAutoSaveStatus('error');
        if (attempt === 1) {
          toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
        }
      }
    } finally {
      saveInFlightRef.current = false;
      if (pendingSaveRef.current) {
        if (retryTimerRef.current !== undefined) {
          window.clearTimeout(retryTimerRef.current);
        }
        retryTimerRef.current = window.setTimeout(() => {
          retryTimerRef.current = undefined;
          void flushRuntimeSaveQueue();
        }, retryDelay);
      }
    }
  }

  function patchRuntimeSettings(patch: Partial<AppSettings>) {
    onSettingsFormChange({ ...settingsForm, ...patch });
    setRuntimeDirty(true);
  }

  async function handleProviderSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    let extraBody: Record<string, unknown> | undefined;
    if (providerForm.extraBodyText.trim()) {
      try {
        extraBody = JSON.parse(providerForm.extraBodyText);
      } catch {
        toast.warning(t('error.invalidJson'));
        return;
      }
    }
    const payload = {
      provider_type: activeProvider?.provider_type ?? 'openai_compatible',
      name: providerForm.name.trim(),
      base_url: providerForm.base_url.trim(),
      api_key: providerForm.api_key.trim(),
      models: parseModels(providerForm.modelsText),
      extra_body: extraBody,
    };
    if (!payload.name || !payload.base_url || (!activeProvider && !payload.api_key) || payload.models.length === 0) {
      toast.warning(t('error.providerValidation'));
      return;
    }
    setIsSubmittingProvider(true);
    try {
      if (activeProvider) {
        await onProviderEdit(activeProvider.id, payload);
        toast.success(t('notification.providerSaved'));
      } else {
        await onProviderAdd(payload);
        toast.success(t('notification.providerCreated'));
      }
      setProviderForm((current) => ({ ...current, api_key: '' }));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setIsSubmittingProvider(false);
    }
  }

  async function handleDeleteProvider() {
    if (!activeProvider) return;
    const confirmed = await confirm({
      title: t('provider.delete'),
      message: t('provider.deleteConfirm', { name: activeProvider.name }),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;
    try {
      await onProviderRemove(activeProvider.id);
      toast.success(t('notification.providerDeleted'));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    }
  }

  async function handleFetchModels() {
    let extraBody: Record<string, unknown> | undefined;
    if (providerForm.extraBodyText.trim()) {
      try {
        extraBody = JSON.parse(providerForm.extraBodyText);
      } catch {
        toast.warning(t('error.invalidJson'));
        return;
      }
    }
    const baseUrl = providerForm.base_url.trim();
    const apiKey = providerForm.api_key.trim();
    if (!baseUrl || (!activeProvider?.has_api_key && !apiKey)) {
      toast.warning(t('provider.fetchModelsMissingConfig'));
      return;
    }
    setFetchingModels(true);
    try {
      const models = activeProvider && !apiKey && baseUrl === activeProvider.base_url && !extraBody
        ? await fetchModels(activeProvider.id)
        : await fetchModelsForProvider({
          provider_type: activeProvider?.provider_type ?? 'openai_compatible',
          base_url: baseUrl,
          api_key: apiKey,
          extra_body: extraBody,
        });
      setProviderForm((current) => ({ ...current, modelsText: models.join('\n') }));
      toast.success(t('notification.modelsLoaded', { count: models.length }));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setFetchingModels(false);
    }
  }

  return (
    <>
    <PanelShell
      className="settings-panel"
      sidebar={(
        <div className="context-pane settings-nav">
          <PaneHeader
            className="mobile-pane-header"
            title={t('nav.settings')}
            leading={<Button className="back-button" type="button" onClick={onOpenMain}>{t('common.back')}</Button>}
          />
          <div className="list-stack">
            <ListItem
              type="button"
              active={activeSection === 'runtime'}
              title={t('settings.title')}
              subtitle={t('settings.description')}
              leading={<Server size={18} />}
              onClick={() => { setActiveSection('runtime'); onOpenMain(); }}
            />
            <ListItem
              type="button"
              active={activeSection === 'providers'}
              title={t('provider.title')}
              subtitle={t('provider.description')}
              leading={<KeyRound size={18} />}
              onClick={() => { setActiveSection('providers'); onOpenMain(); }}
            />
          </div>
        </div>
      )}
      main={(
        <div className="main-pane config-main">
          <PaneHeader
            className="config-page-header"
            eyebrow={t('nav.settings')}
            title={activeSection === 'runtime' ? t('settings.title') : t('provider.title')}
            description={activeSection === 'runtime' ? t('settings.description') : t('provider.description')}
            actions={activeSection === 'runtime' ? (
              <AutoSaveIndicator status={autoSaveStatus} t={t} />
            ) : (
              <Button variant="secondary" type="button" onClick={() => { onProviderChange(''); onOpenMain(); }} icon={<Plus size={17} />}>
                {t('provider.create')}
              </Button>
            )}
          />

          {activeSection === 'runtime' ? (
            <div className="form-card entity-form narrow-form">
              <div className="form-grid two-column">
                <FormField label={t('settings.host')} htmlFor="settings-host" required>
                  <select
                    id="settings-host"
                    value={settingsForm.host}
                    onChange={(e) => patchRuntimeSettings({ host: e.target.value })}
                  >
                    <option value="127.0.0.1">127.0.0.1</option>
                    <option value="::1">::1</option>
                  </select>
                </FormField>
                <FormField label={t('settings.port')} htmlFor="settings-port" required>
                  <input
                    id="settings-port"
                    type="number"
                    min={1}
                    max={65535}
                    value={settingsForm.port}
                    onChange={(e) => patchRuntimeSettings({ port: Number(e.target.value) })}
                  />
                </FormField>
                <FormField label={t('settings.logLevel')} htmlFor="settings-log" required>
                  <select
                    id="settings-log"
                    value={settingsForm.log_level}
                    onChange={(e) => patchRuntimeSettings({ log_level: e.target.value })}
                  >
                    {['trace', 'debug', 'info', 'warn', 'error'].map((level) => <option key={level} value={level}>{level}</option>)}
                  </select>
                </FormField>
                <FormField label={t('settings.requestTimeout')} htmlFor="settings-timeout" required>
                  <input
                    id="settings-timeout"
                    type="number"
                    min={1}
                    max={600}
                    value={settingsForm.request_timeout_seconds}
                    onChange={(e) => patchRuntimeSettings({ request_timeout_seconds: Number(e.target.value) })}
                  />
                </FormField>
                <FormField label={t('settings.requestRetries')} htmlFor="settings-retries" required>
                  <input
                    id="settings-retries"
                    type="number"
                    min={1}
                    max={10}
                    value={settingsForm.request_retry_attempts}
                    onChange={(e) => patchRuntimeSettings({ request_retry_attempts: Number(e.target.value) })}
                  />
                </FormField>
              </div>
              <div className="llm-log-toggle-row">
                <label className="toggle-label">
                  <input
                    id="settings-llm-log"
                    type="checkbox"
                    checked={settingsForm.llm_log_enabled}
                    onChange={(e) => patchRuntimeSettings({ llm_log_enabled: e.target.checked })}
                  />
                  <span className="toggle-text">{t('settings.llmLogEnabled')}</span>
                </label>
                <Button variant="secondary" type="button" onClick={() => setShowLlmLogs(true)} icon={<FileText size={15} />}>
                  {t('llmLogs.title')}
                </Button>
              </div>
              <p className="hint-text small-hint">{t('settings.llmLogHint')}</p>
              <p className="hint-text">{t('settings.restartHint')}</p>
            </div>
          ) : (
            <div className="provider-layout">
              <aside className="provider-list-panel">
                <PaneHeader compact title={t('provider.title')} />
                <div className="list-stack flush">
                  {providers.length === 0 ? <p className="muted">{t('provider.empty')}</p> : null}
                  {providers.map((provider) => (
                    <ListItem
                      key={provider.id}
                      className="provider-list-item"
                      type="button"
                      active={provider.id === activeProviderId}
                      title={provider.name}
                      subtitle={`${provider.models.length} ${t('provider.modelsCount')}`}
                      leading={<SlidersHorizontal size={17} />}
                      onClick={() => { onProviderChange(provider.id); onOpenMain(); }}
                    />
                  ))}
                </div>
              </aside>
              <form className="form-card entity-form" onSubmit={handleProviderSubmit}>
                <PaneHeader
                  title={activeProvider ? activeProvider.name : t('provider.newTitle')}
                  description={activeProvider ? activeProvider.base_url : t('provider.selectHint')}
                  actions={activeProvider ? (
                    <IconButton variant="danger" type="button" onClick={handleDeleteProvider} label={t('provider.delete')}>
                      <Trash2 size={17} />
                    </IconButton>
                  ) : null}
                  compact
                />
                <div className="form-grid two-column">
                  <FormField label={t('provider.name')} htmlFor="provider-name" required>
                    <input id="provider-name" value={providerForm.name} onChange={(e) => setProviderForm({ ...providerForm, name: e.target.value })} disabled={isSubmittingProvider} />
                  </FormField>
                  <FormField label={t('provider.baseUrl')} htmlFor="provider-base-url" required>
                    <input id="provider-base-url" value={providerForm.base_url} onChange={(e) => setProviderForm({ ...providerForm, base_url: e.target.value })} disabled={isSubmittingProvider} />
                  </FormField>
                  <FormField label={t('provider.apiKey')} htmlFor="provider-api-key" required={!activeProvider}>
                    <input
                      id="provider-api-key"
                      type="password"
                      value={providerForm.api_key}
                      onChange={(e) => setProviderForm({ ...providerForm, api_key: e.target.value })}
                      placeholder={activeProvider?.has_api_key ? activeProvider.masked_api_key : ''}
                      disabled={isSubmittingProvider}
                    />
                  </FormField>
                </div>
                <FormField label={t('provider.extraBody')} htmlFor="provider-extra-body">
                  <textarea
                    id="provider-extra-body"
                    value={providerForm.extraBodyText}
                    onChange={(e) => setProviderForm({ ...providerForm, extraBodyText: e.target.value })}
                    rows={4}
                    placeholder='{"enable_thinking": true, "reasoning_effort": "high"}'
                    disabled={isSubmittingProvider}
                  />
                </FormField>
                <FormField
                  label={t('provider.models')}
                  htmlFor="provider-models"
                  required
                  action={(
                    <Button
                      className="inline-action"
                      variant="ghost"
                      type="button"
                      disabled={fetchingModels}
                      onClick={handleFetchModels}
                      icon={<Download size={14} />}
                    >
                      {fetchingModels ? t('provider.fetchingModels') : t('provider.fetchModels')}
                    </Button>
                  )}
                >
                  <textarea id="provider-models" value={providerForm.modelsText} onChange={(e) => setProviderForm({ ...providerForm, modelsText: e.target.value })} rows={10} disabled={isSubmittingProvider || fetchingModels} />
                </FormField>
                <div className="form-actions">
                  <Button variant="primary" type="submit" disabled={isSubmittingProvider} icon={<Save size={17} />}>
                    {activeProvider ? t('provider.save') : t('provider.create')}
                  </Button>
                </div>
              </form>
            </div>
          )}
        </div>
      )}
    />
    {showLlmLogs ? <LlmLogsModal onClose={() => setShowLlmLogs(false)} /> : null}
  </>
  );
}

