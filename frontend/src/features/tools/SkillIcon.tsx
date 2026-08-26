import { AppWindow, BookOpen, Cable } from 'lucide-react';

import { LocalAssetIcon } from '../../components/LocalAssetIcon';

const WEIAGENTCLI_SKILL_NAME = 'weiagentcli';

function BuiltinClientSkillIcon({ size }: { size: number }) {
  return (
    <span className="skill-icon-builtin-client" aria-hidden="true" style={{ width: size, height: size }}>
      <AppWindow size={size} strokeWidth={1.8} aria-hidden="true" />
      <Cable className="skill-icon-builtin-client__cable" size={Math.max(8, Math.round(size * 0.58))} strokeWidth={2.1} aria-hidden="true" />
    </span>
  );
}

export function SkillIcon({ skillName, iconUrl, iconProxyUrl, size = 18 }: { skillName: string; iconUrl?: string | null; iconProxyUrl?: string | null; size?: number }) {
  if (skillName === WEIAGENTCLI_SKILL_NAME) return <BuiltinClientSkillIcon size={size} />;

  return (
    <LocalAssetIcon
      source={iconUrl}
      proxyUrl={iconProxyUrl}
      fallback={<BookOpen size={size} />}
      className="skill-icon"
      size={size}
    />
  );
}
