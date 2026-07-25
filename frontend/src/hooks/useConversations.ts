import { useState, useCallback, useEffect, useRef } from 'react';
import type {
  AgentRun,
  AgentRunPhase,
  AssistantBlock,
  ChatMessage,
  Conversation,
  ConversationUsage,
  ImageAttachment,
  ThinkingLevel,
  ToolCallState,
  ToolCallStatus,
  ToolArguments,
  StreamEvent,
  UiMessage,
  UiMessageItem,
} from '../types';
import {
  listConversations,
  listActiveAgentRuns,
  createConversation,
  generateConversationTitle,
  createMessageRun,
  cancelMessageRun,
  deleteConversation,
  readConversationMessages,
  readActiveConversationRun,
  readConversationUsage,
  streamRunEvents,
  updateConversation,
} from '../api';

const EMPTY_USAGE: ConversationUsage = {
  input_tokens: 0,
  output_tokens: 0,
  cache_write_tokens: 0,
  cache_read_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
};

const TOOL_START_STORAGE_PREFIX = 'weiagent:tool-start:';
/** Entries persist the tool start timestamp; interrupted runs never clear them. */
const TOOL_START_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

function toolStartStorageKey(callId: string): string {
  return `${TOOL_START_STORAGE_PREFIX}${callId}`;
}

/**
 * Startup sweep: drop tool-start markers leaked by interrupted runs. The stored
 * value is the start timestamp in ms, so anything older than 7 days (or not
 * parseable as a timestamp) is stale.
 */
function sweepStaleToolStartTimes(): void {
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

function readToolStartTime(callId: string): number | undefined {
  try {
    const value = Number(window.localStorage.getItem(toolStartStorageKey(callId)));
    return Number.isFinite(value) && value > 0 ? value : undefined;
  } catch {
    return undefined;
  }
}

function writeToolStartTime(callId: string, startedAtMs: number): void {
  try {
    window.localStorage.setItem(toolStartStorageKey(callId), String(startedAtMs));
  } catch {
    // Storage may be unavailable in privacy-restricted browser contexts.
  }
}

function clearToolStartTime(callId: string): void {
  try {
    window.localStorage.removeItem(toolStartStorageKey(callId));
  } catch {
    // Storage may be unavailable in privacy-restricted browser contexts.
  }
}

function genClientId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return 'c' + Math.random().toString(36).slice(2, 10);
}

function isAbortError(cause: unknown): boolean {
  return cause instanceof DOMException && cause.name === 'AbortError';
}

function toUiMessage(msg: ChatMessage & { role: 'user' | 'assistant' }): UiMessage {
  return { ...msg, clientId: msg.id ?? genClientId() };
}

