import type { FormEvent, SyntheticEvent } from 'react';
import { ChevronDown, Code2, Download, Eye, EyeOff, LockKeyhole, Save } from 'lucide-react';

import type { ModelConfig, ModelProvider, ProviderType } from '../../../types';
import { useI18n } from '../../../i18n';
import { Button, FormField, IconButton, PaneHeader } from '../../../components/ui';
import { PROTOCOL_PRESETS } from './providerTemplates';
import {
  parseContextSize,
  parseExtraBodyText,
  parseModels,
  parseOptionalPrice,
} from './settingsPanelUtils';
import type { ProviderForm } from './settingsPanelUtils';

interface ProviderEditorFormProps {
  providerForm: ProviderForm;
  activeProvider: ModelProvider | undefined;
  isSubmittingProvider: boolean;
  fetchingModels: boolean;
  showApiKey: boolean;
  advancedOpen: boolean;
  providerConnectionLocked: boolean;
  onSubmit: (e: FormEvent<HTMLFormElement>) => void;
  onToggleShowApiKey: () => void;
  onAdvancedOpenChange: (open: boolean) => void;
  onNameChange: (name: string) => void;
  onProtocolChange: (providerType: ProviderType) => void;
  onBaseUrlChange: (baseUrl: string) => void;
  onApiKeyChange: (apiKey: string) => void;
  onModelsTextChange: (modelsText: string) => void;
  onPatchModelConfig: (modelId: string, patch: Partial<ModelConfig>) => void;
  onExtraBodyChange: (extraBodyText: string) => void;
  onFetchModels: () => void;
}

