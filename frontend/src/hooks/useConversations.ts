import { useState, useCallback, useEffect, useRef } from 'react';
import { useI18n } from '../i18n';
import type {
  AgentRun,
  AgentRunPhase,
  Conversation,
  ConversationUsage,
  ChatShortcutAction,
  ContextCompactionEvent,
  ImageAttachment,
  ModelUsage,
  GenerationMode,
  ThinkingLevel,
  StreamEvent,
  ToolCallState,
  UiMessage,
  UiMessageItem,
} from '../types';
import {
  listConversations,
  listActiveAgentRuns,
  createConversation,
  generateConversationTitle,
  createMessageRun,
  steerMessageRun,
  cancelMessageRun,
  deleteConversation,
  readConversationMessages,
  readActiveConversationRun,
  readConversationUsage,
  streamRunEvents,
  updateConversation,
} from '../api';
import {
  appendTextDeltaToAssistant,
  appendReasoningDeltaToAssistant,
  assistantHasVisibleBody,
  genClientId,
  interruptActiveRun,
  isAbortError,
  isAssistantMessage,
  isUserMessage,
  mergeAssistantEvent,
  parseJsonObject,
  patchToolOnMessages,
  toUiMessage,
  toUiMessageItems,
  listTools,
  upsertToolOnMessages,
} from './conversationMessages';
import {
  CONVERSATION_SNAPSHOTS_STORAGE_KEY,
  CONVERSATION_RUN_SNAPSHOTS_STORAGE_KEY,
  UNREAD_CONVERSATIONS_STORAGE_KEY,
  clearToolStartTime,
  isEmptyConversation,
  normalizeUnreadConversationMap,
  readJsonStorage,
  readToolStartTime,
  sortConversationList,
  sweepStaleToolStartTimes,
  unreadMapsEqual,
  writeJsonStorage,
  writeToolStartTime,
  type ConversationSnapshotMap,
  type ConversationRunSnapshotMap,
  type UnreadConversationMap,
} from './conversationStorage';

const EMPTY_USAGE: ConversationUsage = {
  input_tokens: 0,
  output_tokens: 0,
  cache_write_tokens: 0,
  cache_read_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
};

export type PendingChatMessage = {
  id: string;
  content: string;
  attachments: ImageAttachment[];
  agentId: string;
  isStreamMode: boolean;
  thinkingLevel: ThinkingLevel;
  generationMode: GenerationMode;
  shortcutActions: ChatShortcutAction[];
};

export type PendingMessageMoveDirection = 'up' | 'down';

function modelUsageToConversationUsage(usage: ModelUsage): ConversationUsage {
  const promptTokens = usage.total_prompt_tokens && usage.total_prompt_tokens > 0
    ? usage.total_prompt_tokens
    : usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;

  return {
    input_tokens: usage.input_tokens,
    output_tokens: usage.output_tokens,
    cache_write_tokens: usage.cache_write_tokens,
    cache_read_tokens: usage.cache_read_tokens,
    total_prompt_tokens: usage.total_prompt_tokens,
    uncached_input_tokens: usage.uncached_input_tokens,
    cache_hit_ratio: usage.cache_hit_ratio,
    total_tokens: promptTokens + usage.output_tokens,
    cost_usd: usage.cost_usd,
    image_count: usage.image_count,
  };
}

type UseConversationsOptions = {
  activeAgentId: string;
  newConversationTitle: string;
  imageOnlyConversationTitle: string;
  imageAttachmentTitleNote: string;
  runCancelledMessage: string;
  runInterruptedMessage: string;
  onError?: (msg: string) => void;
};

