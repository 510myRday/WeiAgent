import { useEffect } from 'react';

export function useBeforeUnloadGuard(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return undefined;
    const protectPendingChanges = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', protectPendingChanges);
    return () => window.removeEventListener('beforeunload', protectPendingChanges);
  }, [enabled]);
}
