import { useState, useCallback } from 'react';
import type { AppSettings } from '../types';
import { readSettings, updateSettings } from '../api';
import { defaultAvatarColor } from '../avatarPresets';

const defaultSettings: AppSettings = {
  host: '',
  port: 0,
  log_level: 'info',
  avatar_icon: 'bot',
  avatar_color: defaultAvatarColor('bot'),
  language: 'zh',
};

export function useSettings() {
  const [settingsForm, setSettingsForm] = useState<AppSettings>(defaultSettings);

  const refreshSettings = useCallback(async () => {
    const next = await readSettings();
    setSettingsForm(next);
    return next;
  }, []);

  const saveSettings = useCallback(async (payload: AppSettings) => {
    const updated = await updateSettings(payload);
    setSettingsForm(updated);
    return updated;
  }, []);

  return { settingsForm, setSettingsForm, refreshSettings, saveSettings };
}
