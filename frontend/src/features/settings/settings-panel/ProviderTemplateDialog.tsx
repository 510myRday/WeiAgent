import { useState } from 'react';
import { ChevronRight, LayoutTemplate, SearchX, SlidersHorizontal } from 'lucide-react';

import { useI18n } from '../../../i18n';
import { Button, Dialog, SearchInput } from '../../../components/ui';
import { PROVIDER_TEMPLATES, protocolPresetByType, providerTemplateById } from './providerTemplates';
import type { ProviderTemplate, ProviderTemplateId } from './providerTemplates';

interface ProviderTemplateDialogProps {
  open: boolean;
  onClose: () => void;
  onSelect: (templateId: ProviderTemplateId) => void;
}

export function ProviderTemplateDialog({ open, onClose, onSelect }: ProviderTemplateDialogProps) {
  const { t } = useI18n();
  const [search, setSearch] = useState('');

  const normalizedSearch = search.trim().toLocaleLowerCase();
  const filteredProviderTemplates = PROVIDER_TEMPLATES.filter((template) => {
    if (!template.fixed) return false;
    if (!normalizedSearch) return true;
    const protocol = template.providerType ? protocolPresetByType[template.providerType] : null;
    return [
      t(template.labelKey),
      t(template.descriptionKey),
      template.baseUrl,
      protocol ? t(protocol.labelKey) : '',
      protocol ? t(protocol.shortKey) : '',
    ].some((value) => value?.toLocaleLowerCase().includes(normalizedSearch));
  });

  function handleSelect(templateId: ProviderTemplateId) {
    setSearch('');
    onSelect(templateId);
  }

  function handleClose() {
    setSearch('');
    onClose();
  }

  function renderProviderTemplateOption(template: ProviderTemplate) {
    const protocol = template.providerType
      ? protocolPresetByType[template.providerType]
      : null;
    return (
      <button
        className={`provider-template-option provider-template-option--${template.id}`}
        key={template.id}
        type="button"
        onClick={() => handleSelect(template.id)}
      >
        <span className="provider-template-option__mark" aria-hidden="true">
          {template.iconPath ? (
            <img src={template.iconPath} alt="" width="26" height="26" />
          ) : (
            <SlidersHorizontal size={21} />
          )}
        </span>
        <span className="provider-template-option__copy">
          <strong>{t(template.labelKey)}</strong>
          <small>{t(template.descriptionKey)}</small>
          <span className="provider-template-option__meta">
            <span>{protocol ? t(protocol.shortKey) : t('provider.template.customMode')}</span>
            {template.baseUrl ? <code>{template.baseUrl}</code> : null}
          </span>
        </span>
        <ChevronRight className="provider-template-option__chevron" size={18} aria-hidden="true" />
      </button>
    );
  }

  return (
    <Dialog
      open={open}
      title={t('provider.template.title')}
      description={t('provider.template.description')}
      icon={<LayoutTemplate size={21} />}
      closeLabel={t('common.close')}
      onClose={handleClose}
      size="lg"
      className="provider-template-dialog"
      bodyClassName="provider-template-dialog__body"
    >
      <div className="provider-template-featured">
        {renderProviderTemplateOption(providerTemplateById.custom)}
      </div>
      <div className="provider-template-search">
        <SearchInput
          value={search}
          onChange={setSearch}
          placeholder={t('provider.template.searchPlaceholder')}
        />
      </div>
      {filteredProviderTemplates.length > 0 ? (
        <div className="provider-template-grid">
          {filteredProviderTemplates.map(renderProviderTemplateOption)}
        </div>
      ) : (
        <div className="provider-template-empty" role="status">
          <SearchX size={24} aria-hidden="true" />
          <div>
            <strong>{t('provider.template.searchEmpty')}</strong>
            <span>{t('provider.template.searchEmptyDescription')}</span>
          </div>
          <Button variant="ghost" type="button" onClick={() => setSearch('')}>
            {t('provider.template.clearSearch')}
          </Button>
        </div>
      )}
    </Dialog>
  );
}
