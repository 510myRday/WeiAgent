import { useCallback, useSyncExternalStore } from 'react';

import { listToolProcesses } from '../api';
import type { ToolProcessInfo } from '../toolTypes';

type ToolProcessSnapshot = {
  process: ToolProcessInfo | null;
  error: string;
  loading: boolean;
  notFound: boolean;
};

type ProcessEntry = {
  processId: string;
  agentId: string;
  snapshot: ToolProcessSnapshot;
  listeners: Set<() => void>;
};

const EMPTY_SNAPSHOT: ToolProcessSnapshot = {
  process: null,
  error: '',
  loading: false,
  notFound: false,
};
const entries = new Map<string, ProcessEntry>();
let pollTimer: number | undefined;
let pollInFlight = false;
let pollFailureCount = 0;

function processKey(processId: string, agentId: string) {
  return `${agentId}:${processId}`;
}

function notify(entry: ProcessEntry, snapshot: ToolProcessSnapshot) {
  entry.snapshot = snapshot;
  entry.listeners.forEach((listener) => listener());
}

function schedulePoll(delay: number) {
  if (
    typeof document === 'undefined'
    || document.visibilityState !== 'visible'
    || entries.size === 0
    || pollTimer !== undefined
    || pollInFlight
  ) {
    return;
  }
  pollTimer = window.setTimeout(() => {
    pollTimer = undefined;
    void pollProcesses();
  }, delay);
}

async function pollProcesses() {
  if (
    pollInFlight
    || entries.size === 0
    || document.visibilityState !== 'visible'
  ) {
    return;
  }
  pollInFlight = true;
  let nextDelay: number | undefined;
  try {
    const processes = await listToolProcesses();
    const byKey = new Map(processes.map((process) => [
      processKey(process.process_id, process.agent_id),
      process,
    ]));
    let hasRunningProcess = false;
    entries.forEach((entry, key) => {
      const process = byKey.get(key) ?? null;
      if (process?.status === 'running') hasRunningProcess = true;
      notify(entry, {
        process,
        error: '',
        loading: false,
        notFound: process === null,
      });
    });
    pollFailureCount = 0;
    if (hasRunningProcess) nextDelay = 1000;
  } catch (cause) {
    pollFailureCount += 1;
    const error = cause instanceof Error ? cause.message : 'Failed to refresh background process';
    entries.forEach((entry) => {
      notify(entry, {
        ...entry.snapshot,
        error,
        loading: false,
        notFound: false,
      });
    });
    nextDelay = Math.min(30000, 1500 * (2 ** Math.min(pollFailureCount - 1, 4)));
  } finally {
    pollInFlight = false;
    if (nextDelay !== undefined) schedulePoll(nextDelay);
  }
}

function subscribe(processId: string, agentId: string, listener: () => void) {
  if (!processId || !agentId) return () => {};
  const key = processKey(processId, agentId);
  let entry = entries.get(key);
  if (!entry) {
    entry = {
      processId,
      agentId,
      snapshot: {
        process: null,
        error: '',
        loading: true,
        notFound: false,
      },
      listeners: new Set(),
    };
    entries.set(key, entry);
  }
  entry.listeners.add(listener);
  schedulePoll(0);
  return () => {
    const current = entries.get(key);
    current?.listeners.delete(listener);
    if (current?.listeners.size === 0) entries.delete(key);
    if (entries.size === 0 && pollTimer !== undefined) {
      window.clearTimeout(pollTimer);
      pollTimer = undefined;
    }
  };
}

function getSnapshot(processId: string, agentId: string) {
  if (!processId || !agentId) return EMPTY_SNAPSHOT;
  return entries.get(processKey(processId, agentId))?.snapshot ?? EMPTY_SNAPSHOT;
}

export function updateToolProcessCache(process: ToolProcessInfo) {
  const entry = entries.get(processKey(process.process_id, process.agent_id));
  if (!entry) return;
  notify(entry, {
    process,
    error: '',
    loading: false,
    notFound: false,
  });
}

export function useToolProcess(processId: string | undefined, agentId: string) {
  const id = processId ?? '';
  const subscribeToProcess = useCallback(
    (listener: () => void) => subscribe(id, agentId, listener),
    [agentId, id],
  );
  const readSnapshot = useCallback(
    () => getSnapshot(id, agentId),
    [agentId, id],
  );
  return useSyncExternalStore(subscribeToProcess, readSnapshot, () => EMPTY_SNAPSHOT);
}

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'visible') {
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
        pollTimer = undefined;
      }
      return;
    }
    if (entries.size > 0) schedulePoll(0);
  });
}
