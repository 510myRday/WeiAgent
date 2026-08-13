import { useEffect, useState } from 'react';

import { apiAuthorizationHeaders } from '../apiAuth';

export function isLocalAssetSource(source: string): boolean {
  return /^file:\/\//i.test(source)
    || /^[a-z]:[\\/]/i.test(source)
    || /^\\\\/.test(source)
    || /^\/(?!\/)/.test(source);
}

export function useLocalAssetUrl(source: string): { displaySource: string; failed: boolean; onError: () => void } {
  const normalized = source.trim();
  const [failedSource, setFailedSource] = useState('');
  const [displaySource, setDisplaySource] = useState('');

  useEffect(() => {
    setFailedSource('');
    setDisplaySource(isLocalAssetSource(normalized) ? '' : normalized);
    if (!normalized || !isLocalAssetSource(normalized)) return;

    const controller = new AbortController();
    let objectUrl = '';
    const query = new URLSearchParams({ path: normalized });
    void fetch(`/api/assets/local?${query.toString()}`, {
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
  }, [normalized]);

  return {
    displaySource,
    failed: !displaySource || failedSource === normalized,
    onError: () => setFailedSource(normalized),
  };
}
