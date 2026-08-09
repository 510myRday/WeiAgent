import { useEffect, useState, type FormEvent } from 'react';

import { useI18n } from '../i18n';
import { Button, Dialog } from './ui';

export const IMAGE_SIZE_OPTIONS = [
  'auto',
  '1024x1024',
  '1536x1024',
  '1024x1536',
  '1792x1024',
  '1024x1792',
] as const;

export const CUSTOM_IMAGE_SIZE = '__custom__';

export function isValidCustomImageSize(value: string): boolean {
  const match = value.trim().match(/^(\d{1,5})x(\d{1,5})$/i);
  if (!match) return false;
  const width = Number(match[1]);
  const height = Number(match[2]);
  return width >= 1 && width <= 16384 && height >= 1 && height <= 16384;
}

interface ImageSizeControlProps {
  value: string | undefined;
  disabled?: boolean;
  resetKey?: string;
  selectAriaLabel: string;
  onChange: (value: string | undefined) => void;
}

export function ImageSizeControl({
  value,
  disabled = false,
  resetKey = '',
  selectAriaLabel,
  onChange,
}: ImageSizeControlProps) {
  const { t } = useI18n();
  const [customDialogOpen, setCustomDialogOpen] = useState(false);
  const [customDraft, setCustomDraft] = useState('');
  const [customError, setCustomError] = useState(false);

  const isPreset = !value || IMAGE_SIZE_OPTIONS.includes(value as (typeof IMAGE_SIZE_OPTIONS)[number]);
  const currentCustomSize = isPreset ? undefined : value;
  const selectValue = currentCustomSize ?? value ?? '';

  useEffect(() => {
    setCustomDialogOpen(false);
    setCustomDraft(currentCustomSize ?? '');
    setCustomError(false);
  }, [currentCustomSize, resetKey]);

  function openCustomDialog() {
    setCustomDraft(currentCustomSize ?? '');
    setCustomError(false);
    setCustomDialogOpen(true);
  }

  function handleSelectChange(nextValue: string) {
    if (nextValue === CUSTOM_IMAGE_SIZE) {
      openCustomDialog();
      return;
    }
    onChange(nextValue || undefined);
  }

  function handleCustomSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const normalized = customDraft.trim();
    if (!isValidCustomImageSize(normalized)) {
      setCustomError(true);
      return;
    }
    setCustomError(false);
    setCustomDialogOpen(false);
    onChange(normalized);
  }

  return (
    <div className="image-size-control">
      <select
        value={selectValue}
        disabled={disabled}
        aria-label={selectAriaLabel}
        onChange={(event) => handleSelectChange(event.target.value)}
      >
        <option value="">{t('provider.imageDefault')}</option>
        {IMAGE_SIZE_OPTIONS.map((size) => (
          <option key={size} value={size}>{size}</option>
        ))}
        {currentCustomSize ? <option value={currentCustomSize}>{currentCustomSize}</option> : null}
        <option value={CUSTOM_IMAGE_SIZE}>{t('provider.imageCustom')}</option>
      </select>
      <Dialog
        open={customDialogOpen}
        title={t('provider.imageCustomDialogTitle')}
        description={t('provider.imageCustomDialogDescription')}
        closeLabel={t('common.close')}
        onClose={() => setCustomDialogOpen(false)}
        size="sm"
        className="image-size-custom-dialog"
        footer={(
          <>
            <Button type="button" variant="ghost" onClick={() => setCustomDialogOpen(false)}>
              {t('common.cancel')}
            </Button>
            <Button type="submit" variant="primary" form="image-size-custom-form">
              {t('common.confirm')}
            </Button>
          </>
        )}
      >
        <form id="image-size-custom-form" className="image-size-custom-dialog__form" onSubmit={handleCustomSubmit}>
          <label htmlFor="image-size-custom-input">{t('provider.imageSize')}</label>
          <input
            id="image-size-custom-input"
            type="text"
            value={customDraft}
            onChange={(event) => {
              setCustomDraft(event.target.value);
              setCustomError(false);
            }}
            placeholder={t('provider.imageCustomPlaceholder')}
            aria-invalid={customError}
            aria-describedby={customError ? 'image-size-custom-error' : undefined}
            autoComplete="off"
            inputMode="numeric"
            spellCheck={false}
          />
          {customError ? (
            <span id="image-size-custom-error" className="image-size-custom-dialog__error" role="alert">
              {t('provider.imageCustomInvalid')}
            </span>
          ) : null}
        </form>
      </Dialog>
    </div>
  );
}