export function ProviderEditorForm({
  providerForm,
  activeProvider,
  isSubmittingProvider,
  fetchingModels,
  showApiKey,
  advancedOpen,
  providerConnectionLocked,
  onSubmit,
  onToggleShowApiKey,
  onAdvancedOpenChange,
  onNameChange,
  onProtocolChange,
  onBaseUrlChange,
  onApiKeyChange,
  onModelsTextChange,
  onPatchModelConfig,
  onExtraBodyChange,
  onFetchModels,
}: ProviderEditorFormProps) {
  const { t } = useI18n();
  const modelCount = parseModels(providerForm.modelsText).length;
  const extraBodyStatus = parseExtraBodyText(providerForm.extraBodyText).status;

  return (
    <form className="form-card entity-form provider-editor" onSubmit={onSubmit}>
      <p className="provider-editor-kicker">
        {activeProvider ? t('provider.editing') : t('provider.creating')}
      </p>

      <section className="provider-form-section">
        <PaneHeader
          compact
          title={t('provider.connection')}
          actions={(
            <div className="provider-connection-statuses">
              {providerConnectionLocked ? (
                <span className="mini-status-badge provider-template-locked">
                  <LockKeyhole size={13} aria-hidden="true" />
                  {t('provider.template.locked')}
                </span>
              ) : null}
              <span className={`mini-status-badge${
                activeProvider?.has_api_key || providerForm.api_key ? '' : ' is-required'
              }`}>
                {activeProvider?.has_api_key || providerForm.api_key
                  ? t('provider.keyConfigured')
                  : t('provider.keyRequired')}
              </span>
            </div>
          )}
        />
        <div className="form-grid two-column provider-connection-grid">
          <FormField label={t('provider.name')} htmlFor="provider-name" required>
            <input id="provider-name" value={providerForm.name} onChange={(e) => onNameChange(e.target.value)} disabled={isSubmittingProvider} required />
          </FormField>
          <FormField label={t('provider.apiMode')} htmlFor="provider-api-mode" required>
            <select
              className={providerConnectionLocked ? 'provider-template-fixed-control' : undefined}
              id="provider-api-mode"
              value={providerForm.provider_type}
              onChange={(event) => onProtocolChange(event.target.value as ProviderType)}
              disabled={isSubmittingProvider || providerConnectionLocked}
              required
            >
              {PROTOCOL_PRESETS.map((preset) => (
                <option key={preset.type} value={preset.type}>
                  {t(preset.labelKey)}
                </option>
              ))}
            </select>
          </FormField>
          <FormField label={t('provider.baseUrl')} htmlFor="provider-base-url" required>
            <input className={providerConnectionLocked ? 'provider-template-fixed-control' : undefined} id="provider-base-url" value={providerForm.base_url} onChange={(e) => onBaseUrlChange(e.target.value)} disabled={isSubmittingProvider || providerConnectionLocked} required />
          </FormField>
          <FormField label={t('provider.apiKey')} htmlFor="provider-api-key" required={!activeProvider}>
            <div className="provider-secret-input">
              <input
                id="provider-api-key"
                type={showApiKey ? 'text' : 'password'}
                value={providerForm.api_key}
                onChange={(e) => onApiKeyChange(e.target.value)}
                placeholder={activeProvider?.has_api_key ? activeProvider.masked_api_key : ''}
                disabled={isSubmittingProvider}
                required={!activeProvider}
              />
              <IconButton
                className="provider-secret-toggle"
                variant="ghost"
                type="button"
                onClick={onToggleShowApiKey}
                label={showApiKey ? t('provider.hideKey') : t('provider.showKey')}
              >
                {showApiKey ? <EyeOff size={16} /> : <Eye size={16} />}
              </IconButton>
            </div>
          </FormField>
        </div>
      </section>

      <section className="provider-form-section">
        <div className="provider-model-section-heading">
          <div>
            <h3>{t('provider.models')}</h3>
            <p>{t('provider.modelsDescription')}</p>
          </div>
          <div className="provider-field-actions">
            <span className={`mini-status-badge${modelCount === 0 ? ' is-required' : ''}`}>
              {modelCount === 0
                ? t('provider.modelsRequired')
                : t('provider.modelCount', { count: modelCount })}
            </span>
            <Button
              className="inline-action"
              variant="ghost"
              type="button"
              disabled={fetchingModels}
              onClick={onFetchModels}
              icon={<Download size={14} />}
            >
              {fetchingModels ? t('provider.fetchingModels') : t('provider.fetchModels')}
            </Button>
          </div>
        </div>

        <div className="provider-model-workspace">
          <FormField
            label={t('provider.modelIds')}
            htmlFor="provider-models"
            required
          >
            <textarea
              id="provider-models"
              value={providerForm.modelsText}
              onChange={(event) => onModelsTextChange(event.target.value)}
              rows={Math.max(8, Math.min(14, modelCount + 3))}
              required
              aria-required="true"
              placeholder={t('provider.modelsPlaceholder')}
              disabled={isSubmittingProvider || fetchingModels}
            />
          </FormField>

          <div className="provider-model-config">
            <div className="provider-model-config__label">
              {t('provider.modelConfiguration')}
              <span>（{t('provider.priceUnit')}）</span>
            </div>
            <div className="model-config-list">
              {providerForm.modelConfigs.length === 0 ? (
                <div className="model-config-empty">{t('provider.modelConfigurationEmpty')}</div>
              ) : (
                <>
                  <div className="model-config-header" aria-hidden="true">
                    <span>{t('provider.model')}</span>
                    <span>{t('provider.capabilities')}</span>
                    <span>{t('provider.contextSize')}</span>
                    <span>{t('provider.inputPrice')}</span>
                    <span>{t('provider.outputPrice')}</span>
                    <span>{t('provider.cacheWritePrice')}</span>
                    <span>{t('provider.cacheReadPrice')}</span>
                  </div>
                  {providerForm.modelConfigs.map((config, index) => (
                    <div className="model-config-row" key={config.model_id}>
                      <strong className="model-config-name" title={config.model_id}>
                        {config.model_id}
                      </strong>
                      <div className="model-capabilities">
                        {([
                          ['multimodal', 'provider.multimodal'],
                          ['thinking', 'provider.thinking'],
                        ] as const).map(([field, labelKey]) => (
                          <label className="model-capability-toggle" key={field}>
                            <span>{t(labelKey)}</span>
                            <input
                              type="checkbox"
                              checked={config[field]}
                              onChange={(event) => onPatchModelConfig(config.model_id, {
                                [field]: event.target.checked,
                              })}
                              disabled={isSubmittingProvider}
                              aria-label={`${config.model_id} ${t(labelKey)}`}
                            />
                            <span className="model-capability-toggle__track" aria-hidden="true" />
                          </label>
                        ))}
                      </div>
                      <label className="model-context-field">
                        <span className="model-price-field__mobile-label">{t('provider.contextSize')}</span>
                        <span className="model-context-field__input">
                          <input
                            id={`provider-model-${index}-context_size`}
                            type="number"
                            min="1"
                            step="1"
                            inputMode="numeric"
                            value={config.context_size}
                            placeholder="128000"
                            onChange={(event) => onPatchModelConfig(config.model_id, {
                              context_size: parseContextSize(event.target.value),
                            })}
                            disabled={isSubmittingProvider}
                            aria-label={`${config.model_id} ${t('provider.contextSize')}`}
                          />
                        </span>
                      </label>
                      {([
                        ['input_price', 'provider.inputPrice'],
                        ['output_price', 'provider.outputPrice'],
                        ['cache_write_price', 'provider.cacheWritePrice'],
                        ['cache_read_price', 'provider.cacheReadPrice'],
                      ] as const).map(([field, labelKey]) => (
                        <label className="model-price-field" key={field}>
                          <span className="model-price-field__mobile-label">{t(labelKey)}</span>
                          <span className="model-price-field__input">
                            <span aria-hidden="true">$</span>
                            <input
                              id={`provider-model-${index}-${field}`}
                              type="number"
                              min="0"
                              step="0.000001"
                              inputMode="decimal"
                              value={config[field] ?? ''}
                              placeholder="0.00"
                              onChange={(event) => onPatchModelConfig(config.model_id, {
                                [field]: parseOptionalPrice(event.target.value),
                              })}
                              disabled={isSubmittingProvider}
                              aria-label={`${config.model_id} ${t(labelKey)}`}
                            />
                          </span>
                        </label>
                      ))}
                    </div>
                  ))}
                </>
              )}
            </div>
          </div>
        </div>
      </section>

      <details
        className="provider-advanced"
        open={advancedOpen}
        onToggle={(event: SyntheticEvent<HTMLDetailsElement>) => {
          onAdvancedOpenChange(event.currentTarget.open);
        }}
      >
        <summary>
          <span className="provider-advanced__icon" aria-hidden="true">
            <Code2 size={17} />
          </span>
          <span className="provider-advanced__content">
            <strong>{t('provider.advanced')}</strong>
            <small>{t('provider.extraBody')}</small>
          </span>
          {extraBodyStatus === 'valid' ? (
            <span className="mini-status-badge">{t('provider.advancedConfigured')}</span>
          ) : extraBodyStatus === 'invalid' ? (
            <span className="mini-status-badge is-required">{t('provider.advancedInvalid')}</span>
          ) : null}
          <ChevronDown className="provider-advanced__chevron" size={17} aria-hidden="true" />
        </summary>
        <FormField label={t('provider.extraBody')} htmlFor="provider-extra-body">
          <textarea
            id="provider-extra-body"
            value={providerForm.extraBodyText}
            onChange={(e) => onExtraBodyChange(e.target.value)}
            rows={5}
            placeholder='{"temperature": 0.7}'
            disabled={isSubmittingProvider}
            aria-invalid={extraBodyStatus === 'invalid'}
          />
          {extraBodyStatus === 'invalid' ? (
            <p className="hint-text provider-extra-body-error" role="alert">
              {t('error.invalidJson')}
            </p>
          ) : null}
        </FormField>
      </details>

      {!activeProvider ? (
        <div className="form-actions provider-form-actions">
          <Button variant="primary" type="submit" disabled={isSubmittingProvider} icon={<Save size={17} />}>
            {t('provider.create')}
          </Button>
        </div>
      ) : null}
    </form>
  );
}
