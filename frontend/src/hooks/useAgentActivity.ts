import { useCallback, useEffect, useRef, useState } from 'react';

import { listConversations } from '../api';
import { usePageVisibility } from './usePageVisibility';

const REFRESH_INTERVAL_MS = 3000;

function latestConversationUpdatedAt(updatedAtValues: string[]): string | undefined {
  return updatedAtValues.reduce<string | undefined>((latest, value) => {
    if (!latest) return value;
    return Date.parse(value) > Date.parse(latest) ? value : latest;
  }, undefined);
}

export function useAgentActivity(agentIds: string[]) {
  const isPageVisible = usePageVisibility();
  const [latestConversationUpdatedAtByAgent, setLatestConversationUpdatedAtByAgent] = useState<Record<string, string>>({});
  const [readyAgentIdsKey, setReadyAgentIdsKey] = useState('');
  const requestIdRef = useRef(0);
  const localActivityRef = useRef<Record<string, string>>({});

  const refreshAgentActivity = useCallback(async (ids: string[] = agentIds) => {
    const uniqueAgentIds = [...new Set(ids.filter(Boolean))];
    const requestId = ++requestIdRef.current;
    const results = await Promise.allSettled(uniqueAgentIds.map(async (agentId) => ({
      agentId,
      conversations: await listConversations(agentId),
    })));
    if (requestId !== requestIdRef.current) return;

    setLatestConversationUpdatedAtByAgent((current) => {
      const next = { ...current };
      const listedAgentIds = new Set(uniqueAgentIds);
      for (const agentId of Object.keys(next)) {
        if (!listedAgentIds.has(agentId)) {
          delete next[agentId];
          delete localActivityRef.current[agentId];
        }
      }
      for (const result of results) {
        if (result.status !== 'fulfilled') continue;
        const latest = latestConversationUpdatedAt(result.value.conversations.map((conversation) => conversation.updated_at));
        const previous = next[result.value.agentId];
        const localActivity = localActivityRef.current[result.value.agentId];
        if (localActivity && latest && Date.parse(latest) >= Date.parse(localActivity)) {
          delete localActivityRef.current[result.value.agentId];
        }
        if (latest && (!previous || Date.parse(latest) > Date.parse(previous))) {
          next[result.value.agentId] = latest;
        } else if (!latest && !localActivity) {
          delete next[result.value.agentId];
        }
      }
      return next;
    });
    setReadyAgentIdsKey(uniqueAgentIds.join('\n'));
  }, [agentIds]);

  const touchAgentActivity = useCallback((agentId: string, updatedAt = new Date().toISOString()) => {
    if (!agentId) return;
    localActivityRef.current[agentId] = updatedAt;
    setLatestConversationUpdatedAtByAgent((current) => {
      const previous = current[agentId];
      if (previous && Date.parse(previous) >= Date.parse(updatedAt)) return current;
      return { ...current, [agentId]: updatedAt };
    });
  }, []);

  useEffect(() => {
    if (agentIds.length === 0) {
      setReadyAgentIdsKey('');
      return;
    }
    if (!isPageVisible) return;
    setReadyAgentIdsKey('');
    void refreshAgentActivity();
    const timer = window.setInterval(() => void refreshAgentActivity(), REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [agentIds, isPageVisible, refreshAgentActivity]);

  const isReady = readyAgentIdsKey === agentIds.join('\n');
  return { latestConversationUpdatedAtByAgent, isReady, refreshAgentActivity, touchAgentActivity };
}
