import type { ProviderTemplateId } from './providerTemplates';
import type { ProviderForm } from './settingsPanelUtils';

export type NewProviderDraft = {
  form: ProviderForm;
  templateId: ProviderTemplateId;
};

// Module-level so the unsaved "create provider" form survives SettingsPanel
// unmounts (sidebar navigation). Kept in memory only: the draft may contain an
// API key, which must never be written to local/session storage.
let draft: NewProviderDraft | null = null;

export function saveNewProviderDraft(next: NewProviderDraft): void {
  draft = next;
}

export function loadNewProviderDraft(): NewProviderDraft | null {
  return draft;
}

export function clearNewProviderDraft(): void {
  draft = null;
}