function parseJsonObject(value: string): ToolArguments {
  try {
    const parsed = JSON.parse(value);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function statusFromToolResult(result: unknown): ToolCallStatus {
  const status = isRecord(result) ? result.status : undefined;
  if (
    status === 'completed'
    || status === 'running'
    || status === 'blocked'
    || status === 'rejected'
    || status === 'error'
    || status === 'pending_approval'
  ) {
    return status;
  }
  return 'completed';
}

function isAssistantMessage(item: UiMessageItem): item is UiMessage & { role: 'assistant' } {
  return item.role === 'assistant';
}

function isUserMessage(item: UiMessageItem): item is UiMessage & { role: 'user' } {
  return item.role === 'user';
}

function listTools(message: UiMessage): ToolCallState[] {
  return (message.blocks ?? [])
    .filter((block): block is Extract<AssistantBlock, { type: 'tool' }> => block.type === 'tool')
    .map((block) => block.tool);
}

function updateAssistantTools(
  message: UiMessage,
  mapper: (tool: ToolCallState) => ToolCallState,
): UiMessage & { role: 'assistant' } {
  if (!message.blocks?.length) return { ...message, role: 'assistant' };
  let changed = false;
  const blocks = message.blocks.map((block) => {
    if (block.type !== 'tool') return block;
    const nextTool = mapper(block.tool);
    if (nextTool === block.tool) return block;
    changed = true;
    return { type: 'tool' as const, tool: nextTool };
  });
  return changed
    ? { ...message, role: 'assistant', blocks }
    : { ...message, role: 'assistant' };
}

function upsertToolOnAssistant(
  message: UiMessage,
  tool: ToolCallState,
): UiMessage & { role: 'assistant' } {
  const blocks = [...(message.blocks ?? [])];
  const existingIndex = blocks.findIndex(
    (block) => block.type === 'tool' && block.tool.call_id === tool.call_id,
  );
  if (existingIndex >= 0) {
    const existing = blocks[existingIndex];
    if (existing.type === 'tool') {
      blocks[existingIndex] = {
        type: 'tool',
        tool: {
          ...existing.tool,
          ...tool,
          arguments: { ...existing.tool.arguments, ...tool.arguments },
          arguments_text: tool.arguments_text ?? existing.tool.arguments_text,
          result: tool.result ?? existing.tool.result,
        },
      };
    }
  } else {
    // Promote plain content into an ordered text block before the first tool.
    if (
      message.content
      && !blocks.some((block) => block.type === 'text' || block.type === 'tool')
    ) {
      blocks.push({ type: 'text', content: message.content });
    }
    blocks.push({ type: 'tool', tool });
  }
  return { ...message, role: 'assistant', blocks };
}

function upsertToolOnMessages(
  items: UiMessageItem[],
  tool: ToolCallState,
  agentId: string,
  conversationId: string,
): UiMessageItem[] {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (isUserMessage(item)) break;
    if (isAssistantMessage(item)) {
      return items.map((entry, entryIndex) => (
        entryIndex === index ? upsertToolOnAssistant(entry, tool) : entry
      ));
    }
  }
  const seed: UiMessage = {
    id: '',
    agent_id: agentId,
    conversation_id: conversationId,
    role: 'assistant',
    content: '',
    status: 'streaming',
    created_at: new Date().toISOString(),
    clientId: genClientId(),
    blocks: [{ type: 'tool', tool }],
  };
  return [...items, seed];
}

function patchToolOnMessages(
  items: UiMessageItem[],
  callId: string,
  patch: Partial<ToolCallState> | ((tool: ToolCallState) => ToolCallState),
): UiMessageItem[] {
  return items.map((item) => {
    if (!isAssistantMessage(item) || !item.blocks?.length) return item;
    let changed = false;
    const blocks = item.blocks.map((block) => {
      if (block.type !== 'tool' || block.tool.call_id !== callId) return block;
      changed = true;
          const nextTool = typeof patch === 'function'
        ? patch(block.tool)
        : {
          ...block.tool,
          ...patch,
            arguments: patch.arguments
              ? { ...block.tool.arguments, ...patch.arguments }
              : block.tool.arguments,
            arguments_text: patch.arguments_text ?? block.tool.arguments_text,
            result: patch.result ?? block.tool.result,
        };
      return { type: 'tool' as const, tool: nextTool };
    });
    return changed ? { ...item, blocks } : item;
  });
}

function appendTextDeltaToAssistant(message: UiMessage, delta: string): UiMessage & { role: 'assistant' } {
  const content = (message.content ?? '') + delta;
  const blocks = [...(message.blocks ?? [])];
  const last = blocks[blocks.length - 1];
  if (last && last.type === 'text') {
    blocks[blocks.length - 1] = { type: 'text', content: last.content + delta };
  } else if (blocks.some((block) => block.type === 'tool')) {
    blocks.push({ type: 'text', content: delta });
  }
  return {
    ...message,
    role: 'assistant',
    content,
    blocks: blocks.length ? blocks : message.blocks,
    status: 'streaming',
  };
}

function assistantHasVisibleBody(message: UiMessage): boolean {
  if (message.content?.trim()) return true;
  if (message.reasoning_content?.trim()) return true;
  if (listTools(message).length > 0) return true;
  return false;
}

function interruptActiveRun(
  items: UiMessageItem[],
  reason: string,
  toolStatus: 'rejected' | 'error',
): UiMessageItem[] {
  let activeRunStart = -1;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (isUserMessage(items[index])) {
      activeRunStart = index;
      break;
    }
  }

  const next = items.map((item, index) => {
    if (index <= activeRunStart || !isAssistantMessage(item)) return item;
    return updateAssistantTools(item, (tool) => {
      if (tool.status !== 'running' && tool.status !== 'pending_approval') return tool;
      return {
        ...tool,
        status: tool.status === 'pending_approval' ? 'rejected' as const : toolStatus,
        result: toolStatus === 'error'
          ? { ...tool.result, message: reason }
          : { ...tool.result, reason },
      };
    });
  });

  let assistantIndex = -1;
  for (let index = next.length - 1; index > activeRunStart; index -= 1) {
    const item = next[index];
    if (isAssistantMessage(item) && item.status === 'streaming') {
      assistantIndex = index;
      break;
    }
  }
  if (assistantIndex < 0) return next;
  const assistant = next[assistantIndex];
  if (!isAssistantMessage(assistant)) return next;
  if (!assistantHasVisibleBody(assistant)) {
    return next.filter((_, index) => index !== assistantIndex);
  }
  return next.map((item, index) => (
    index === assistantIndex && isAssistantMessage(item) ? { ...item, status: 'failed' as const } : item
  ));
}

