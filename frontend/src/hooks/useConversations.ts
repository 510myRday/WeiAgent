import { useState, useCallback, useEffect, useRef } from 'react';
import type {
  AgentRun,
  AgentRunPhase,
  ChatMessage,
  Conversation,
  ConversationUsage,
  ImageAttachment,
  ThinkingLevel,
  ToolCallMessage,
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

function interruptActiveRun(
  items: UiMessageItem[],
  reason: string,
  toolStatus: 'rejected' | 'error',
): UiMessageItem[] {
  let activeRunStart = -1;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if ('role' in item && item.role === 'user') {
      activeRunStart = index;
      break;
    }
  }
  const next = items.map((item, index) => {
    if (
      index > activeRunStart
      && 'type' in item
      && item.type === 'tool_call'
      && (item.status === 'running' || item.status === 'pending_approval')
    ) {
      return {
        ...item,
        status: item.status === 'pending_approval' ? 'rejected' as const : toolStatus,
        result: toolStatus === 'error' ? { ...item.result, message: reason } : { ...item.result, reason },
      };
    }
    return item;
  });
  let assistantIndex = -1;
  for (let index = next.length - 1; index > activeRunStart; index -= 1) {
    const item = next[index];
    if ('role' in item && item.role === 'assistant' && item.status === 'streaming') {
      assistantIndex = index;
      break;
    }
  }
  if (assistantIndex < 0) return next;
  const assistant = next[assistantIndex];
  if (!('role' in assistant)) return next;
  if (!assistant.content && !assistant.reasoning_content) {
    return next.filter((_, index) => index !== assistantIndex);
  }
  return next.map((item, index) => (
    index === assistantIndex && 'role' in item ? { ...item, status: 'failed' as const } : item
  ));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function toUiMessageItems(raw: ChatMessage[]): UiMessageItem[] {
  const items: UiMessageItem[] = [];
  const toolIndexes = new Map<string, number>();

  for (const msg of raw) {
    if (msg.role === 'assistant' && Array.isArray(msg.tool_calls) && msg.tool_calls.length > 0) {
      if (msg.content.trim() || msg.reasoning_content?.trim()) {
        items.push(toUiMessage({ ...msg, role: 'assistant', tool_calls: undefined }));
      }
      for (const call of msg.tool_calls) {
        const item: ToolCallMessage = {
          type: 'tool_call',
          call_id: call.id,
          name: call.function.name,
          arguments: parseJsonObject(call.function.arguments),
          permission_mode: call.permission_mode,
          status: 'running',
        };
        toolIndexes.set(call.id, items.length);
        items.push(item);
      }
      continue;
    }

    if (msg.role === 'tool') {
      const result = parseJsonObject(msg.content);
      const callId = msg.tool_call_id ?? msg.id;
      const index = toolIndexes.get(callId);
      if (index !== undefined) {
        const current = items[index];
        if ('type' in current && current.type === 'tool_call') {
          items[index] = { ...current, status: statusFromToolResult(result), result };
        }
      } else {
        items.push({
          type: 'tool_call',
          call_id: callId,
          name: 'tool',
          arguments: {},
          status: statusFromToolResult(result),
          result,
        });
      }
      continue;
    }

    if (msg.role === 'user' || msg.role === 'assistant') {
      items.push(toUiMessage(msg as ChatMessage & { role: 'user' | 'assistant' }));
    }
  }

  return items;
}

function isEmptyConversation(conversation: Conversation): boolean {
  return (conversation.message_count ?? 0) === 0;
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
  runCancelledMessage: string;
  runInterruptedMessage: string;
  onError?: (msg: string) => void;
};

export function useConversations({
  newConversationTitle,
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
      setRunningConversationIds(new Set(activeRuns.map((run) => run.conversation_id)));
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
      if (
        last
        && 'role' in last
        && last.role === 'assistant'
        && last.status === 'streaming'
      ) {
        return [
          ...current.slice(0, -1),
          {
            ...last,
            content: last.content + delta,
            status: 'streaming' as const,
          },
        ];
      }
      const prevUi = last && 'role' in last ? last : undefined;
      return [
        ...current,
        {
          id: '',
          agent_id: prevUi?.agent_id ?? activeStreamRef.current.agentId,
          conversation_id: prevUi?.conversation_id ?? activeStreamRef.current.conversationId,
          role: 'assistant' as const,
          content: delta,
          status: 'streaming' as const,
          created_at: new Date().toISOString(),
          clientId: genClientId(),
        },
      ];
    });
  }, []);

  const applyReasoningDelta = useCallback((delta: string) => {
    setMessages((current) => {
      const last = current[current.length - 1];
      if (
        last
        && 'role' in last
        && last.role === 'assistant'
        && last.status === 'streaming'
      ) {
        return [
          ...current.slice(0, -1),
          {
            ...last,
            reasoning_content: (last.reasoning_content ?? '') + delta,
            status: 'streaming' as const,
          },
        ];
      }
      const prevUi = last && 'role' in last ? last : undefined;
      return [
        ...current,
        {
          id: '',
          agent_id: prevUi?.agent_id ?? activeStreamRef.current.agentId,
          conversation_id: prevUi?.conversation_id ?? activeStreamRef.current.conversationId,
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
  }, [flushPendingDelta, loadConversationsForAgent, runCancelledMessage]);

  const handleStreamEvent = useCallback(
    (evt: StreamEvent) => {
      if (evt.event !== 'delta') {
        flushPendingDelta();
      }
      if (evt.event === 'error') {
        setIsSending(false);
        setRunPhase('idle');
        setMessages((current) => {
          return interruptActiveRun(current, runInterruptedMessage, 'error');
        });
        onErrorRef.current?.(typeof evt.data === 'string' ? evt.data : 'Stream error');
        return;
      }
      if (evt.event === 'done') {
        setIsSending(false);
        setRunPhase('idle');
        setMessages((current) => {
          const last = current[current.length - 1];
          if (last && 'role' in last && last.role === 'assistant' && last.status === 'streaming') {
            if (!last.content && !last.reasoning_content) {
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
        setMessages((current) => {
          return interruptActiveRun(current, runCancelledMessage, 'rejected');
        });
        return;
      }
      if (evt.event === 'phase') {
        const phase = (evt.data as { phase: Exclude<AgentRunPhase, 'idle'> }).phase;
        setRunPhase(phase);
        return;
      }
      if (evt.event === 'user') {
        const msg = toUiMessage(evt.data as Parameters<typeof toUiMessage>[0]);
        setMessages((current) => {
          if (current.some((item) => 'role' in item && item.id && item.id === msg.id)) return current;
          const optimisticIndex = current.findIndex((item) => 'role' in item && item.role === 'user' && !item.id && item.content === msg.content);
          if (optimisticIndex >= 0) {
            return current.map((item, index) => index === optimisticIndex && 'clientId' in item ? { ...msg, clientId: item.clientId } : item);
          }
          return [...current, msg];
        });
        return;
      }
      if (evt.event === 'delta') {
        appendDelta(evt.data as string);
        return;
      }
      if (evt.event === 'reasoning_delta') {
        applyReasoningDelta(evt.data as string);
        return;
      }
      if (evt.event === 'assistant') {
        const msg = toUiMessage(evt.data as Parameters<typeof toUiMessage>[0]);
        setMessages((current) => {
          const last = current[current.length - 1];
          if (msg.status === 'failed') {
            const withoutEmptyStreaming = current.filter((item, index) => {
              const isLast = index === current.length - 1;
              return !(
                isLast
                && 'role' in item
                && item.role === 'assistant'
                && item.status === 'streaming'
                && !item.content
                && !item.reasoning_content
              );
            });
            const withFailedPartial = withoutEmptyStreaming.map((item, index) => {
              const isLast = index === withoutEmptyStreaming.length - 1;
              return isLast
                && 'role' in item
                && item.role === 'assistant'
                && item.status === 'streaming'
                ? { ...item, status: 'failed' as const }
                : item;
            });
            return [...withFailedPartial, { ...msg, status: 'failed' as const }];
          }
          if (
            last
            && 'role' in last
            && last.role === 'assistant'
            && last.status === 'streaming'
          ) {
            return [
              ...current.slice(0, -1),
              {
                ...last,
                id: msg.id || last.id,
                content: msg.content || last.content,
                reasoning_content: msg.reasoning_content || last.reasoning_content,
                usage: msg.usage ?? last.usage,
                status: msg.status ?? 'done',
              },
            ];
          }
          return [...current, { ...msg, status: msg.status ?? 'done' }];
        });
        return;
      }
      if (evt.event === 'tool_call') {
        setRunPhase('executing_tool');
        const d = evt.data;
        const toolMsg: ToolCallMessage = {
          type: 'tool_call',
          call_id: d.call_id,
          name: d.name,
          arguments: d.arguments,
          permission_mode: d.permission_mode,
          status: 'running',
        };
        setMessages((current) => {
          const withoutEmptyAssistant = current.filter((item, index) => {
            const isLast = index === current.length - 1;
            return !(
              isLast
              && 'role' in item
              && item.role === 'assistant'
              && item.status === 'streaming'
              && !item.content
              && !item.reasoning_content
            );
          });
          const existingIndex = withoutEmptyAssistant.findIndex(
            (item) => 'type' in item && item.type === 'tool_call' && item.call_id === d.call_id,
          );
          if (existingIndex >= 0) {
            return withoutEmptyAssistant.map((item, index) => (
              index === existingIndex ? toolMsg : item
            ));
          }
          return [...withoutEmptyAssistant, toolMsg];
        });
        return;
      }
      if (evt.event === 'tool_result') {
        const d = evt.data;
        setMessages((current) => current.map((item) => {
          if ('type' in item && item.type === 'tool_call' && item.call_id === d.call_id) {
            return { ...item, status: d.result.status, result: d.result };
          }
          return item;
        }));
        return;
      }
      if (evt.event === 'approval_required') {
        setRunPhase('waiting_for_approval');
        const d = evt.data;
        setMessages((current) => current.map((item) => {
          if ('type' in item && item.type === 'tool_call' && item.call_id === d.call_id) {
            return { ...item, status: 'pending_approval', arguments: { ...item.arguments, ...d } };
          }
          return item;
        }));
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
    if (replayFromStart && run.user_message_id) {
      setMessages((current) => {
        const runStart = current.findIndex(
          (item) => 'role' in item && item.role === 'user' && item.id === run.user_message_id,
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
        // Message history remains usable when an older backend lacks usage data.
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
      const optimisticUserClientId = genClientId();
      const optimisticAssistantClientId = genClientId();
      sendAbortControllerRef.current?.abort();
      sendAbortControllerRef.current = controller;
      setIsSending(true);
      setRunPhase('waiting_for_model');
      setMessages((current) => {
        const restartIndex = restartFromMessageId
          ? current.findIndex(
            (item) => 'role' in item && item.role === 'user' && item.id === restartFromMessageId,
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
            title: content.slice(0, 40) || attachments[0]?.name || newConversationTitle,
          });
          setConversations((current) => [created, ...current]);
          suppressNextLoadConversationIdRef.current = created.id;
          setActiveConversationId(created.id);
          conversationId = created.id;
          setMessages((current) => current.map((msg) => (
            'clientId' in msg && (msg.clientId === optimisticUserClientId || msg.clientId === optimisticAssistantClientId)
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
    [activeConversationId, followRun, newConversationTitle, runInterruptedMessage],
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
    setConversations,
    activeConversationId,
    selectConversation,
    messages,
    setMessages,
    conversationUsage,
    hasOlderMessages,
    isLoadingOlderMessages,
    isLoadingConversations,
    conversationsError,
    isLoadingChat,
    isSending,
    runPhase,
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