export function useConversations({
  activeAgentId,
  newConversationTitle,
  imageOnlyConversationTitle,
  imageAttachmentTitleNote,
  runCancelledMessage,
  runInterruptedMessage,
  onError,
}: UseConversationsOptions) {
  const { locale } = useI18n();
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
  const [pendingMessages, setPendingMessages] = useState<PendingChatMessage[]>([]);
  const [conversationUsage, setConversationUsage] = useState<ConversationUsage>(EMPTY_USAGE);
  const [runningConversationIds, setRunningConversationIds] = useState<Set<string>>(() => new Set());
  const [unreadConversationIdsByAgent, setUnreadConversationIdsByAgent] = useState<UnreadConversationMap>(
    () => normalizeUnreadConversationMap(readJsonStorage(UNREAD_CONVERSATIONS_STORAGE_KEY, {})),
  );

  const loadChatIdRef = useRef(0);
  const olderMessagesRequestIdRef = useRef(0);
  const olderMessagesCursorRef = useRef('');
  const conversationsRequestIdRef = useRef(0);
  const runningRequestIdRef = useRef(0);
  const resumeRequestIdRef = useRef(0);
  const sendRequestIdRef = useRef(0);
  const sendAbortControllerRef = useRef<AbortController | null>(null);
  const unreadRefreshRequestIdRef = useRef(0);
  const activeRunIdRef = useRef('');
  const activeStreamRef = useRef({ agentId: '', conversationId: '' });
  const suppressNextLoadConversationIdRef = useRef('');
  const pendingDeltaRef = useRef('');
  const deltaTimerRef = useRef<number | undefined>(undefined);
  const onErrorRef = useRef(onError);
  const activeAgentIdRef = useRef(activeAgentId);
  const activeConversationIdRef = useRef(activeConversationId);
  const unreadConversationIdsRef = useRef(unreadConversationIdsByAgent);
  const conversationSnapshotsRef = useRef<ConversationSnapshotMap>(
    readJsonStorage(CONVERSATION_SNAPSHOTS_STORAGE_KEY, {}),
  );
  const conversationRunSnapshotsRef = useRef<ConversationRunSnapshotMap>(
    readJsonStorage(CONVERSATION_RUN_SNAPSHOTS_STORAGE_KEY, {}),
  );
  onErrorRef.current = onError;
  activeAgentIdRef.current = activeAgentId;
  activeConversationIdRef.current = activeConversationId;
  unreadConversationIdsRef.current = unreadConversationIdsByAgent;
  // Read via ref inside callbacks (e.g. `send`) so they keep a stable identity
  // while messages update every stream flush.
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  const conversationsRef = useRef(conversations);
  conversationsRef.current = conversations;
  const pendingMessagesRef = useRef(pendingMessages);
  pendingMessagesRef.current = pendingMessages;
  const sendRef = useRef<((
    content: string,
    agentId: string,
    isStreamMode: boolean,
    attachments: ImageAttachment[],
    thinkingLevel: ThinkingLevel,
    generationMode?: GenerationMode,
    restartFromMessageId?: string,
    shortcutActions?: ChatShortcutAction[],
  ) => Promise<void>)>();

  const updateUnreadConversationIds = useCallback(
    (mapper: (current: UnreadConversationMap) => UnreadConversationMap) => {
      setUnreadConversationIdsByAgent((current) => {
        const next = normalizeUnreadConversationMap(mapper(current));
        if (unreadMapsEqual(current, next)) return current;
        unreadConversationIdsRef.current = next;
        writeJsonStorage(UNREAD_CONVERSATIONS_STORAGE_KEY, next);
        return next;
      });
    },
    [],
  );

  const markConversationRead = useCallback((agentId: string, conversationId: string) => {
    if (!agentId || !conversationId) return;
    updateUnreadConversationIds((current) => ({
      ...current,
      [agentId]: (current[agentId] ?? []).filter((id) => id !== conversationId),
    }));
    delete conversationSnapshotsRef.current[conversationId];
    writeJsonStorage(CONVERSATION_SNAPSHOTS_STORAGE_KEY, conversationSnapshotsRef.current);
  }, [updateUnreadConversationIds]);

  const reconcileConversationList = useCallback((
    agentId: string,
    nextConversations: Conversation[],
    activeRunIds: Map<string, string>,
  ) => {
    const nextUnread = { ...unreadConversationIdsRef.current };
    let unreadChanged = false;
    const visibleConversationId = activeAgentIdRef.current === agentId
      ? activeConversationIdRef.current
      : '';
    for (const conversation of nextConversations) {
      const previousMessageCount = conversationSnapshotsRef.current[conversation.id];
      const messageCountChanged = previousMessageCount !== undefined
        && conversation.message_count > previousMessageCount;
      const isVisible = conversation.id === visibleConversationId;
      const currentRunId = activeRunIds.get(conversation.id);
      const previousRunId = conversationRunSnapshotsRef.current[conversation.id];
      const runFinished = previousRunId !== undefined && previousRunId !== currentRunId;
      const isRunning = currentRunId !== undefined;
      const currentIds = nextUnread[agentId] ?? [];

      const shouldMarkUnread = (messageCountChanged && !isRunning) || runFinished;
      if (shouldMarkUnread && !isVisible && !currentIds.includes(conversation.id)) {
        nextUnread[agentId] = [...currentIds, conversation.id];
        unreadChanged = true;
      }
      if (isVisible && currentIds.includes(conversation.id)) {
        nextUnread[agentId] = currentIds.filter((id) => id !== conversation.id);
        unreadChanged = true;
      }

      // Keep the pre-run count while a background run is active so its final
      // message-count increase is detected on the next poll.
      if (!isRunning || isVisible || previousMessageCount === undefined) {
        conversationSnapshotsRef.current[conversation.id] = conversation.message_count;
      }
      if (currentRunId) {
        conversationRunSnapshotsRef.current[conversation.id] = currentRunId;
      } else {
        delete conversationRunSnapshotsRef.current[conversation.id];
      }
    }

    const listedConversationIds = new Set(nextConversations.map((conversation) => conversation.id));
    const existingUnreadIds = nextUnread[agentId] ?? [];
    const retainedUnreadIds = existingUnreadIds.filter((id) => listedConversationIds.has(id));
    if (retainedUnreadIds.length !== existingUnreadIds.length) {
      nextUnread[agentId] = retainedUnreadIds;
      unreadChanged = true;
    }
    for (const conversationId of Object.keys(conversationRunSnapshotsRef.current)) {
      if (!listedConversationIds.has(conversationId)) {
        delete conversationRunSnapshotsRef.current[conversationId];
      }
    }

    if (unreadChanged) {
      const normalized = normalizeUnreadConversationMap(nextUnread);
      unreadConversationIdsRef.current = normalized;
      writeJsonStorage(UNREAD_CONVERSATIONS_STORAGE_KEY, normalized);
      setUnreadConversationIdsByAgent((current) => unreadMapsEqual(current, normalized) ? current : normalized);
    }
    writeJsonStorage(CONVERSATION_SNAPSHOTS_STORAGE_KEY, conversationSnapshotsRef.current);
    writeJsonStorage(CONVERSATION_RUN_SNAPSHOTS_STORAGE_KEY, conversationRunSnapshotsRef.current);
  }, []);

  const refreshUnreadState = useCallback(async (agentIds: string[]) => {
    const requestId = ++unreadRefreshRequestIdRef.current;
    const uniqueAgentIds = [...new Set(agentIds.filter(Boolean))];
    const results = await Promise.allSettled(uniqueAgentIds.map(async (agentId) => {
      const [conversationsResult, activeRunsResult] = await Promise.all([
        listConversations(agentId),
        listActiveAgentRuns(agentId),
      ]);
      return { agentId, conversations: conversationsResult, activeRuns: activeRunsResult };
    }));
    if (requestId !== unreadRefreshRequestIdRef.current) return;
    const listedConversationIds = new Set<string>();

    for (const result of results) {
      if (result.status !== 'fulfilled') continue;
      const { agentId, conversations: next, activeRuns } = result.value;
      next.forEach((conversation) => listedConversationIds.add(conversation.id));
      reconcileConversationList(
        agentId,
        next,
        new Map(activeRuns.map((run) => [run.conversation_id, run.id])),
      );
      if (agentId !== activeAgentIdRef.current) continue;
      setConversations(sortConversationList(next));
      setRunningConversationIds(new Set(activeRuns.map((run) => run.conversation_id)));
    }
    let snapshotsChanged = false;
    for (const conversationId of Object.keys(conversationSnapshotsRef.current)) {
      if (!listedConversationIds.has(conversationId)) {
        delete conversationSnapshotsRef.current[conversationId];
        snapshotsChanged = true;
      }
    }
    if (snapshotsChanged) {
      writeJsonStorage(CONVERSATION_SNAPSHOTS_STORAGE_KEY, conversationSnapshotsRef.current);
    }
  }, [reconcileConversationList]);

  useEffect(() => {
    sweepStaleToolStartTimes();
  }, []);

  useEffect(() => {
    if (activeAgentId && activeConversationId) {
      markConversationRead(activeAgentId, activeConversationId);
    }
  }, [activeAgentId, activeConversationId, markConversationRead]);

  const selectConversation = useCallback((conversationId: string) => {
    ++loadChatIdRef.current;
    ++olderMessagesRequestIdRef.current;
    ++resumeRequestIdRef.current;
    setActiveConversationId(conversationId);
    setMessages([]);
    setConversationUsage(EMPTY_USAGE);
    setHasOlderMessages(false);
    setIsLoadingOlderMessages(false);
    setRetryStatus(null);
    pendingMessagesRef.current = [];
    setPendingMessages([]);
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
      const activeRunIds = activeRunsResult.status === 'fulfilled'
        ? new Map(activeRunsResult.value.map((run) => [run.conversation_id, run.id]))
        : new Map<string, string>();
      reconcileConversationList(agentId, next, activeRunIds);
      const sorted = sortConversationList(next);
      setConversations(sorted);
      setActiveConversationId((current) =>
        current && sorted.some((c) => c.id === current) ? current : sorted[0]?.id ?? '',
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
  }, [reconcileConversationList]);

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
    setRetryStatus(null);
    pendingMessagesRef.current = [];
    setPendingMessages([]);
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
      if (last && isAssistantMessage(last) && !last.system_event) {
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
      if (last && isAssistantMessage(last) && !last.system_event) {
        return [
          ...current.slice(0, -1),
          appendReasoningDeltaToAssistant(last, delta),
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
        setRetryStatus(null);
        setMessages((current) => {
          return interruptActiveRun(current, runInterruptedMessage, 'error');
        });
        onErrorRef.current?.(typeof evt.data === 'string' ? evt.data : 'Stream error');
        return;
      }
      if (evt.event === 'done') {
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
        setRetryStatus(null);
        setMessages((current) => {
          return interruptActiveRun(current, runCancelledMessage, 'rejected');
        });
        return;
      }
      if (evt.event === 'phase') {
        const phase = (evt.data as { phase: Exclude<AgentRunPhase, 'idle'> }).phase;
        setRunPhase(phase);
        // Each waiting_for_model phase starts a fresh model-request retry
        // budget. Keep the status during the wait itself, then clear it when
        // the next model turn or another run phase begins.
        setRetryStatus(null);
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
      if (evt.event === 'usage') {
        setConversationUsage(modelUsageToConversationUsage(evt.data));
        return;
      }
      if (evt.event === 'context_compacted') {
        const event = evt.data as ContextCompactionEvent;
        setMessages((current) => [...current, {
          id: '',
          agent_id: activeStreamRef.current.agentId,
          conversation_id: activeStreamRef.current.conversationId,
          role: 'assistant' as const,
          content: '',
          status: 'done' as const,
          created_at: event.created_at,
          clientId: genClientId(),
          system_event: { type: 'context_compacted' as const, ...event },
        }]);
        return;
      }
      if (evt.event === 'assistant') {
        // A successful model response ends the current retry budget. The next
        // model turn will receive its own waiting_for_model phase and start at
        // zero again.
        setRetryStatus(null);
        const msg = toUiMessage(evt.data as Parameters<typeof toUiMessage>[0]);
        if (msg.usage) setConversationUsage(modelUsageToConversationUsage(msg.usage));
        setMessages((current) => mergeAssistantEvent(current, msg));
        return;
      }
      if (evt.event === 'tool_call_started') {
        setRetryStatus(null);
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
        if (d.result.runtime === 'builtin_read_images') {
          const conversationId = activeStreamRef.current.conversationId;
          void readConversationMessages(conversationId)
            .then((page) => {
              const readImagesTool = toUiMessageItems(page.messages)
                .filter(isAssistantMessage)
                .flatMap(listTools)
                .find((tool) => tool.call_id === d.call_id);
              const attachments = readImagesTool?.result?.attachments;
              if (attachments?.length) {
                setMessages((current) => patchToolOnMessages(current, d.call_id, (tool) => ({
                  ...tool,
                  result: { ...tool.result, attachments },
                })));
              }
            })
            .catch(() => {
              // The text result remains visible if the attachment refresh fails.
            });
        }
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
    conversationRunSnapshotsRef.current[run.conversation_id] = run.id;
    writeJsonStorage(CONVERSATION_RUN_SNAPSHOTS_STORAGE_KEY, conversationRunSnapshotsRef.current);
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
      generationMode: GenerationMode = 'chat',
      restartFromMessageId?: string,
      shortcutActions: ChatShortcutAction[] = [],
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
            shortcut_actions: shortcutActions,
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

        if (isFirstUserMessage && content.trim()) {
          const fallbackTitle = content.trim().split(/\s+/).join(' ').slice(0, 40);
          const currentConversation = conversationsRef.current.find((conversation) => conversation.id === conversationId);
          const isPlaceholder = !currentConversation
            || currentConversation.title.trim() === newConversationTitle.trim()
            || currentConversation.title.trim() === 'New Conversation'
            || currentConversation.title.trim() === '新会话';
          if (isPlaceholder) {
            setConversations((current) => current.map((conversation) => (
              conversation.id === conversationId
                ? { ...conversation, title: fallbackTitle }
                : conversation
            )));
          }
        }

        activeStreamRef.current = { agentId, conversationId };
        const run = await createMessageRun(
          conversationId,
          content,
          generationMode === 'chat' && isStreamMode,
          attachments,
          thinkingLevel,
          generationMode,
          restartFromMessageId,
          shortcutActions,
          controller.signal,
        );
        if (isFirstUserMessage && content.trim()) {
          const titleInput = attachments.length > 0
            ? `${content.trim()}\n\n${imageAttachmentTitleNote}`
            : content.trim();
          void generateConversationTitle(conversationId, titleInput, locale)
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
          const next = pendingMessagesRef.current[0];
          if (next) {
            const remaining = pendingMessagesRef.current.slice(1);
            pendingMessagesRef.current = remaining;
            setPendingMessages(remaining);
            window.queueMicrotask(() => {
              void sendRef.current?.(
                next.content,
                next.agentId,
                next.isStreamMode,
                next.attachments,
                next.thinkingLevel,
                next.generationMode,
                undefined,
                next.shortcutActions,
              );
            });
          }
        }
      }
    },
    [
      activeConversationId,
      followRun,
      imageAttachmentTitleNote,
      imageOnlyConversationTitle,
      locale,
      newConversationTitle,
      runInterruptedMessage,
    ],
  );

  sendRef.current = send;

  const appendOptimisticSteerMessage = useCallback((content: string, attachments: ImageAttachment[] = []) => {
    const clientId = genClientId();
    const stream = activeStreamRef.current;
    const message: UiMessage = {
      id: '',
      agent_id: stream.agentId || activeAgentIdRef.current,
      conversation_id: stream.conversationId || activeConversationIdRef.current,
      role: 'user',
      content,
      attachments,
      status: 'done',
      created_at: new Date().toISOString(),
      clientId,
    };
    setMessages((current) => [...current, message]);
    return clientId;
  }, []);

  const appendContextCompactionEvent = useCallback((
    conversationId: string,
    event: ContextCompactionEvent,
  ) => {
    setMessages((current) => {
      if (current.some((item) => (
        item.conversation_id === conversationId
        && item.system_event?.type === 'context_compacted'
        && item.system_event.created_at === event.created_at
      ))) {
        return current;
      }
      const last = current[current.length - 1];
      return [...current, {
        id: '',
        agent_id: last?.agent_id ?? activeStreamRef.current.agentId,
        conversation_id: conversationId,
        role: 'assistant' as const,
        content: '',
        status: 'done' as const,
        created_at: event.created_at,
        clientId: genClientId(),
        system_event: { type: 'context_compacted' as const, ...event },
      }];
    });
  }, []);

  const removeOptimisticMessage = useCallback((clientId: string) => {
    setMessages((current) => current.filter((message) => message.clientId !== clientId));
  }, []);

  const enqueueMessage = useCallback((
    content: string,
    agentId: string,
    isStreamMode: boolean,
    attachments: ImageAttachment[],
    thinkingLevel: ThinkingLevel,
    generationMode: GenerationMode,
    shortcutActions: ChatShortcutAction[] = [],
  ) => {
    const message: PendingChatMessage = {
      id: genClientId(),
      content,
      attachments,
      agentId,
      isStreamMode,
      thinkingLevel,
      generationMode,
      shortcutActions,
    };
    setPendingMessages((current) => {
      const next = [...current, message];
      pendingMessagesRef.current = next;
      return next;
    });
  }, []);

  const removePendingMessage = useCallback((id: string) => {
    setPendingMessages((current) => {
      const next = current.filter((message) => message.id !== id);
      pendingMessagesRef.current = next;
      return next;
    });
  }, []);

  const movePendingMessage = useCallback((id: string, direction: PendingMessageMoveDirection) => {
    setPendingMessages((current) => {
      const index = current.findIndex((message) => message.id === id);
      const targetIndex = direction === 'up' ? index - 1 : index + 1;
      if (index < 0 || targetIndex < 0 || targetIndex >= current.length) return current;
      const next = [...current];
      [next[index], next[targetIndex]] = [next[targetIndex], next[index]];
      pendingMessagesRef.current = next;
      return next;
    });
  }, []);

  const steerActiveRun = useCallback(async (content: string) => {
    const runId = activeRunIdRef.current;
    if (!runId || !content.trim()) return;
    const trimmedContent = content.trim();
    const optimisticClientId = appendOptimisticSteerMessage(trimmedContent);
    try {
      await steerMessageRun(runId, trimmedContent);
    } catch (cause) {
      removeOptimisticMessage(optimisticClientId);
      onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to guide the active run');
    }
  }, [appendOptimisticSteerMessage, removeOptimisticMessage]);

  const steerPendingMessage = useCallback(async (id: string) => {
    const message = pendingMessagesRef.current.find((item) => item.id === id);
    const runId = activeRunIdRef.current;
    if (!message || !runId) return;
    const pendingIndex = pendingMessagesRef.current.findIndex((item) => item.id === id);
    const remaining = pendingMessagesRef.current.filter((item) => item.id !== id);
    pendingMessagesRef.current = remaining;
    setPendingMessages(remaining);
    const optimisticClientId = appendOptimisticSteerMessage(message.content, message.attachments);
    try {
      await steerMessageRun(runId, message.content.trim());
    } catch (cause) {
      removeOptimisticMessage(optimisticClientId);
      setPendingMessages((current) => {
        if (current.some((item) => item.id === message.id)) return current;
        const next = [...current];
        next.splice(Math.min(Math.max(pendingIndex, 0), next.length), 0, message);
        pendingMessagesRef.current = next;
        return next;
      });
      onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to guide the active run');
    }
  }, [appendOptimisticSteerMessage, removeOptimisticMessage]);

  const createNewConversation = useCallback(
    async (agentId: string) => {
      // A just-started run can still look empty until its first message is persisted.
      // Do not reuse that conversation when the user explicitly starts another one.
      const existing = conversations.find((conversation) => (
        isEmptyConversation(conversation)
        && (!isSending || conversation.id !== activeConversationId)
      ));
      if (existing) {
        if (existing.id !== activeConversationId) selectConversation(existing.id);
        return existing;
      }
      const created = await createConversation(agentId, { title: newConversationTitle });
      setConversations((current) => [created, ...current]);
      selectConversation(created.id);
      return created;
    },
    [
      activeConversationId,
      conversations,
      isSending,
      newConversationTitle,
      selectConversation,
    ],
  );

  const removeConversation = useCallback(
    async (conversationId: string, agentId: string) => {
      await deleteConversation(conversationId);
      delete conversationSnapshotsRef.current[conversationId];
      delete conversationRunSnapshotsRef.current[conversationId];
      writeJsonStorage(CONVERSATION_SNAPSHOTS_STORAGE_KEY, conversationSnapshotsRef.current);
      writeJsonStorage(CONVERSATION_RUN_SNAPSHOTS_STORAGE_KEY, conversationRunSnapshotsRef.current);
      updateUnreadConversationIds((current) => ({
        ...current,
        [agentId]: (current[agentId] ?? []).filter((id) => id !== conversationId),
      }));
      if (conversationId === activeConversationId) {
        setActiveConversationId('');
        setMessages([]);
        setConversationUsage(EMPTY_USAGE);
        setHasOlderMessages(false);
        olderMessagesCursorRef.current = '';
      }
      await loadConversationsForAgent(agentId);
    },
    [activeConversationId, loadConversationsForAgent, updateUnreadConversationIds],
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
    unreadConversationIdsByAgent,
    refreshUnreadState,
    markConversationRead,
    loadConversationsForAgent,
    refreshRunningConversations,
    loadMessages,
    appendContextCompactionEvent,
    loadOlderMessages,
    resumeActiveRun,
    detachActiveRun,
    cancelActiveSend,
    pendingMessages,
    enqueueMessage,
    movePendingMessage,
    removePendingMessage,
    steerActiveRun,
    steerPendingMessage,
    clearChat,
    send,
    createNewConversation,
    removeConversation,
    setConversationPinned,
  };
}
