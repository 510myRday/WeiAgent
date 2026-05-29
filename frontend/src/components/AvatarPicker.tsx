import {
  Bot,
  Brain,
  Compass,
  Crown,
  Heart,
  Smile,
  Sparkles,
  Star,
  Zap,
} from 'lucide-react';
import type { ComponentType } from 'react';
import type { AvatarIcon } from '../types';
import {
  AVATAR_COLORS,
  AVATAR_PRESETS,
  avatarBackground,
  defaultAvatarColor,
  getPresetByIcon,
} from '../avatarPresets';

const ICON_MAP: Record<AvatarIcon, ComponentType<{ size?: number; color?: string }>> = {
  bot: Bot,
  sparkles: Sparkles,
  zap: Zap,
  heart: Heart,
  star: Star,
  smile: Smile,
  crown: Crown,
  brain: Brain,
  compass: Compass,
};

export function AvatarBadge({
  icon,
  color,
  size = 18,
  className = 'row-avatar',
}: {
  icon: AvatarIcon;
  color: string;
  size?: number;
  className?: string;
}) {
  const preset = getPresetByIcon(icon);
  const Icon = ICON_MAP[preset.icon];
  const iconOnly = className.split(' ').some((name) => name === 'avatar-icon-only' || name === 'avatar-icon-inline');

  return (
    <span
      className={className}
      style={{
        background: iconOnly ? 'transparent' : avatarBackground(color),
        borderColor: iconOnly ? 'transparent' : color,
        color,
      }}
    >
      <Icon size={size} color={color} />
    </span>
  );
}

export function AvatarPicker({
  icon,
  color,
  title,
  onChange,
}: {
  icon: AvatarIcon;
  color: string;
  title: string;
  onChange: (next: { icon: AvatarIcon; color: string }) => void;
}) {
  return (
    <div className="avatar-editor">
      <div className="avatar-editor-preview">
        <AvatarBadge icon={icon} color={color} size={24} className="row-avatar circular-avatar" />
        <span>{title}</span>
      </div>
      <div className="avatar-editor-grid" aria-label={title}>
        {AVATAR_PRESETS.map((preset) => (
          <button
            key={preset.id}
            type="button"
            className={'avatar-option' + (preset.icon === icon ? ' active' : '')}
            style={{
              background: avatarBackground(preset.icon === icon ? color : defaultAvatarColor(preset.icon)),
              borderColor: preset.icon === icon ? color : 'transparent',
            }}
            onClick={() => onChange({ icon: preset.icon, color })}
            title={preset.label}
            aria-label={preset.label}
          >
            <AvatarBadge
              icon={preset.icon}
              color={preset.icon === icon ? color : defaultAvatarColor(preset.icon)}
              size={19}
              className="avatar-icon-only"
            />
          </button>
        ))}
      </div>
      <div className="avatar-color-grid">
        {AVATAR_COLORS.map((nextColor) => (
          <button
            key={nextColor}
            type="button"
            className={'color-swatch' + (nextColor === color ? ' active' : '')}
            style={{ background: nextColor }}
            onClick={() => onChange({ icon, color: nextColor })}
            aria-label={nextColor}
            title={nextColor}
          />
        ))}
      </div>
    </div>
  );
}
