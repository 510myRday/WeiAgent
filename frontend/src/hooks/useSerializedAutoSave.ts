import { useCallback, useEffect, useRef, useState } from 'react';

import { useBeforeUnloadGuard } from './useBeforeUnloadGuard';

export type AutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

export const AUTO_SAVE_DELAY_MS = 600;
export const AUTO_SAVE_RETRY_DELAY_MS = 1200;
export const AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;

export function nextAutoSaveRetryDelay(attempt: number): number {
  return Math.min(
    AUTO_SAVE_MAX_RETRY_DELAY_MS,
    AUTO_SAVE_RETRY_DELAY_MS * (2 ** Math.min(attempt - 1, 5)),
  );
}

type UseSerializedAutoSaveOptions<T extends { contextKey: string; serialized: string }> = {
  contextKey: string;
  enabled: boolean;
  item: T | null;
  persist: (item: T) => Promise<void>;
  onError?: (error: unknown, attempt: number) => void;
  onSaved?: (item: T) => void | Promise<void>;
  guardUnload?: boolean;
};

export function useSerializedAutoSave<T extends { contextKey: string; serialized: string }>({
  contextKey,
  enabled,
  item,
  persist,
  onError,
  onSaved,
  guardUnload = true,
}: UseSerializedAutoSaveOptions<T>) {
  const [status, setStatus] = useState<AutoSaveStatus>('idle');
  const persistRef = useRef(persist);
  const onErrorRef = useRef(onError);
  const onSavedRef = useRef(onSaved);
  const isMountedRef = useRef(true);
  const activeContextRef = useRef(contextKey);
  const latestSerializedRef = useRef(new Map<string, string>());
  const persistedSerializedRef = useRef(new Map<string, string>());
  const pendingSavesRef = useRef(new Map<string, T>());
  const retryCountsRef = useRef(new Map<string, number>());
  const saveInFlightRef = useRef(false);
  const savePromiseRef = useRef<Promise<boolean> | null>(null);
  const debounceTimerRef = useRef<number | undefined>(undefined);
  const debounceContextRef = useRef('');
  const retryTimerRef = useRef<number | undefined>(undefined);
  const flushSaveQueueRef = useRef<() => Promise<boolean>>(() => Promise.resolve(true));

  persistRef.current = persist;
  onErrorRef.current = onError;
  onSavedRef.current = onSaved;
  activeContextRef.current = contextKey;

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  const flushSaveQueue = useCallback(async (): Promise<boolean> => {
    if (savePromiseRef.current) return savePromiseRef.current;
    if (saveInFlightRef.current) return false;
    const nextSave = pendingSavesRef.current.values().next().value as T | undefined;
    if (!nextSave) return true;

    pendingSavesRef.current.delete(nextSave.contextKey);
    saveInFlightRef.current = true;
    if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
      setStatus('saving');
    }

    let retryDelay = 0;
    let saved = false;
    const operation = (async () => {
      try {
        await persistRef.current(nextSave);
        saved = true;
        persistedSerializedRef.current.set(nextSave.contextKey, nextSave.serialized);
        retryCountsRef.current.delete(nextSave.contextKey);
        await onSavedRef.current?.(nextSave);
        if (
          isMountedRef.current
          && activeContextRef.current === nextSave.contextKey
          && latestSerializedRef.current.get(nextSave.contextKey) === nextSave.serialized
        ) {
          const queued = pendingSavesRef.current.get(nextSave.contextKey);
          if (queued?.serialized === nextSave.serialized) {
            pendingSavesRef.current.delete(nextSave.contextKey);
          }
          if (debounceTimerRef.current !== undefined && debounceContextRef.current === nextSave.contextKey) {
            window.clearTimeout(debounceTimerRef.current);
            debounceTimerRef.current = undefined;
            debounceContextRef.current = '';
          }
          setStatus('saved');
        }
      } catch (error) {
        const attempt = (retryCountsRef.current.get(nextSave.contextKey) ?? 0) + 1;
        retryCountsRef.current.set(nextSave.contextKey, attempt);
        const isLatest = latestSerializedRef.current.get(nextSave.contextKey) === nextSave.serialized;
        if (isLatest) {
          pendingSavesRef.current.set(nextSave.contextKey, nextSave);
          retryDelay = nextAutoSaveRetryDelay(attempt);
        }
        if (isMountedRef.current && activeContextRef.current === nextSave.contextKey) {
          setStatus('error');
          onErrorRef.current?.(error, attempt);
        }
      } finally {
        saveInFlightRef.current = false;
        if (pendingSavesRef.current.size > 0) {
          const queuedContext = (pendingSavesRef.current.values().next().value as T).contextKey;
          if (queuedContext !== nextSave.contextKey) {
            retryDelay = 0;
          }
          if (retryTimerRef.current !== undefined) {
            window.clearTimeout(retryTimerRef.current);
          }
          retryTimerRef.current = window.setTimeout(() => {
            retryTimerRef.current = undefined;
            void flushSaveQueueRef.current();
          }, retryDelay);
        }
      }
      return saved;
    })();

    savePromiseRef.current = operation;
    try {
      return await operation;
    } finally {
      if (savePromiseRef.current === operation) savePromiseRef.current = null;
    }
  }, []);

  flushSaveQueueRef.current = flushSaveQueue;

  useEffect(() => {
    if (!enabled || !item) return;

    const previousSerialized = latestSerializedRef.current.get(item.contextKey);
    latestSerializedRef.current.set(item.contextKey, item.serialized);
    if (previousSerialized !== item.serialized) {
      retryCountsRef.current.delete(item.contextKey);
    }

    if (persistedSerializedRef.current.get(item.contextKey) === item.serialized) {
      pendingSavesRef.current.delete(item.contextKey);
      if (previousSerialized !== item.serialized) {
        setStatus('saved');
      }
      return;
    }

    pendingSavesRef.current.set(item.contextKey, item);
    setStatus('pending');
    if (debounceTimerRef.current !== undefined) {
      window.clearTimeout(debounceTimerRef.current);
    }
    debounceContextRef.current = item.contextKey;
    debounceTimerRef.current = window.setTimeout(() => {
      debounceTimerRef.current = undefined;
      debounceContextRef.current = '';
      void flushSaveQueueRef.current();
    }, AUTO_SAVE_DELAY_MS);
  }, [enabled, flushSaveQueue, item]);

  useEffect(() => () => {
    if (debounceContextRef.current === contextKey && debounceTimerRef.current !== undefined) {
      window.clearTimeout(debounceTimerRef.current);
      debounceTimerRef.current = undefined;
      debounceContextRef.current = '';
    }
    void flushSaveQueueRef.current();
  }, [contextKey]);

  const clearTimers = useCallback(() => {
    if (debounceTimerRef.current !== undefined) {
      window.clearTimeout(debounceTimerRef.current);
      debounceTimerRef.current = undefined;
      debounceContextRef.current = '';
    }
    if (retryTimerRef.current !== undefined) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = undefined;
    }
  }, []);

  const retryNow = useCallback(() => {
    clearTimers();
    void flushSaveQueueRef.current();
  }, [clearTimers]);

  const flushNow = useCallback(async () => {
    clearTimers();
    let saved = await flushSaveQueueRef.current();
    while (saved && pendingSavesRef.current.size > 0) {
      saved = await flushSaveQueueRef.current();
    }
    return saved;
  }, [clearTimers]);

  const markPersisted = useCallback((key: string, serialized: string) => {
    persistedSerializedRef.current.set(key, serialized);
    latestSerializedRef.current.set(key, serialized);
    pendingSavesRef.current.delete(key);
  }, []);

  const resetContext = useCallback((key: string, serialized = '') => {
    if (serialized) {
      persistedSerializedRef.current.set(key, serialized);
      latestSerializedRef.current.set(key, serialized);
    } else {
      persistedSerializedRef.current.delete(key);
      latestSerializedRef.current.delete(key);
    }
    pendingSavesRef.current.delete(key);
    retryCountsRef.current.delete(key);
    setStatus('idle');
  }, []);

  useBeforeUnloadGuard(
    guardUnload && (status === 'pending' || status === 'saving' || status === 'error'),
  );

  return {
    status,
    setStatus,
    retryNow,
    flushNow,
    markPersisted,
    resetContext,
    hasUnsaved: pendingSavesRef.current.size > 0 || status === 'pending' || status === 'saving' || status === 'error',
    isMountedRef,
  };
}

