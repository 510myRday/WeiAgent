import { useCallback, useEffect, useRef, useState } from 'react';

import { useFeedback } from '../components/ui';
import { useI18n } from '../i18n';

export function useClipboard(resetDelayMs = 1600) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const [copied, setCopied] = useState(false);
  const resetTimerRef = useRef<number | undefined>(undefined);

  useEffect(() => () => {
    if (resetTimerRef.current !== undefined) {
      window.clearTimeout(resetTimerRef.current);
    }
  }, []);

  const copy = useCallback(async (text: string) => {
    if (!text) return false;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (resetTimerRef.current !== undefined) {
        window.clearTimeout(resetTimerRef.current);
      }
      resetTimerRef.current = window.setTimeout(() => {
        resetTimerRef.current = undefined;
        setCopied(false);
      }, resetDelayMs);
      return true;
    } catch {
      setCopied(false);
      toast.error(t('notification.copyFailed'));
      return false;
    }
  }, [resetDelayMs, t, toast]);

  return { copied, copy };
}
