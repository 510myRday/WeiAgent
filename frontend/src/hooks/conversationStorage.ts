import type { Conversation } from '../types';

export const TOOL_START_STORAGE_PREFIX = 'weiagent:tool-start:';
export const UNREAD_CONVERSATIONS_STORAGE_KEY = 'weiagent:chat-unread-conversations:v1';
export const CONVERSATION_SNAPSHOTS_STORAGE_KEY = 'weiagent:chat-conversation-snapshots:v1';
/** Entries persist the tool start timestamp; interrupted runs never clear them. */
const TOOL_START_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

export type UnreadConversationMap = Record<string, string[]>;
export type ConversationSnapshotMap = Record<string, number>;

export function readJsonStorage<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? JSON.parse(raw) as T : fallback;
  } catch {
    return fallback;
  }
}

export function writeJsonStorage<T>(key: string, value: T): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage may be unavailable in privacy-restricted browser contexts.
  }
}

export function normalizeUnreadConversationMap(value: UnreadConversationMap): UnreadConversationMap {
  return Object.fromEntries(Object.entries(value).flatMap(([agentId, ids]) => {
    const normalized = Array.isArray(ids) ? [...new Set(ids)].filter(Boolean) : [];
    return normalized.length > 0 ? [[agentId, normalized]] : [];
  }));
}

export function unreadMapsEqual(left: UnreadConversationMap, right: UnreadConversationMap): boolean {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  if (leftKeys.length !== rightKeys.length) return false;
  return leftKeys.every((agentId) => {
    const leftIds = left[agentId] ?? [];
    const rightIds = right[agentId] ?? [];
    return leftIds.length === rightIds.length && leftIds.every((id, index) => id === rightIds[index]);
  });
}

function toolStartStorageKey(callId: string): string {
  return `${TOOL_START_STORAGE_PREFIX}${callId}`;
}

/**
 * Startup sweep: drop tool-start markers leaked by interrupted runs. The stored
 * value is the start timestamp in ms, so anything older than 7 days (or not
 * parseable as a timestamp) is stale.
 */
export function sweepStaleToolStartTimes(): void {
  try {
    const staleKeys: string[] = [];
    for (let index = 0; index < window.localStorage.length; index += 1) {
      const key = window.localStorage.key(index);
      if (!key || !key.startsWith(TOOL_START_STORAGE_PREFIX)) continue;
      const startedAtMs = Number(window.localStorage.getItem(key));
      if (
        !Number.isFinite(startedAtMs)
        || startedAtMs <= 0
        || Date.now() - startedAtMs > TOOL_START_MAX_AGE_MS
      ) {
        staleKeys.push(key);
      }
    }
    staleKeys.forEach((key) => window.localStorage.removeItem(key));
  } catch {
    // Storage may be unavailable in privacy-restricted browser contexts.
  }
}

export function readToolStartTime(callId: string): number | undefined {
  try {
    const value = Number(window.localStorage.getItem(toolStartStorageKey(callId)));
    return Number.isFinite(value) && value > 0 ? value : undefined;
  } catch {
    return undefined;
  }
}

export function writeToolStartTime(callId: string, startedAtMs: number): void {
  try {
    window.localStorage.setItem(toolStartStorageKey(callId), String(startedAtMs));
  } catch {
    // Storage may be unavailable in privacy-restricted browser contexts.
  }
}

export function clearToolStartTime(callId: string): void {
  try {
    window.localStorage.removeItem(toolStartStorageKey(callId));
  } catch {
    // Storage may be unavailable in privacy-restricted browser contexts.
  }
}


export function isEmptyConversation(conversation: Conversation): boolean {
  return conversation.message_count === 0;
}

export function sortConversationList(conversations: Conversation[]): Conversation[] {
  return [...conversations].sort((left, right) => {
    const pinOrder = Number(right.is_pinned) - Number(left.is_pinned);
    if (pinOrder !== 0) return pinOrder;
    return new Date(right.updated_at).getTime() - new Date(left.updated_at).getTime();
  });
}
