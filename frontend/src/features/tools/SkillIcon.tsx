import { useEffect, useState } from 'react';
import { BookOpen } from 'lucide-react';

import { apiAuthorizationHeaders } from '../../apiAuth';
import { isLocalMcpLogoSource } from './McpServerIcon';

export function SkillIcon({ iconUrl, size = 18 }: { iconUrl?: string | null; size?: number }) {
  const source = iconUrl?.trim() || '';
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
        if (!(error instanceof DOMException && error.name === 'AbortError')) setFailedSource(source);
      });

    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [source]);

  return (
    <span className="skill-icon" aria-hidden="true" style={{ width: size, height: size }}>
      {displaySource && failedSource !== source ? <img src={displaySource} alt="" onError={() => setFailedSource(source)} /> : <BookOpen size={size} />}
    </span>
  );
}
