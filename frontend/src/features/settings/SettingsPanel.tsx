import { useState, useEffect, FormEvent } from 'react';
import { Download, KeyRound, Plus, Save, Server, SlidersHorizontal, Trash2 } from 'lucide-react';

import type { AppSettings, ModelProvider, ModelProviderPayload } from '../../types';
import { fetchModels, fetchModelsForProvider } from '../../api';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { AlertBanner, Button, FormField, IconButton, ListItem, PaneHeader } from '../../components/ui';

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

const defaultProviderForm = {
  name: '',
  base_url: '',
  api_key: '',
  modelsText: '',
  extraBodyText: '',
};

function parseModels(value: string): string[] {
  return Array.from(new Set(value.split(/\r?\n|,/).map((model) => model.trim()).filter(Boolean))).sort();
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
  const [activeSection, setActiveSection] = useState<SettingsSection>('runtime');
  const [providerForm, setProviderForm] = useState(defaultProviderForm);
  const [isSubmittingProvider, setIsSubmittingProvider] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [error, setError] = useState('');

  const activeProvider = providers.find((provider) => provider.id === activeProviderId);

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

  async function handleSettingsSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    try {
      await onSettingsSave(settingsForm);
      setError('');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    }
  }

  async function handleProviderSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    let extraBody: Record<string, unknown> | undefined;
    if (providerForm.extraBodyText.trim()) {
      try {
        extraBody = JSON.parse(providerForm.extraBodyText);
      } catch {
        setError('Extra body is not valid JSON');
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
      setError(t('error.providerValidation'));
      return;
    }
    setIsSubmittingProvider(true);
    setError('');
    try {
      if (activeProvider) {
        await onProviderEdit(activeProvider.id, payload);
      } else {
        await onProviderAdd(payload);
      }
      setProviderForm((current) => ({ ...current, api_key: '' }));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    } finally {
      setIsSubmittingProvider(false);
    }
  }

  async function handleDeleteProvider() {
    if (!activeProvider) return;
    if (!window.confirm(t('provider.deleteConfirm').replace('{name}', activeProvider.name))) return;
    try {
      await onProviderRemove(activeProvider.id);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    }
  }

  async function handleFetchModels() {
    let extraBody: Record<string, unknown> | undefined;
    if (providerForm.extraBodyText.trim()) {
      try {
        extraBody = JSON.parse(providerForm.extraBodyText);
      } catch {
        setError('Extra body is not valid JSON');
        return;
      }
    }
    const baseUrl = providerForm.base_url.trim();
    const apiKey = providerForm.api_key.trim();
    if (!baseUrl || (!activeProvider?.has_api_key && !apiKey)) {
      setError(t('provider.fetchModelsMissingConfig'));
      return;
    }
    setFetchingModels(true);
    setError('');
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
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    } finally {
      setFetchingModels(false);
    }
  }

  return (
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
            actions={activeSection === 'providers' ? (
              <Button variant="secondary" type="button" onClick={() => { onProviderChange(''); onOpenMain(); }} icon={<Plus size={17} />}>
                {t('provider.create')}
              </Button>
            ) : null}
          />

          {error ? <AlertBanner message={error} onDismiss={() => setError('')} dismissLabel={t('error.dismiss')} /> : null}

          {activeSection === 'runtime' ? (
            <form className="form-card entity-form narrow-form" onSubmit={handleSettingsSubmit}>
              <div className="form-grid two-column">
                <FormField label={t('settings.host')} htmlFor="settings-host" required>
                  <input id="settings-host" value={settingsForm.host} onChange={(e) => onSettingsFormChange({ ...settingsForm, host: e.target.value })} />
                </FormField>
                <FormField label={t('settings.port')} htmlFor="settings-port" required>
                  <input id="settings-port" type="number" min={1} max={65535} value={settingsForm.port} onChange={(e) => onSettingsFormChange({ ...settingsForm, port: Number(e.target.value) })} />
                </FormField>
                <FormField label={t('settings.logLevel')} htmlFor="settings-log" required>
                  <select id="settings-log" value={settingsForm.log_level} onChange={(e) => onSettingsFormChange({ ...settingsForm, log_level: e.target.value })}>
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
                    onChange={(e) => onSettingsFormChange({ ...settingsForm, request_timeout_seconds: Number(e.target.value) })}
                  />
                </FormField>
                <FormField label={t('settings.requestRetries')} htmlFor="settings-retries" required>
                  <input
                    id="settings-retries"
                    type="number"
                    min={1}
                    max={10}
                    value={settingsForm.request_retry_attempts}
                    onChange={(e) => onSettingsFormChange({ ...settingsForm, request_retry_attempts: Number(e.target.value) })}
                  />
                </FormField>
              </div>
              <p className="hint-text">{t('settings.restartHint')}</p>
              <div className="form-actions">
                <Button variant="primary" type="submit" icon={<Save size={17} />}>{t('settings.save')}</Button>
              </div>
            </form>
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
  );
}
