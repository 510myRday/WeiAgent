export type AvatarPreset = {
  id: string;
  label: string;
  bg: string;
  color: string;
  icon: 'bot' | 'sparkles' | 'zap' | 'heart' | 'star' | 'smile';
};

export const AVATAR_PRESETS: AvatarPreset[] = [
  { id: 'default', label: '经典机器人', bg: '#e9f9f0', color: '#22c55e', icon: 'bot' },
  { id: 'sparkle', label: '星光闪耀', bg: '#eff6ff', color: '#3b82f6', icon: 'sparkles' },
  { id: 'energy', label: '能量闪电', bg: '#fefce8', color: '#eab308', icon: 'zap' },
  { id: 'love', label: '爱心守护', bg: '#fdf2f8', color: '#ec4899', icon: 'heart' },
  { id: 'star', label: '超级之星', bg: '#f5f3ff', color: '#8b5cf6', icon: 'star' },
  { id: 'happy', label: '开心笑脸', bg: '#fff7ed', color: '#f97316', icon: 'smile' },
];

const STORAGE_KEY = 'app-avatar-preset-id';

export function getSavedAvatarId(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) || 'default';
  } catch {
    return 'default';
  }
}

export function saveAvatarId(id: string): void {
  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch { /* ignore */ }
}

export function getPresetById(id: string): AvatarPreset {
  return AVATAR_PRESETS.find(p => p.id === id) || AVATAR_PRESETS[0];
}
