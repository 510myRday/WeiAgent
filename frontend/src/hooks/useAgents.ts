import { useState, useCallback } from 'react';
import type { Agent, CreateAgentPayload, UpdateAgentPayload } from '../types';
import {
  listAgents,
  createAgent,
  updateAgent,
  deleteAgent,
} from '../api';

export function useAgents() {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [activeAgentId, setActiveAgentId] = useState('');
  const [isLoadingAgents, setIsLoadingAgents] = useState(true);

  const refreshAgents = useCallback(async () => {
    setIsLoadingAgents(true);
    try {
      const next = await listAgents();
      setAgents(next);
      setActiveAgentId((current) =>
        current && next.some((a) => a.id === current) ? current : next[0]?.id ?? '',
      );
      return next;
    } finally {
      setIsLoadingAgents(false);
    }
  }, []);

  const addAgent = useCallback(async (payload: CreateAgentPayload) => {
    const created = await createAgent(payload);
    setAgents((current) => [...current, created]);
    setActiveAgentId(created.id);
    return created;
  }, []);

  const editAgent = useCallback(async (id: string, payload: UpdateAgentPayload) => {
    const updated = await updateAgent(id, payload);
    setAgents((current) =>
      current.map((a) => (a.id === updated.id ? updated : a)),
    );
    return updated;
  }, []);

  const removeAgent = useCallback(async (id: string) => {
    await deleteAgent(id);
    setAgents((current) => current.filter((a) => a.id !== id));
    setActiveAgentId('');
  }, []);

  return {
    agents,
    setAgents,
    activeAgentId,
    setActiveAgentId,
    isLoadingAgents,
    refreshAgents,
    addAgent,
    editAgent,
    removeAgent,
  };
}

