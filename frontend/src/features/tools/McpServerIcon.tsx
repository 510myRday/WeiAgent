import { Cable } from 'lucide-react';

import { LocalAssetIcon } from '../../components/LocalAssetIcon';

export function McpServerIcon({ logoUrl, size = 18 }: { logoUrl?: string | null; size?: number }) {
  return (
    <LocalAssetIcon
      source={logoUrl}
      fallback={<Cable size={size} />}
      className="mcp-server-icon"
      size={size}
    />
  );
}