function toUiMessageItems(raw: ChatMessage[]): UiMessageItem[] {
  const items: UiMessageItem[] = [];
  const toolOwner = new Map<string, number>();
  let lastAssistantIndex = -1;

  for (const msg of raw) {
    if (msg.role === 'assistant') {
      const toolCalls = Array.isArray(msg.tool_calls) ? msg.tool_calls : [];
      const tools: ToolCallState[] = toolCalls.map((call) => ({
        call_id: call.id,
        name: call.function.name,
        arguments: parseJsonObject(call.function.arguments),
        started_at_ms: call.started_at_ms ?? readToolStartTime(call.id),
        permission_mode: call.permission_mode,
        status: 'running' as const,
      }));

      // One bubble per user turn: fold all assistant/tool steps into the latest assistant.
      if (lastAssistantIndex >= 0 && isAssistantMessage(items[lastAssistantIndex])) {
        let merged = items[lastAssistantIndex];
        if (msg.reasoning_content?.trim()) {
          merged = {
            ...merged,
            role: 'assistant',
            reasoning_content: [merged.reasoning_content, msg.reasoning_content]
              .filter(Boolean)
              .join("\n\n"),
          };
        }
        if (msg.content.trim()) {
          const blocks = [...(merged.blocks ?? [])];
          if (
            merged.content
            && !blocks.some((block) => block.type === 'text' || block.type === 'tool')
          ) {
            blocks.push({ type: 'text', content: merged.content });
          }
          const lastBlock = blocks[blocks.length - 1];
          if (lastBlock && lastBlock.type === 'text') {
            blocks[blocks.length - 1] = {
              type: 'text',
              content: `${lastBlock.content}\n\n${msg.content}`,
            };
          } else {
            blocks.push({ type: 'text', content: msg.content });
          }
          merged = {
            ...merged,
            role: 'assistant',
            content: merged.content
              ? `${merged.content}\n\n${msg.content}`
              : msg.content,
            blocks: blocks.length ? blocks : undefined,
            usage: msg.usage ?? merged.usage,
            status: msg.status ?? merged.status,
            id: msg.id || merged.id,
          };
        }
        for (const tool of tools) {
          merged = upsertToolOnAssistant(merged, tool);
          toolOwner.set(tool.call_id, lastAssistantIndex);
        }
        if (msg.usage) merged = { ...merged, role: 'assistant', usage: msg.usage };
        items[lastAssistantIndex] = { ...merged, role: 'assistant' };
        continue;
      }

      const blocks: AssistantBlock[] = [];
      if (msg.content.trim()) {
        blocks.push({ type: 'text', content: msg.content });
      }
      for (const tool of tools) {
        blocks.push({ type: 'tool', tool });
      }

      const ui = toUiMessage({
        ...msg,
        role: 'assistant',
        tool_calls: undefined,
      });
      ui.blocks = blocks.length ? blocks : undefined;
      items.push(ui);
      lastAssistantIndex = items.length - 1;
      for (const tool of tools) {
        toolOwner.set(tool.call_id, lastAssistantIndex);
      }
      continue;
    }

    if (msg.role === 'tool') {
      const result = parseJsonObject(msg.content);
      const callId = msg.tool_call_id ?? msg.id;
      const ownerIndex = toolOwner.get(callId);
      const status = statusFromToolResult(result);
      if (ownerIndex !== undefined) {
        const current = items[ownerIndex];
        if (isAssistantMessage(current)) {
          const existing = listTools(current).find((tool) => tool.call_id === callId);
          items[ownerIndex] = upsertToolOnAssistant(current, {
            call_id: callId,
            name: existing?.name ?? 'tool',
            arguments: existing?.arguments ?? {},
            permission_mode: existing?.permission_mode,
            status,
            result,
          });
        }
      } else if (lastAssistantIndex >= 0 && isAssistantMessage(items[lastAssistantIndex])) {
        items[lastAssistantIndex] = upsertToolOnAssistant(items[lastAssistantIndex], {
          call_id: callId,
          name: 'tool',
          arguments: {},
          status,
          result,
        });
        toolOwner.set(callId, lastAssistantIndex);
      } else {
        const orphan: UiMessage = {
          id: msg.id,
          agent_id: msg.agent_id,
          conversation_id: msg.conversation_id,
          role: 'assistant',
          content: '',
          status: 'done',
          created_at: msg.created_at,
          clientId: msg.id ?? genClientId(),
          blocks: [{
            type: 'tool',
            tool: {
              call_id: callId,
              name: 'tool',
              arguments: {},
              status,
              result,
            },
          }],
        };
        items.push(orphan);
        lastAssistantIndex = items.length - 1;
        toolOwner.set(callId, lastAssistantIndex);
      }
      continue;
    }

    if (msg.role === 'user') {
      items.push(toUiMessage(msg as ChatMessage & { role: 'user' }));
      lastAssistantIndex = -1;
    }
  }

  return items;
}

function isEmptyConversation(conversation: Conversation): boolean {
  return conversation.message_count === 0;
}

function sortConversationList(conversations: Conversation[]): Conversation[] {
  return [...conversations].sort((left, right) => {
    const pinOrder = Number(right.is_pinned) - Number(left.is_pinned);
    if (pinOrder !== 0) return pinOrder;
    return new Date(right.updated_at).getTime() - new Date(left.updated_at).getTime();
  });
}

type UseConversationsOptions = {
  newConversationTitle: string;
  imageOnlyConversationTitle: string;
  imageAttachmentTitleNote: string;
  runCancelledMessage: string;
  runInterruptedMessage: string;
  onError?: (msg: string) => void;
};

