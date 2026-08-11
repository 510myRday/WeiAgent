import { useEffect, useState } from 'react';
import { Cable } from 'lucide-react';

import { apiAuthorizationHeaders } from '../../apiAuth';

export function isLocalMcpLogoSource(source: string): boolean {
  return /^file:\/\//i.test(source)
    || /^[a-z]:[\\/]/i.test(source)
    || /^\\\\/.test(source)
    || /^\/(?!\/)/.test(source);
}

export function McpServerIcon({ logoUrl, size = 18 }: { logoUrl?: string | null; size?: number }) {
  const source = logoUrl?.trim() || '';
  const [failedSource, setFailedSource] = useState('');
  const [displaySource, setDisplaySource] = useState('');

  useEffect(() => {
    setFailedSource('');
    setDisplaySource(isLocalMcpLogoSource(source) ? '' : source);
    if (!source || !isLocalMcpLogoSource(source)) return;

    const controller = new AbortController();
    let objectUrl = '';
    const query = new URLSearchParams({ path: source });
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
          setFailedSource(source);
        }
      });

    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [source]);

  return (
    <span className="mcp-server-icon" aria-hidden="true">
      {displaySource && failedSource !== source ? (
        <img src={displaySource} alt="" onError={() => setFailedSource(source)} />
      ) : (
        <Cable size={size} />
      )}
    </span>
  );
}
