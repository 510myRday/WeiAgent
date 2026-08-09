import { memo } from 'react';

import type { AppSettings, ModelProvider } from '../../../types';
import { useI18n } from '../../../i18n';
import { FormField } from '../../../components/ui';
import { compareNames, sortByName } from '../../../utils/sort';
import { clampCompressThreshold } from './settingsPanelUtils';

interface RuntimeSectionProps {
  settingsForm: AppSettings;
  onPatch: (patch: Partial<AppSettings>) => void;
}

export const RuntimeSection = memo(function RuntimeSection({ settingsForm, onPatch }: RuntimeSectionProps) {
  const { t } = useI18n();
  return (
    <div className="form-card entity-form narrow-form">
      <div className="settings-field-pair">
        <label className="settings-field-pair__label settings-field-pair__label--left" htmlFor="settings-host">
          {t('settings.host')}<span className="required-marker" aria-hidden="true">*</span>
        </label>
        <label className="settings-field-pair__label settings-field-pair__label--right" htmlFor="settings-port">
          {t('settings.port')}<span className="required-marker" aria-hidden="true">*</span>
        </label>
        <p className="settings-field-pair__hint">{t('settings.restartHint')}</p>
        <select
          className="settings-field-pair__control settings-field-pair__control--left"
          id="settings-host"
          value={settingsForm.host}
          onChange={(e) => onPatch({ host: e.target.value })}
          required
        >
          <option value="127.0.0.1">127.0.0.1</option>
          <option value="::1">::1</option>
        </select>
        <input
          className="settings-field-pair__control settings-field-pair__control--right"
          id="settings-port"
          type="number"
          min={1}
          max={65535}
          value={settingsForm.port}
          onChange={(e) => onPatch({ port: Number(e.target.value) })}
          required
        />
        <FormField label={t('settings.logLevel')} htmlFor="settings-log" required>
          <select
            id="settings-log"
            value={settingsForm.log_level}
            onChange={(e) => onPatch({ log_level: e.target.value })}
            required
          >
            {['trace', 'debug', 'info', 'warn', 'error'].map((level) => <option key={level} value={level}>{level}</option>)}
          </select>
        </FormField>
      </div>
    </div>
  );
});

interface AgentSectionProps {
  settingsForm: AppSettings;
  providers: ModelProvider[];
  onPatch: (patch: Partial<AppSettings>) => void;
}

export const AgentSection = memo(function AgentSection({ settingsForm, providers, onPatch }: AgentSectionProps) {
  const { t } = useI18n();
  return (
    <div className="form-card entity-form narrow-form model-agent-settings-card">
      <div className="settings-field-pair">
        <label className="settings-field-pair__label settings-field-pair__label--left" htmlFor="settings-timeout">
          {t('settings.requestTimeout')}<span className="required-marker" aria-hidden="true">*</span>
        </label>
        <label className="settings-field-pair__label settings-field-pair__label--right" htmlFor="settings-retries">
          {t('settings.requestRetries')}<span className="required-marker" aria-hidden="true">*</span>
        </label>
        <p className="settings-field-pair__hint">{t('settings.agentRetryHint')}</p>
        <input
          className="settings-field-pair__control settings-field-pair__control--left"
          id="settings-timeout"
          type="number"
          min={1}
          max={600}
          value={settingsForm.request_timeout_seconds}
          onChange={(e) => onPatch({ request_timeout_seconds: Number(e.target.value) })}
          required
        />
        <input
          className="settings-field-pair__control settings-field-pair__control--right"
          id="settings-retries"
          type="number"
          min={1}
          max={10}
          value={settingsForm.request_retry_attempts}
          onChange={(e) => onPatch({ request_retry_attempts: Number(e.target.value) })}
          required
        />
      </div>
      <div className="setting-row settings-context-policy">
        <div className="setting-info">
          <label htmlFor="settings-context-compress-threshold">
            {t('settings.contextCompressThreshold')}
            <span className="required-marker" aria-hidden="true">*</span>
          </label>
          <p className="setting-desc">{t('settings.contextCompressThresholdHint')}</p>
        </div>
        <label
          className="timeout-input settings-percent-input"
          htmlFor="settings-context-compress-threshold"
        >
          <input
            id="settings-context-compress-threshold"
            type="number"
            min={1}
            max={99}
            step={1}
            value={clampCompressThreshold(
              settingsForm.context_compress_threshold_percent ?? 85,
            )}
            onChange={(e) => onPatch({
              context_compress_threshold_percent: clampCompressThreshold(
                Number(e.target.value),
              ),
            })}
            required
          />
          <span className="unit">{t('settings.contextCompressThresholdUnit')}</span>
        </label>
      </div>
      <div className="settings-field-pair conversation-title-settings-grid">
        <label className="settings-field-pair__label settings-field-pair__label--left" htmlFor="settings-conversation-title-provider">
          {t('settings.conversationTitleProvider')}
        </label>
        <label className="settings-field-pair__label settings-field-pair__label--right" htmlFor="settings-conversation-title-model">
          {t('settings.conversationTitleModel')}
        </label>
        <p className="settings-field-pair__hint">{t('settings.conversationTitleHint')}</p>
        <select
          className="settings-field-pair__control settings-field-pair__control--left"
          id="settings-conversation-title-provider"
          value={settingsForm.conversation_title_provider_id ?? ''}
          onChange={(event) => {
            const providerId = event.target.value || null;
            const provider = providers.find((item) => item.id === providerId);
            onPatch({
              conversation_title_provider_id: providerId,
              conversation_title_model_id: provider?.models.includes(settingsForm.conversation_title_model_id ?? '')
                ? settingsForm.conversation_title_model_id
                : provider?.models[0] ?? null,
            });
          }}
        >
          <option value="">{t('settings.conversationTitleDisabled')}</option>
          {sortByName(providers).map((provider) => (
            <option key={provider.id} value={provider.id}>{provider.name}</option>
          ))}
        </select>
        <select
          className="settings-field-pair__control settings-field-pair__control--right"
          id="settings-conversation-title-model"
          value={settingsForm.conversation_title_model_id ?? ''}
          onChange={(event) => onPatch({
            conversation_title_model_id: event.target.value || null,
          })}
          disabled={!settingsForm.conversation_title_provider_id}
        >
          <option value="">{t('settings.conversationTitleModelPlaceholder')}</option>
          {[...(providers.find((provider) => provider.id === settingsForm.conversation_title_provider_id)?.models ?? [])].sort(compareNames).map((model) => (
            <option key={model} value={model}>{model}</option>
          ))}
        </select>
      </div>
    </div>
  );
});
