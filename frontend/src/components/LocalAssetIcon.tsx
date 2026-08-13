import type { ReactNode } from 'react';

import { useLocalAssetUrl } from '../hooks';

export function LocalAssetIcon({
  source,
  fallback,
  className,
  size = 18,
}: {
  source?: string | null;
  fallback: ReactNode;
  className: string;
  size?: number;
}) {
  const { displaySource, failed, onError } = useLocalAssetUrl(source?.trim() || '');

  return (
    <span className={className} aria-hidden="true" style={{ width: size, height: size }}>
      {displaySource && !failed ? <img src={displaySource} alt="" onError={onError} /> : fallback}
    </span>
  );
}
