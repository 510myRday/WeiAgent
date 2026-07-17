import { useEffect, useState } from 'react';
import { Check, CircleAlert, Clock, LoaderCircle, RotateCcw } from 'lucide-react';

export type AutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

const SAVED_VISIBLE_MS = 1800;

interface AutoSaveIndicatorProps {
  status: AutoSaveStatus;
  t: (path: string) => string;
  onRetry?: () => void;
}

/** Shows pending/saving/error, briefly shows saved, then hides. */
export function AutoSaveIndicator({ status, t, onRetry }: AutoSaveIndicatorProps) {
  const [displayStatus, setDisplayStatus] = useState<AutoSaveStatus>(status === 'saved' ? 'idle' : status);

  useEffect(() => {
    if (status === 'saved') {
      setDisplayStatus('saved');
      const timer = window.setTimeout(() => {
        setDisplayStatus('idle');
      }, SAVED_VISIBLE_MS);
      return () => window.clearTimeout(timer);
    }
    setDisplayStatus(status);
    return undefined;
  }, [status]);

  if (displayStatus === 'idle') return null;

  const icon = displayStatus === 'saving'
    ? <LoaderCircle size={15} className="is-spinning" />
    : displayStatus === 'error'
      ? <CircleAlert size={15} />
      : displayStatus === 'saved'
        ? <Check size={15} />
        : <Clock size={15} />;

  return (
    <span className={`auto-save-status is-${displayStatus}`} role="status" aria-live="polite">
      {icon}
      {t(`tools.autoSave.${displayStatus}`)}
      {displayStatus === 'error' && onRetry ? (
        <button className="auto-save-retry" type="button" onClick={onRetry}>
          <RotateCcw size={13} />
          {t('common.retry')}
        </button>
      ) : null}
    </span>
  );
}