export function useConversations({
  newConversationTitle,
  imageOnlyConversationTitle,
  imageAttachmentTitleNote,
  runCancelledMessage,
  runInterruptedMessage,
  onError,
}: UseConversationsOptions) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState('');
  const [messages, setMessages] = useState<UiMessageItem[]>([]);
  const [hasOlderMessages, setHasOlderMessages] = useState(false);
  const [isLoadingOlderMessages, setIsLoadingOlderMessages] = useState(false);
  const [isLoadingConversations, setIsLoadingConversations] = useState(false);
  const [conversationsError, setConversationsError] = useState('');
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [runPhase, setRunPhase] = useState<AgentRunPhase>('idle');
  const [retryStatus, setRetryStatus] = useState<{ attempt: number; maxAttempts: number; reason: string } | null>(null);
  const [conversationUsage, setConversationUsage] = useState<ConversationUsage>(EMPTY_USAGE);
  const [runningConversationIds, setRunningConversationIds] = useState<Set<string>>(() => new Set());

  const loadChatIdRef = useRef(0);
  const olderMessagesRequestIdRef = useRef(0);
  const olderMessagesCursorRef = useRef('');
  const conversationsRequestIdRef = useRef(0);
  const runningRequestIdRef = useRef(0);
  const resumeRequestIdRef = useRef(0);
  const sendRequestIdRef = useRef(0);
  const sendAbortControllerRef = useRef<AbortController | null>(null);
  const activeRunIdRef = useRef('');
  const activeStreamRef = useRef({ agentId: '', conversationId: '' });
  const suppressNextLoadConversationIdRef = useRef('');
  const pendingDeltaRef = useRef('');
  const deltaTimerRef = useRef<number | undefined>(undefined);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  // Read via ref inside callbacks (e.g. `send`) so they keep a stable identity
  // while messages update every stream flush.
  const messagesRef = useRef(messages);
  messagesRef.current = messages;

  useEffect(() => {
    sweepStaleToolStartTimes();
  }, []);

  const selectConversation = useCallback((conversationId: string) => {
    ++loadChatIdRef.current;
    ++olderMessagesRequestIdRef.current;
    ++resumeRequestIdRef.current;
    setActiveConversationId(conversationId);
    setMessages([]);
    setConversationUsage(EMPTY_USAGE);
    setHasOlderMessages(false);
    setIsLoadingOlderMessages(false);
    olderMessagesCursorRef.current = '';
    setIsLoadingChat(Boolean(conversationId));
  }, []);

  const loadConversationsForAgent = useCallback(async (agentId: string) => {
    const requestId = ++conversationsRequestIdRef.current;
    setIsLoadingConversations(true);
    setConversationsError('');
    const [conversationsResult, activeRunsResult] = await Promise.allSettled([
      listConversations(agentId),
      listActiveAgentRuns(agentId),
    ]);
    if (requestId !== conversationsRequestIdRef.current) return;

    if (conversationsResult.status === 'fulfilled') {
      const next = conversationsResult.value;
      setConversations(next);
      setActiveConversationId((current) =>
        current && next.some((c) => c.id === current) ? current : next[0]?.id ?? '',
      );
    } else {
      const message = conversationsResult.reason instanceof Error
        ? conversationsResult.reason.message
        : 'Failed to load conversations';
      setConversationsError(message);
      onErrorRef.current?.(message);
    }

    if (activeRunsResult.status === 'fulfilled') {
      setRunningConversationIds(new Set(activeRunsResult.value.map((run) => run.conversation_id)));
    } else {
      setRunningConversationIds(new Set());
      onErrorRef.current?.(
        activeRunsResult.reason instanceof Error
          ? activeRunsResult.reason.message
          : 'Failed to load active runs',
      );
    }
    setIsLoadingConversations(false);
  }, []);

  const loadMessages = useCallback(async (conversationId: string) => {
    resumeRequestIdRef.current += 1;
    olderMessagesRequestIdRef.current += 1;
    const requestId = ++loadChatIdRef.current;
    if (suppressNextLoadConversationIdRef.current === conversationId) {
      suppressNextLoadConversationIdRef.current = '';
      setIsLoadingChat(false);
      return true;
    }
    setIsLoadingChat(true);
    setMessages([]);
    setHasOlderMessages(false);
    olderMessagesCursorRef.current = '';
    try {
      const page = await readConversationMessages(conversationId);
      if (requestId !== loadChatIdRef.current) return false;
      setMessages(toUiMessageItems(page.messages));
      setConversationUsage(page.usage ?? EMPTY_USAGE);
      olderMessagesCursorRef.current = page.next_cursor ?? '';
      setHasOlderMessages(page.has_more);
      return true;
    } catch (cause) {
      if (requestId === loadChatIdRef.current) {
        onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to load messages');
      }
      return false;
    } finally {
      if (requestId === loadChatIdRef.current) setIsLoadingChat(false);
    }
  }, []);

  const clearChat = useCallback(() => {
    ++loadChatIdRef.current;
    ++olderMessagesRequestIdRef.current;
    ++conversationsRequestIdRef.current;
    ++runningRequestIdRef.current;
    ++resumeRequestIdRef.current;
    setConversations([]);
    setActiveConversationId('');
    setMessages([]);
    setConversationUsage(EMPTY_USAGE);
    setHasOlderMessages(false);
    setIsLoadingOlderMessages(false);
    olderMessagesCursorRef.current = '';
    setRunningConversationIds(new Set());
    setIsLoadingConversations(false);
    setConversationsError('');
    setIsLoadingChat(false);
    setRunPhase('idle');
  }, []);

  const refreshRunningConversations = useCallback(async (agentId: string) => {
    const requestId = ++runningRequestIdRef.current;
    try {
      const activeRuns = await listActiveAgentRuns(agentId);
      if (requestId !== runningRequestIdRef.current) return;
      const next = new Set(activeRuns.map((run) => run.conversation_id));
      setRunningConversationIds((current) => {
        if (current.size === next.size && [...next].every((id) => current.has(id))) {
          return current;
        }
        return next;
      });
    } catch {
      // The next refresh will retry while the chat view is active.
    }
  }, []);

  const loadOlderMessages = useCallback(async (conversationId: string) => {
    const cursor = olderMessagesCursorRef.current;
    if (!cursor) return false;
    const requestId = ++olderMessagesRequestIdRef.current;
    setIsLoadingOlderMessages(true);
    try {
      const page = await readConversationMessages(conversationId, cursor);
      if (requestId !== olderMessagesRequestIdRef.current) return false;
      setMessages((current) => [...toUiMessageItems(page.messages), ...current]);
      if (page.usage) setConversationUsage(page.usage);
      olderMessagesCursorRef.current = page.next_cursor ?? '';
      setHasOlderMessages(page.has_more);
      return true;
    } catch (cause) {
      if (requestId === olderMessagesRequestIdRef.current) {
        onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to load older messages');
      }
      return false;
    } finally {
      if (requestId === olderMessagesRequestIdRef.current) setIsLoadingOlderMessages(false);
    }
  }, []);

  const applyDelta = useCallback((delta: string) => {
    setMessages((current) => {
      const last = current[current.length - 1];
      if (last && isAssistantMessage(last)) {
        // Keep one assistant bubble for the whole run, including post-tool text.
        return [
          ...current.slice(0, -1),
          appendTextDeltaToAssistant(
            { ...last, status: last.status === 'failed' ? last.status : 'streaming' },
            delta,
          ),
        ];
      }
      return [
        ...current,
        {
          id: '',
          agent_id: last?.agent_id ?? activeStreamRef.current.agentId,
          conversation_id: last?.conversation_id ?? activeStreamRef.current.conversationId,
          role: 'assistant' as const,
          content: delta,
          status: 'streaming' as const,
          created_at: new Date().toISOString(),
          clientId: genClientId(),
          blocks: undefined,
        },
      ];
    });
  }, []);

  const applyReasoningDelta = useCallback((delta: string) => {
    setMessages((current) => {
      const last = current[current.length - 1];
      if (last && isAssistantMessage(last)) {
        return [
          ...current.slice(0, -1),
          {
            ...last,
            reasoning_content: (last.reasoning_content ?? '') + delta,
            status: 'streaming' as const,
          },
        ];
      }
      return [
        ...current,
        {
          id: '',
          agent_id: last?.agent_id ?? activeStreamRef.current.agentId,
          conversation_id: last?.conversation_id ?? activeStreamRef.current.conversationId,
          role: 'assistant' as const,
          content: '',
          reasoning_content: delta,
          status: 'streaming' as const,
          created_at: new Date().toISOString(),
          clientId: genClientId(),
        },
      ];
    });
  }, []);

  const flushPendingDelta = useCallback(() => {
    if (deltaTimerRef.current !== undefined) {
      window.clearTimeout(deltaTimerRef.current);
      deltaTimerRef.current = undefined;
    }
    const delta = pendingDeltaRef.current;
    pendingDeltaRef.current = '';
    if (delta) applyDelta(delta);
  }, [applyDelta]);

  const appendDelta = useCallback((delta: string) => {
    pendingDeltaRef.current += delta;
    if (deltaTimerRef.current !== undefined) return;
    deltaTimerRef.current = window.setTimeout(() => {
      deltaTimerRef.current = undefined;
      const buffered = pendingDeltaRef.current;
      pendingDeltaRef.current = '';
      if (buffered) applyDelta(buffered);
    }, 32);
  }, [applyDelta]);

  const detachActiveRun = useCallback(() => {
    flushPendingDelta();
    resumeRequestIdRef.current += 1;
    sendRequestIdRef.current += 1;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    activeRunIdRef.current = '';
    setRunPhase('idle');
    setIsSending(false);
    setRetryStatus(null);
  }, [flushPendingDelta]);

  const cancelActiveSend = useCallback(() => {
    flushPendingDelta();
    resumeRequestIdRef.current += 1;
    const runId = activeRunIdRef.current;
    const conversationId = activeStreamRef.current.conversationId;
    const agentId = activeStreamRef.current.agentId;
    sendRequestIdRef.current += 1;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    activeRunIdRef.current = '';
    if (runId) {
      setRunningConversationIds((current) => {
        const next = new Set(current);
        next.delete(conversationId);
        return next;
      });
      void cancelMessageRun(runId)
        .then(async () => {
          try {
            setConversationUsage(await readConversationUsage(conversationId));
          } catch {
            // Usage refresh will retry when the conversation is opened again.
          }
          if (agentId) void loadConversationsForAgent(agentId);
        })
        .catch((cause) => {
          onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to cancel run');
        });
    }
    setMessages((current) => {
      return interruptActiveRun(current, runCancelledMessage, 'rejected');
    });
    setRunPhase('idle');
    setIsSending(false);
    setRetryStatus(null);
  }, [flushPendingDelta, loadConversationsForAgent, runCancelledMessage]);

  const handleStreamEvent = useCallback(
    (evt: StreamEvent) => {
      if (evt.event !== 'delta') {
        flushPendingDelta();
      }
      if (evt.event === 'error') {
        setIsSending(false);
        setRunPhase('idle');
        setRetryStatus(null);
        setMessages((current) => {
          return interruptActiveRun(current, runInterruptedMessage, 'error');
        });
        onErrorRef.current?.(typeof evt.data === 'string' ? evt.data : 'Stream error');
        return;
      }
      if (evt.event === 'done') {
        setIsSending(false);
        setRunPhase('idle');
        setRetryStatus(null);
        setMessages((current) => {
          const last = current[current.length - 1];
          if (last && isAssistantMessage(last) && last.status === 'streaming') {
            if (!assistantHasVisibleBody(last)) {
              return current.slice(0, -1);
            }
            return [...current.slice(0, -1), { ...last, status: 'done' as const }];
          }
          return current;
        });
        return;
      }
      if (evt.event === 'cancelled') {
        setIsSending(false);
        setRunPhase('idle');
        setRetryStatus(null);
        setMessages((current) => {
          return interruptActiveRun(current, runCancelledMessage, 'rejected');
        });
        return;
      }
      if (evt.event === 'phase') {
        const phase = (evt.data as { phase: Exclude<AgentRunPhase, 'idle'> }).phase;
        setRunPhase(phase);
        if (phase !== 'waiting_for_model') {
          setRetryStatus(null);
        }
        return;
      }
      if (evt.event === 'retry') {
        const data = evt.data as { attempt: number; max_attempts: number; reason: string };
        setRetryStatus({
          attempt: data.attempt,
          maxAttempts: data.max_attempts,
          reason: data.reason,
        });
        return;
      }
      if (evt.event === 'user') {
        const msg = toUiMessage(evt.data as Parameters<typeof toUiMessage>[0]);
        setMessages((current) => {
          if (current.some((item) => item.id && item.id === msg.id)) return current;
          const optimisticIndex = current.findIndex((item) => isUserMessage(item) && !item.id && item.content === msg.content);
          if (optimisticIndex >= 0) {
            return current.map((item, index) => index === optimisticIndex ? { ...msg, clientId: item.clientId } : item);
          }
          return [...current, msg];
        });
        return;
      }
      if (evt.event === 'delta') {
        setRetryStatus(null);
        appendDelta(evt.data as string);
        return;
      }
      if (evt.event === 'reasoning_delta') {
        setRetryStatus(null);
        applyReasoningDelta(evt.data as string);
        return;
      }
      if (evt.event === 'assistant') {
        const msg = toUiMessage(evt.data as Parameters<typeof toUiMessage>[0]);
        setMessages((current) => {
          const last = current[current.length - 1];
          if (msg.status === 'failed') {
            if (last && isAssistantMessage(last)) {
              return [
                ...current.slice(0, -1),
                {
                  ...last,
                  id: msg.id || last.id,
                  content: msg.content || last.content,
                  reasoning_content: msg.reasoning_content || last.reasoning_content,
                  usage: msg.usage ?? last.usage,
                  status: 'failed' as const,
                },
              ];
            }
            return [...current, { ...msg, status: 'failed' as const }];
          }
          if (last && isAssistantMessage(last)) {
            return [
              ...current.slice(0, -1),
              {
                ...last,
                id: msg.id || last.id,
                content: msg.content || last.content,
                reasoning_content: msg.reasoning_content || last.reasoning_content,
                usage: msg.usage ?? last.usage,
                status: msg.status ?? (last.status === 'streaming' ? 'done' : last.status),
              },
            ];
          }
          return [...current, { ...msg, status: msg.status ?? 'done' }];
        });
        return;
      }
      if (evt.event === 'tool_call_started') {
        setRunPhase('executing_tool');
        const d = evt.data;
        const startedAtMs = readToolStartTime(d.call_id) ?? Date.now();
        writeToolStartTime(d.call_id, startedAtMs);
        const tool: ToolCallState = {
          call_id: d.call_id,
          name: d.name,
          arguments: {},
          arguments_text: '',
          phase: 'generating_arguments',
          started_at_ms: startedAtMs,
          permission_mode: d.permission_mode,
          status: 'running',
        };
        setMessages((current) => upsertToolOnMessages(
          current,
          tool,
          activeStreamRef.current.agentId,
          activeStreamRef.current.conversationId,
        ));
        return;
      }
      if (evt.event === 'tool_call_args_delta') {
        const d = evt.data;
        setMessages((current) => patchToolOnMessages(current, d.call_id, (tool) => {
          const argumentsText = (tool.arguments_text ?? '') + d.arguments;
          return {
            ...tool,
            phase: 'generating_arguments',
            arguments_text: argumentsText,
            arguments: parseJsonObject(argumentsText),
          };
        }));
        return;
      }
      if (evt.event === 'tool_call') {
        setRunPhase('executing_tool');
        const d = evt.data;
        const startedAtMs = readToolStartTime(d.call_id) ?? Date.now();
        writeToolStartTime(d.call_id, startedAtMs);
        const tool: ToolCallState = {
          call_id: d.call_id,
          name: d.name,
          arguments: d.arguments,
          arguments_text: JSON.stringify(d.arguments),
          phase: 'executing',
          started_at_ms: startedAtMs,
          permission_mode: d.permission_mode,
          status: 'running',
        };
        setMessages((current) => upsertToolOnMessages(
          current,
          tool,
          activeStreamRef.current.agentId,
          activeStreamRef.current.conversationId,
        ));
        return;
      }
      if (evt.event === 'tool_result') {
        const d = evt.data;
        clearToolStartTime(d.call_id);
        setMessages((current) => patchToolOnMessages(current, d.call_id, {
          status: d.result.status,
          result: d.result,
        }));
        return;
      }
      if (evt.event === 'approval_required') {
        setRunPhase('waiting_for_approval');
        const d = evt.data;
        setMessages((current) => patchToolOnMessages(current, d.call_id, (tool) => ({
          ...tool,
          status: 'pending_approval',
          arguments: { ...tool.arguments, ...d },
        })));
        return;
      }
    },
    [
      appendDelta,
      applyReasoningDelta,
      flushPendingDelta,
      runCancelledMessage,
      runInterruptedMessage,
    ],
  );

  const followRun = useCallback(async (
    run: AgentRun,
    agentId: string,
    controller: AbortController,
    requestId: number,
    replayFromStart: boolean,
  ) => {
    activeRunIdRef.current = run.id;
    activeStreamRef.current = { agentId, conversationId: run.conversation_id };
    setRunningConversationIds((current) => new Set(current).add(run.conversation_id));
    setIsSending(true);
    setRunPhase(run.phase ?? 'waiting_for_model');
    setRetryStatus(null);
    if (replayFromStart && run.user_message_id) {
      setMessages((current) => {
        const runStart = current.findIndex(
          (item) => isUserMessage(item) && item.id === run.user_message_id,
        );
        return runStart >= 0 ? current.slice(0, runStart) : current;
      });
    }
    try {
      await streamRunEvents(
        run.id,
        0,
        (event) => {
          if (requestId !== sendRequestIdRef.current) return;
          handleStreamEvent(event);
        },
        controller.signal,
      );
      await loadConversationsForAgent(agentId);
      try {
        setConversationUsage(await readConversationUsage(run.conversation_id));
      } catch {
        // Usage refresh is best-effort: a failed request here must not break
        // the finished run, so keep the previous usage snapshot.
      }
      if (requestId === sendRequestIdRef.current) {
        setRunningConversationIds((current) => {
          const next = new Set(current);
          next.delete(run.conversation_id);
          return next;
        });
      }
    } catch (cause) {
      if (requestId === sendRequestIdRef.current && !isAbortError(cause)) {
        flushPendingDelta();
        setRunningConversationIds((current) => {
          const next = new Set(current);
          next.delete(run.conversation_id);
          return next;
        });
        setMessages((current) => interruptActiveRun(current, runInterruptedMessage, 'error'));
        onErrorRef.current?.(cause instanceof Error ? cause.message : 'Run stream failed');
      }
    } finally {
      if (requestId === sendRequestIdRef.current) {
        sendAbortControllerRef.current = null;
        activeRunIdRef.current = '';
        setRunningConversationIds((current) => {
          const next = new Set(current);
          next.delete(run.conversation_id);
          return next;
        });
        setIsSending(false);
        setRunPhase('idle');
      }
    }
  }, [flushPendingDelta, handleStreamEvent, loadConversationsForAgent, runInterruptedMessage]);

  const resumeActiveRun = useCallback(async (conversationId: string, agentId: string) => {
    const resumeRequestId = ++resumeRequestIdRef.current;
    const run = await readActiveConversationRun(conversationId);
    if (resumeRequestId !== resumeRequestIdRef.current) return;
    if (!run || run.status !== 'running') return;
    const requestId = ++sendRequestIdRef.current;
    const controller = new AbortController();
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = controller;
    await followRun(run, agentId, controller, requestId, true);
  }, [followRun]);

  useEffect(() => () => {
    resumeRequestIdRef.current += 1;
    sendRequestIdRef.current += 1;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    activeRunIdRef.current = '';
    pendingDeltaRef.current = '';
    if (deltaTimerRef.current !== undefined) {
      window.clearTimeout(deltaTimerRef.current);
      deltaTimerRef.current = undefined;
    }
  }, []);

  const send = useCallback(
    async (
      content: string,
      agentId: string,
      isStreamMode: boolean,
      attachments: ImageAttachment[],
      thinkingLevel: ThinkingLevel,
      restartFromMessageId?: string,
    ) => {
      const requestId = ++sendRequestIdRef.current;
      const controller = new AbortController();
      const firstUserMessage = messagesRef.current.find(isUserMessage);
      const isFirstUserMessage = !firstUserMessage
        || (restartFromMessageId !== undefined && firstUserMessage.id === restartFromMessageId);
      const optimisticUserClientId = genClientId();
      const optimisticAssistantClientId = genClientId();
      sendAbortControllerRef.current?.abort();
      sendAbortControllerRef.current = controller;
      setIsSending(true);
      setRunPhase('waiting_for_model');
      setRetryStatus(null);
      setMessages((current) => {
        const restartIndex = restartFromMessageId
          ? current.findIndex(
            (item) => isUserMessage(item) && item.id === restartFromMessageId,
          )
          : -1;
        const retained = restartIndex >= 0 ? current.slice(0, restartIndex) : current;
        return [
          ...retained,
          {
            id: '',
            agent_id: agentId,
            conversation_id: activeConversationId,
            role: 'user' as const,
            content,
            attachments,
            status: 'done' as const,
            created_at: new Date().toISOString(),
            clientId: optimisticUserClientId,
          },
          {
            id: '',
            agent_id: agentId,
            conversation_id: activeConversationId,
            role: 'assistant' as const,
            content: '',
            reasoning_content: '',
            status: 'streaming' as const,
            created_at: new Date().toISOString(),
            clientId: optimisticAssistantClientId,
          },
        ];
      });

      try {
        let conversationId = activeConversationId;
        if (!conversationId) {
          const created = await createConversation(agentId, {
            title: content.trim().slice(0, 40) || (attachments.length > 0
              ? imageOnlyConversationTitle
              : newConversationTitle),
          });
          setConversations((current) => [created, ...current]);
          suppressNextLoadConversationIdRef.current = created.id;
          setActiveConversationId(created.id);
          conversationId = created.id;
          setMessages((current) => current.map((msg) => (
            msg.clientId === optimisticUserClientId || msg.clientId === optimisticAssistantClientId
              ? { ...msg, conversation_id: conversationId }
              : msg
          )));
        }

        activeStreamRef.current = { agentId, conversationId };
        const run = await createMessageRun(
          conversationId,
          content,
          isStreamMode,
          attachments,
          thinkingLevel,
          restartFromMessageId,
          controller.signal,
        );
        if (isFirstUserMessage && content.trim()) {
          const titleInput = attachments.length > 0
            ? `${content.trim()}\n\n${imageAttachmentTitleNote}`
            : content.trim();
          void generateConversationTitle(conversationId, titleInput)
            .then((updated) => {
              setConversations((current) => current.map((conversation) => (
                conversation.id === updated.id ? updated : conversation
              )));
            })
            .catch(() => {
              // Title generation is best effort; the initial-message fallback is already set.
            });
        }
        if (requestId !== sendRequestIdRef.current) return;
        if (restartFromMessageId) {
          try {
            setConversationUsage(await readConversationUsage(conversationId));
          } catch {
            // The final run refresh below will update the usage summary.
          }
        }
        await followRun(run, agentId, controller, requestId, false);
      } catch (cause) {
        if (requestId === sendRequestIdRef.current) {
          setIsSending(false);
          setRunPhase('idle');
          if (!isAbortError(cause)) {
            setMessages((current) => {
              return interruptActiveRun(current, runInterruptedMessage, 'error');
            });
            onErrorRef.current?.(cause instanceof Error ? cause.message : 'Send failed');
          }
        }
      } finally {
        if (requestId === sendRequestIdRef.current) {
          sendAbortControllerRef.current = null;
          setIsSending(false);
          setRunPhase('idle');
        }
      }
    },
    [
      activeConversationId,
      followRun,
      imageAttachmentTitleNote,
      imageOnlyConversationTitle,
      newConversationTitle,
      runInterruptedMessage,
    ],
  );

  const createNewConversation = useCallback(
    async (agentId: string) => {
      const existing = conversations.find(isEmptyConversation);
      if (existing) {
        if (existing.id !== activeConversationId) selectConversation(existing.id);
        return existing;
      }
      const created = await createConversation(agentId, { title: newConversationTitle });
      setConversations((current) => [created, ...current]);
      selectConversation(created.id);
      return created;
    },
    [activeConversationId, conversations, newConversationTitle, selectConversation],
  );

  const removeConversation = useCallback(
    async (conversationId: string, agentId: string) => {
      await deleteConversation(conversationId);
      if (conversationId === activeConversationId) {
        setActiveConversationId('');
        setMessages([]);
        setConversationUsage(EMPTY_USAGE);
        setHasOlderMessages(false);
        olderMessagesCursorRef.current = '';
      }
      await loadConversationsForAgent(agentId);
    },
    [activeConversationId, loadConversationsForAgent],
  );

  const setConversationPinned = useCallback(
    async (conversationId: string, isPinned: boolean) => {
      setConversations((current) => sortConversationList(current.map((conversation) => (
        conversation.id === conversationId
          ? { ...conversation, is_pinned: isPinned }
          : conversation
      ))));
      try {
        const updated = await updateConversation(conversationId, { is_pinned: isPinned });
        setConversations((current) => sortConversationList(current.map((conversation) => (
          conversation.id === conversationId ? updated : conversation
        ))));
        return updated;
      } catch (cause) {
        setConversations((current) => sortConversationList(current.map((conversation) => (
          conversation.id === conversationId && conversation.is_pinned === isPinned
            ? { ...conversation, is_pinned: !isPinned }
            : conversation
        ))));
        throw cause;
      }
    },
    [],
  );

  return {
    conversations,
    activeConversationId,
    selectConversation,
    messages,
    conversationUsage,
    hasOlderMessages,
    isLoadingOlderMessages,
    isLoadingConversations,
    conversationsError,
    isLoadingChat,
    isSending,
    runPhase,
    retryStatus,
    runningConversationIds,
    loadConversationsForAgent,
    refreshRunningConversations,
    loadMessages,
    loadOlderMessages,
    resumeActiveRun,
    detachActiveRun,
    cancelActiveSend,
    clearChat,
    send,
    createNewConversation,
    removeConversation,
    setConversationPinned,
  };
}
