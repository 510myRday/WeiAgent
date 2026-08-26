import type { ReactNode } from 'react';

import { useLocalAssetUrl } from '../hooks';

export function LocalAssetIcon({
  source,
  fallback,
  className,
  size = 18,
  proxyUrl,
}: {
  source?: string | null;
  fallback: ReactNode;
  className: string;
  size?: number;
  proxyUrl?: string | null;
}) {
  const { displaySource, failed, onError } = useLocalAssetUrl(source?.trim() || '', proxyUrl);

  return (
    <span className={className} aria-hidden="true" style={{ width: size, height: size }}>
      {displaySource && !failed ? <img src={displaySource} alt="" onError={onError} /> : fallback}
    </span>
  );
}
