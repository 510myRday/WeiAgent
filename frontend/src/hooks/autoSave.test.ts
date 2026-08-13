import { describe, expect, it } from 'vitest';

import { nextAutoSaveRetryDelay, AUTO_SAVE_RETRY_DELAY_MS, AUTO_SAVE_MAX_RETRY_DELAY_MS } from './useSerializedAutoSave';
import { isLocalAssetSource } from './useLocalAssetUrl';

describe('nextAutoSaveRetryDelay', () => {
  it('uses exponential backoff and caps the delay', () => {
    expect(nextAutoSaveRetryDelay(1)).toBe(AUTO_SAVE_RETRY_DELAY_MS);
    expect(nextAutoSaveRetryDelay(2)).toBe(AUTO_SAVE_RETRY_DELAY_MS * 2);
    expect(nextAutoSaveRetryDelay(20)).toBe(AUTO_SAVE_MAX_RETRY_DELAY_MS);
  });
});

describe('isLocalAssetSource', () => {
  it('detects local file paths and rejects remote urls', () => {
    expect(isLocalAssetSource('file:///C:/icons/a.png')).toBe(true);
    expect(isLocalAssetSource('C:\\\\icons\\\\a.png')).toBe(true);
    expect(isLocalAssetSource('/usr/share/icon.png')).toBe(true);
    expect(isLocalAssetSource('https://cdn.example.com/a.png')).toBe(false);
  });
});
