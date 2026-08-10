import { useEffect, useState } from 'react';
import { Cable } from 'lucide-react';

export function McpServerIcon({ logoUrl, size = 18 }: { logoUrl?: string | null; size?: number }) {
  const source = logoUrl?.trim() || '';
  const [failedSource, setFailedSource] = useState('');

  useEffect(() => {
    setFailedSource('');
  }, [source]);

  return (
    <span className="mcp-server-icon" aria-hidden="true">
      {source && failedSource !== source ? (
        <img src={source} alt="" onError={() => setFailedSource(source)} />
      ) : (
        <Cable size={size} />
      )}
    </span>
  );
}
