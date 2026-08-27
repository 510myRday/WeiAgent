import { useState } from 'react';
import { KeyRound, Save } from 'lucide-react';

import { useI18n } from '../../i18n';
import { Button, Dialog } from '../../components/ui';

interface KeyEditDialogProps {
  currentKey: string;
  currentLabel: string;
  title: string;
  currentKeyLabel: string;
  newKeyLabel: string;
  newKeyPlaceholder: string;
  labelLabel: string;
  labelPlaceholder: string;
  closeLabel: string;
  onSave: (nextKey: string, nextLabel: string) => void;
  onClose: () => void;
}

export function KeyEditDialog({
  currentKey,
  currentLabel,
  title,
  currentKeyLabel,
  newKeyLabel,
  newKeyPlaceholder,
  labelLabel,
  labelPlaceholder,
  closeLabel,
  onSave,
  onClose,
}: KeyEditDialogProps) {
  const { t } = useI18n();
  const [nextKey, setNextKey] = useState('');
  const [nextLabel, setNextLabel] = useState(currentLabel);

  return (
    <Dialog
      open
      title={title}
      icon={<KeyRound size={19} />}
      closeLabel={closeLabel}
      onClose={onClose}
      size="sm"
      className="key-edit-dialog"
      bodyClassName="key-edit-dialog__body"
      footer={(
        <>
          <Button type="button" variant="ghost" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            type="button"
            variant="primary"
            icon={<Save size={15} />}
            onClick={() => onSave(nextKey.trim(), nextLabel.trim())}
          >
            {t('common.save')}
          </Button>
        </>
      )}
    >
      <div className="key-edit-dialog__current">
        <span>{currentKeyLabel}</span>
        <code>{currentKey}</code>
      </div>
      <label className="key-edit-dialog__field" htmlFor="key-edit-dialog-new-key">
        <span>{newKeyLabel}</span>
        <input
          id="key-edit-dialog-new-key"
          type="text"
          value={nextKey}
          placeholder={newKeyPlaceholder}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setNextKey(event.target.value)}
        />
      </label>
      <label className="key-edit-dialog__field" htmlFor="key-edit-dialog-label">
        <span>{labelLabel}</span>
        <input
          id="key-edit-dialog-label"
          value={nextLabel}
          placeholder={labelPlaceholder}
          maxLength={80}
          onChange={(event) => setNextLabel(event.target.value)}
        />
      </label>
    </Dialog>
  );
}
