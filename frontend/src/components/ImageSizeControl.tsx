import { useEffect, useState, type FormEvent } from 'react';

import { useI18n } from '../i18n';
import { Button, Dialog } from './ui';

const IMAGE_SIZE_OPTIONS = [
  'auto',
  '1024x1024',
  '1536x1024',
  '1024x1536',
  '1792x1024',
  '1024x1792',
  '1:1',
  '3:4',
  '4:3',
  '16:9',
  '9:16',
] as const;

export const CUSTOM_IMAGE_SIZE = '__custom__';

type CustomImageSizeMode = 'dimensions' | 'ratio';

function isValidCustomImageSize(value: string): boolean {
  const match = value.trim().match(/^(\d{1,5})(x|:)(\d{1,5})$/i);
  if (!match) return false;
  const width = Number(match[1]);
  const height = Number(match[3]);
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
  const [customMode, setCustomMode] = useState<CustomImageSizeMode>('dimensions');

  const isPreset = !value || IMAGE_SIZE_OPTIONS.includes(value as (typeof IMAGE_SIZE_OPTIONS)[number]);
  const currentCustomSize = isPreset ? undefined : value;
  const selectValue = currentCustomSize ?? value ?? '';

  useEffect(() => {
    setCustomDialogOpen(false);
    setCustomDraft(currentCustomSize ?? '');
    setCustomError(false);
    setCustomMode(currentCustomSize?.includes(':') ? 'ratio' : 'dimensions');
  }, [currentCustomSize, resetKey]);

  function openCustomDialog() {
    setCustomDraft(currentCustomSize ?? '');
    setCustomError(false);
    setCustomMode(currentCustomSize?.includes(':') ? 'ratio' : 'dimensions');
    setCustomDialogOpen(true);
  }

  function changeCustomMode(mode: CustomImageSizeMode) {
    setCustomMode(mode);
    setCustomDraft('');
    setCustomError(false);
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
          <div className="image-size-custom-dialog__mode" role="tablist" aria-label={t('provider.imageCustomModeLabel')}>
            <button
              type="button"
              role="tab"
              aria-selected={customMode === 'dimensions'}
              className={customMode === 'dimensions' ? 'is-active' : ''}
              onClick={() => changeCustomMode('dimensions')}
            >
              {t('provider.imageCustomDimensions')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={customMode === 'ratio'}
              className={customMode === 'ratio' ? 'is-active' : ''}
              onClick={() => changeCustomMode('ratio')}
            >
              {t('provider.imageCustomRatio')}
            </button>
          </div>
          <label htmlFor="image-size-custom-input">
            {customMode === 'dimensions' ? t('provider.imageCustomDimensions') : t('provider.imageCustomRatio')}
          </label>
          <input
            id="image-size-custom-input"
            type="text"
            value={customDraft}
            onChange={(event) => {
              setCustomDraft(event.target.value);
              setCustomError(false);
            }}
            placeholder={t(
              customMode === 'dimensions'
                ? 'provider.imageCustomDimensionsPlaceholder'
                : 'provider.imageCustomRatioPlaceholder',
            )}
            aria-invalid={customError}
            aria-describedby={customError ? 'image-size-custom-error' : undefined}
            autoComplete="off"
            inputMode={customMode === 'dimensions' ? 'numeric' : 'text'}
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
