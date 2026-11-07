import type { AvatarIcon } from './types';

export type AvatarPreset = {
  id: string;
  label: string;
  icon: AvatarIcon;
};

export const AVATAR_PRESETS: AvatarPreset[] = [
  { id: 'default', label: '经典机器人', icon: 'bot' },
  { id: 'sparkle', label: '星光闪耀', icon: 'sparkles' },
  { id: 'energy', label: '能量闪电', icon: 'zap' },
  { id: 'love', label: '爱心守护', icon: 'heart' },
  { id: 'star', label: '超级之星', icon: 'star' },
  { id: 'happy', label: '开心笑脸', icon: 'smile' },
  { id: 'chief', label: '首席专家', icon: 'crown' },
  { id: 'thinker', label: '深度思考', icon: 'brain' },
  { id: 'navigator', label: '方向导航', icon: 'compass' },
];

// The source image is intentionally kept as one asset. Each avatar is cropped
// at runtime with CSS background positioning instead of being pre-split into
// nine files.
const AVATAR_TILE_POSITIONS = [
  '2% 2%', '50% 2%', '98% 2%',
  '2% 50%', '50% 50%', '98% 50%',
  '2% 98%', '50% 98%', '98% 98%',
] as const;

export const AVATAR_COLORS = [
  '#22c55e',
  '#2563eb',
  '#8b5cf6',
  '#ec4899',
  '#f97316',
  '#eab308',
  '#06b6d4',
  '#64748b',
];

export function avatarBackground(color: string): string {
  return `${color}18`;
}

export function defaultAvatarColor(icon: AvatarIcon): string {
  const index = AVATAR_PRESETS.findIndex((preset) => preset.icon === icon);
  return AVATAR_COLORS[index >= 0 ? index % AVATAR_COLORS.length : 0];
}

export function getPresetByIcon(icon: string): AvatarPreset {
  return AVATAR_PRESETS.find((p) => p.icon === icon) || AVATAR_PRESETS[0];
}

export function avatarTilePosition(icon: AvatarIcon): string {
  const index = AVATAR_PRESETS.findIndex((preset) => preset.icon === icon);
  return AVATAR_TILE_POSITIONS[index >= 0 ? index : 0];
}
