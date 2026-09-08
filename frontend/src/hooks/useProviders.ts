import { useState, useCallback } from 'react';
import type { ModelProvider, ModelProviderPayload } from '../types';
import {
  listModelProviders,
  createModelProvider,
  updateModelProvider,
  deleteModelProvider,
} from '../api';

export function useProviders() {
  const [providers, setProviders] = useState<ModelProvider[]>([]);
  const [activeProviderId, setActiveProviderId] = useState('');

  const refreshProviders = useCallback(async () => {
    const next = await listModelProviders();
    setProviders(next);
    setActiveProviderId((current) =>
      current && next.some((p) => p.id === current) ? current : next[0]?.id ?? '',
    );
    return next;
  }, []);

  const addProvider = useCallback(async (payload: ModelProviderPayload) => {
    const created = await createModelProvider(payload);
    setProviders((current) => [...current, created]);
    setActiveProviderId(created.id);
    return created;
  }, []);

  const editProvider = useCallback(async (id: string, payload: Partial<ModelProviderPayload>) => {
    const updated = await updateModelProvider(id, payload);
    setProviders((current) =>
      current.map((p) => (p.id === updated.id ? updated : p)),
    );
    return updated;
  }, []);

  const removeProvider = useCallback(async (id: string) => {
    await deleteModelProvider(id);
    setProviders((current) => {
      const next = current.filter((p) => p.id !== id);
      setActiveProviderId((active) => active === id ? next[0]?.id ?? '' : active);
      return next;
    });
  }, []);

  return {
    providers,
    activeProviderId,
    setActiveProviderId,
    refreshProviders,
    addProvider,
    editProvider,
    removeProvider,
  };
}
