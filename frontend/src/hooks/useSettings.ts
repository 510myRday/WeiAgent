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
  request_timeout_seconds: 60,
  request_retry_attempts: 5,
  context_compress_threshold_percent: 90,
  llm_log_enabled: false,
  conversation_title_provider_id: null,
  conversation_title_model_id: null,
};

function normalizeSettings(settings: AppSettings): AppSettings {
  const threshold = Number(settings.context_compress_threshold_percent);
  return {
    ...defaultSettings,
    ...settings,
    context_compress_threshold_percent: Number.isInteger(threshold)
      && threshold >= 1
      && threshold <= 99
      ? threshold
      : 90,
  };
}

export function useSettings() {
  const [settingsForm, setSettingsForm] = useState<AppSettings>(defaultSettings);

  const refreshSettings = useCallback(async () => {
    const next = normalizeSettings(await readSettings());
    setSettingsForm(next);
    return next;
  }, []);

  const saveSettings = useCallback(async (payload: AppSettings) => {
    const updated = normalizeSettings(await updateSettings(payload));
    setSettingsForm(updated);
    return updated;
  }, []);

  return { settingsForm, setSettingsForm, refreshSettings, saveSettings };
}

