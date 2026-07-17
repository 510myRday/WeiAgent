const DRAFT_PREFIX = 'weiagent-chat-draft:';

export function readChatDraft(key: string): string {
  if (!key) return '';
  try {
    return localStorage.getItem(`${DRAFT_PREFIX}${key}`) ?? '';
  } catch {
    return '';
  }
}

export function writeChatDraft(key: string, value: string) {
  if (!key) return;
  try {
    const storageKey = `${DRAFT_PREFIX}${key}`;
    if (value) {
      localStorage.setItem(storageKey, value);
    } else {
      localStorage.removeItem(storageKey);
    }
  } catch {
    // Draft persistence is best-effort when storage is unavailable.
  }
}
