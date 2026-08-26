import { useEffect, useState } from 'react';

import { apiAuthorizationHeaders } from '../apiAuth';

export function isLocalAssetSource(source: string): boolean {
  return /^file:\/\//i.test(source)
    || /^[a-z]:[\\/]/i.test(source)
    || /^\\\\/.test(source)
    || /^\/(?!\/)/.test(source);
}

export function useLocalAssetUrl(source: string, proxyUrl?: string | null): { displaySource: string; failed: boolean; onError: () => void } {
  const normalized = source.trim();
  const normalizedProxy = proxyUrl?.trim() ?? '';
  const [failedSource, setFailedSource] = useState('');
  const [displaySource, setDisplaySource] = useState('');

  useEffect(() => {
    setFailedSource('');
    setDisplaySource(isLocalAssetSource(normalized) || normalizedProxy ? '' : normalized);
    if (!normalized || (!isLocalAssetSource(normalized) && !normalizedProxy)) return;

    const controller = new AbortController();
    let objectUrl = '';
    const query = new URLSearchParams(isLocalAssetSource(normalized)
      ? { path: normalized }
      : { url: normalized, proxy_url: normalizedProxy });
    const endpoint = isLocalAssetSource(normalized) ? '/api/assets/local' : '/api/assets/remote';
    void fetch(`${endpoint}?${query.toString()}`, {
      headers: apiAuthorizationHeaders(),
      signal: controller.signal,
    })
      .then((response) => {
        if (!response.ok) throw new Error(response.statusText);
        return response.blob();
      })
      .then((blob) => {
        objectUrl = URL.createObjectURL(blob);
        setDisplaySource(objectUrl);
      })
      .catch((error: unknown) => {
        if (!(error instanceof DOMException && error.name === 'AbortError')) {
          setFailedSource(normalized);
        }
      });

    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [normalized, normalizedProxy]);

  return {
    displaySource,
    failed: !displaySource || failedSource === normalized,
    onError: () => setFailedSource(normalized),
  };
}
