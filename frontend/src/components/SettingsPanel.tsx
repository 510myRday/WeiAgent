import { useState, useEffect, FormEvent } from 'react';
import { Download, KeyRound, Plus, Save, Server, SlidersHorizontal, Trash2 } from 'lucide-react';
import type { AppSettings, ModelProvider } from '../types';
import { fetchModels } from '../api';
import { useI18n } from '../i18n';

interface SettingsPanelProps {
  settingsForm: AppSettings;
  onSettingsFormChange: (form: AppSettings) => void;
  onSettingsSave: (payload: AppSettings) => Promise<AppSettings>;
  providers: ModelProvider[];
  activeProviderId: string;
  onProviderChange: (id: string) => void;
  onProviderAdd: (payload: { name: string; base_url: string; api_key: string; models: string[] }) => Promise<ModelProvider>;
  onProviderEdit: (id: string, payload: Partial<{ name: string; base_url: string; api_key: string; models: string[] }>) => Promise<ModelProvider>;
  onProviderRemove: (id: string) => Promise<void>;
}

type SettingsSection = 'runtime' | 'providers';

const defaultProviderForm = {
  name: '',
  base_url: 'https://api.openai.com/v1',
  api_key: '',
  modelsText: '',
};

function parseModels(value: string): string[] {
  return Array.from(new Set(value.split(/\r?\n|,/).map((m) => m.trim()).filter(Boolean))).sort();
}

