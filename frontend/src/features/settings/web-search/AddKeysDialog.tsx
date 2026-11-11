import { useState } from 'react';
import { KeyRound, LoaderCircle, Plus } from 'lucide-react';

import type { WebSearchSourceConfig } from '../../../toolTypes';
import { useI18n } from '../../../i18n';
import { Button, Dialog } from '../../../components/ui';
import { providerLabel } from './utils';

interface AddKeysDialogProps {
  source: WebSearchSourceConfig;
  isAdding: boolean;
  onSubmit: (rawText: string) => void;
  onClose: () => void;
}

export function AddKeysDialog({ source, isAdding, onSubmit, onClose }: AddKeysDialogProps) {
  const { t } = useI18n();
  const [draft, setDraft] = useState('');

  return (
    <Dialog
      open
      title={t('tools.webSearch.addKeysDialogTitle', { provider: providerLabel(source.provider, t) })}
      description={t('tools.webSearch.addKeysHint')}
      icon={<KeyRound size={19} />}
      closeLabel={t('common.close')}
      onClose={() => {
        if (!isAdding) onClose();
      }}
      closeOnBackdrop={!isAdding}
      closeOnEscape={!isAdding}
      size="sm"
      className="web-search-key-dialog"
      bodyClassName="web-search-key-dialog__body"
      footer={(
        <>
          <Button
            type="button"
            variant="ghost"
            onClick={onClose}
            disabled={isAdding}
          >
            {t('common.cancel')}
          </Button>
          <Button
            type="button"
            variant="primary"
            icon={isAdding ? <LoaderCircle size={15} className="is-spinning" /> : <Plus size={15} />}
            onClick={() => onSubmit(draft)}
            disabled={isAdding || !draft.trim()}
          >
            {t('tools.webSearch.addKey')}
          </Button>
        </>
      )}
    >
      <label className="web-search-key-dialog__field" htmlFor={`web-search-key-input-${source.id}`}>
        <span>{t('tools.webSearch.keysLabel')}</span>
        <textarea
          id={`web-search-key-input-${source.id}`}
          className="pattern-input web-search-key-dialog__input"
          value={draft}
          placeholder={t('tools.webSearch.keyPlaceholder')}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
              event.preventDefault();
              onSubmit(draft);
            }
          }}
        />
      </label>
    </Dialog>
  );
}
