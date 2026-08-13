import { BookOpen } from 'lucide-react';

import { LocalAssetIcon } from '../../components/LocalAssetIcon';

export function SkillIcon({ iconUrl, size = 18 }: { iconUrl?: string | null; size?: number }) {
  return (
    <LocalAssetIcon
      source={iconUrl}
      fallback={<BookOpen size={size} />}
      className="skill-icon"
      size={size}
    />
  );
}