export function SettingsPanel({
  settingsForm,
  onSettingsFormChange,
  onSettingsSave,
  providers,
  activeProviderId,
  onProviderChange,
  onProviderAdd,
  onProviderEdit,
  onProviderRemove,
}: SettingsPanelProps) {
  const { t } = useI18n();
  const [activeSection, setActiveSection] = useState<SettingsSection>('runtime');
  const [providerForm, setProviderForm] = useState(defaultProviderForm);
  const [isSubmittingProvider, setIsSubmittingProvider] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [error, setError] = useState('');

  const activeProvider = providers.find((p) => p.id === activeProviderId);

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
    const payload = {
      name: providerForm.name.trim(),
      base_url: providerForm.base_url.trim(),
      api_key: providerForm.api_key.trim(),
      models: parseModels(providerForm.modelsText),
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
    if (!activeProvider) return;
    setFetchingModels(true);
    setError('');
    try {
      const models = await fetchModels(activeProvider.id);
      setProviderForm((current) => ({ ...current, modelsText: models.join('\n') }));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    } finally {
      setFetchingModels(false);
    }
  }

  return (
    <section className="settings-panel config-panel">
      <div className="config-sidebar settings-nav">
        <div className="panel-header">
          <h2>{t('nav.settings')}</h2>
        </div>
        <div className="settings-section-list">
          <button
            className={activeSection === 'runtime' ? 'settings-section-item active' : 'settings-section-item'}
            type="button"
            onClick={() => setActiveSection('runtime')}
          >
            <Server size={18} />
            <span>
              <strong>{t('settings.title')}</strong>
              <small>{t('settings.description')}</small>
            </span>
          </button>
          <button
            className={activeSection === 'providers' ? 'settings-section-item active' : 'settings-section-item'}
            type="button"
            onClick={() => setActiveSection('providers')}
          >
            <KeyRound size={18} />
            <span>
              <strong>{t('provider.title')}</strong>
              <small>{t('provider.description')}</small>
            </span>
          </button>
        </div>
      </div>

      <div className="config-main settings-main">
        <div className="config-page-header">
          <div>
            <p className="eyebrow">{t('nav.settings')}</p>
            <h2>{activeSection === 'runtime' ? t('settings.title') : t('provider.title')}</h2>
            <p>{activeSection === 'runtime' ? t('settings.description') : t('provider.description')}</p>
          </div>
          {activeSection === 'providers' ? (
            <button className="secondary-button light" type="button" onClick={() => onProviderChange('')}>
              <Plus size={17} />
              {t('provider.create')}
            </button>
          ) : null}
        </div>

        {error ? (
          <div className="error-banner">
            <span>{error}</span>
            <button className="error-dismiss" type="button" onClick={() => setError('')} aria-label={t('error.dismiss')}>&times;</button>
          </div>
        ) : null}

        {activeSection === 'runtime' ? (
          <form className="agent-form form-card narrow-form" onSubmit={handleSettingsSubmit}>
            <div className="form-grid two-column">
              <div className="field">
                <label htmlFor="settings-host">{t('settings.host')}</label>
                <input id="settings-host" value={settingsForm.host} onChange={(e) => onSettingsFormChange({ ...settingsForm, host: e.target.value })} />
              </div>
              <div className="field">
                <label htmlFor="settings-port">{t('settings.port')}</label>
                <input id="settings-port" type="number" min={1} max={65535} value={settingsForm.port} onChange={(e) => onSettingsFormChange({ ...settingsForm, port: Number(e.target.value) })} />
              </div>
              <div className="field">
                <label htmlFor="settings-log">{t('settings.logLevel')}</label>
                <select id="settings-log" value={settingsForm.log_level} onChange={(e) => onSettingsFormChange({ ...settingsForm, log_level: e.target.value })}>
                  {['trace', 'debug', 'info', 'warn', 'error'].map((level) => <option key={level} value={level}>{level}</option>)}
                </select>
              </div>
            </div>
            <p className="hint-text">{t('settings.restartHint')}</p>
            <div className="agent-form-actions form-actions-sticky">
              <button className="primary-button" type="submit"><Save size={17} />{t('settings.save')}</button>
            </div>
          </form>
        ) : (
          <div className="provider-layout">
            <aside className="provider-list-panel">
              <div className="panel-header compact">
                <h3>{t('provider.title')}</h3>
              </div>
              <div className="agent-list">
                {providers.length === 0 ? <p className="muted">{t('provider.empty')}</p> : null}
                {providers.map((provider) => (
                  <button
                    className={provider.id === activeProviderId ? 'agent-item active' : 'agent-item'}
                    key={provider.id}
                    type="button"
                    onClick={() => onProviderChange(provider.id)}
                  >
                    <SlidersHorizontal size={17} />
                    <span>
                      <strong>{provider.name}</strong>
                      <small>{provider.models.length} {t('provider.modelsCount')}</small>
                    </span>
                  </button>
                ))}
              </div>
            </aside>
            <form className="agent-form form-card" onSubmit={handleProviderSubmit}>
              <div className="panel-header">
                <div>
                  <h2>{activeProvider ? activeProvider.name : t('provider.newTitle')}</h2>
                  <p className="muted">{activeProvider ? activeProvider.base_url : t('provider.selectHint')}</p>
                </div>
                {activeProvider ? (
                  <button className="icon-button danger light" type="button" onClick={handleDeleteProvider} title={t('provider.delete')} aria-label={t('provider.delete')}>
                    <Trash2 size={17} />
                  </button>
                ) : null}
              </div>
              <div className="form-grid two-column">
                <div className="field">
                  <label htmlFor="provider-name">{t('provider.name')}</label>
                  <input id="provider-name" value={providerForm.name} onChange={(e) => setProviderForm({ ...providerForm, name: e.target.value })} disabled={isSubmittingProvider} />
                </div>
                <div className="field">
                  <label htmlFor="provider-base-url">{t('provider.baseUrl')}</label>
                  <input id="provider-base-url" value={providerForm.base_url} onChange={(e) => setProviderForm({ ...providerForm, base_url: e.target.value })} disabled={isSubmittingProvider} />
                </div>
                <div className="field">
                  <label htmlFor="provider-api-key">{t('provider.apiKey')}</label>
                  <input
                    id="provider-api-key"
                    type="password"
                    value={providerForm.api_key}
                    onChange={(e) => setProviderForm({ ...providerForm, api_key: e.target.value })}
                    placeholder={activeProvider?.has_api_key ? activeProvider.masked_api_key : ''}
                    disabled={isSubmittingProvider}
                  />
                </div>
              </div>
              <div className="field">
                <label htmlFor="provider-models" className="label-row">
                  <span>{t('provider.models')}</span>
                  {activeProvider ? (
                    <button className="inline-button" type="button" disabled={fetchingModels} onClick={handleFetchModels}>
                      <Download size={14} />
                      {fetchingModels ? t('provider.fetchingModels') : t('provider.fetchModels')}
                    </button>
                  ) : null}
                </label>
                <textarea id="provider-models" value={providerForm.modelsText} onChange={(e) => setProviderForm({ ...providerForm, modelsText: e.target.value })} rows={10} disabled={isSubmittingProvider || fetchingModels} />
              </div>
              <div className="agent-form-actions form-actions-sticky">
                <button className="primary-button" type="submit" disabled={isSubmittingProvider}>
                  <Save size={17} />
                  {activeProvider ? t('provider.save') : t('provider.create')}
                </button>
              </div>
            </form>
          </div>
        )}
      </div>
    </section>
  );
}

