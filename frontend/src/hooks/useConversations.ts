import { useState, useCallback, useRef } from 'react';
import type {
  AgentRunPhase,
  ChatMessage,
  Conversation,
  ToolCallMessage,
  ToolCallStatus,
  UiMessage,
  UiMessageItem,
} from '../types';
import {
  listConversations,
  createConversation,
  deleteConversation,
  readConversationMessages,
  streamMessage,
} from '../api';

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

function parseJsonObject(value: string): Record<string, any> {
  try {
    const parsed = JSON.parse(value);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function statusFromToolResult(result: any): ToolCallStatus {
  const status = result?.status;
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
  if (!assistant.content) {
    return next.filter((_, index) => index !== assistantIndex);
  }
  return next.map((item, index) => (
    index === assistantIndex && 'role' in item ? { ...item, status: 'failed' as const } : item
  ));
}

function toUiMessageItems(raw: ChatMessage[]): UiMessageItem[] {
  const items: UiMessageItem[] = [];
  const toolIndexes = new Map<string, number>();

  for (const msg of raw) {
    if (msg.role === 'assistant' && Array.isArray(msg.tool_calls) && msg.tool_calls.length > 0) {
      if (msg.content.trim()) {
        items.push(toUiMessage({ ...msg, role: 'assistant', tool_calls: undefined }));
      }
      for (const call of msg.tool_calls) {
        const item: ToolCallMessage = {
          type: 'tool_call',
          call_id: call.id,
          name: call.function.name,
          arguments: parseJsonObject(call.function.arguments),
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

type UseConversationsOptions = {
  newConversationTitle: string;
  runCancelledMessage: string;
  runInterruptedMessage: string;
  onError?: (msg: string) => void;
  timeoutSeconds: number;
};

export function useConversations({
  newConversationTitle,
  runCancelledMessage,
  runInterruptedMessage,
  onError,
  timeoutSeconds,
}: UseConversationsOptions) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState('');
  const [messages, setMessages] = useState<UiMessageItem[]>([]);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [runPhase, setRunPhase] = useState<AgentRunPhase>('idle');

  const loadChatIdRef = useRef(0);
  const sendRequestIdRef = useRef(0);
  const sendAbortControllerRef = useRef<AbortController | null>(null);
  const activeStreamRef = useRef({ agentId: '', conversationId: '' });
  const suppressNextLoadConversationIdRef = useRef('');
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  const loadConversationsForAgent = useCallback(async (agentId: string) => {
    try {
      const next = await listConversations(agentId);
      setConversations(next);
      setActiveConversationId((current) =>
        current && next.some((c) => c.id === current) ? current : next[0]?.id ?? '',
      );
    } catch (cause) {
      setConversations([]);
      setActiveConversationId('');
      onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to load conversations');
    }
  }, []);

  const loadMessages = useCallback(async (conversationId: string) => {
    if (suppressNextLoadConversationIdRef.current === conversationId) {
      suppressNextLoadConversationIdRef.current = '';
      return;
    }
    const requestId = ++loadChatIdRef.current;
    setIsLoadingChat(true);
    setMessages([]);
    try {
      const raw = await readConversationMessages(conversationId);
      if (requestId !== loadChatIdRef.current) return;
      setMessages(toUiMessageItems(raw));
    } catch (cause) {
      if (requestId === loadChatIdRef.current) {
        onErrorRef.current?.(cause instanceof Error ? cause.message : 'Failed to load messages');
      }
    } finally {
      if (requestId === loadChatIdRef.current) setIsLoadingChat(false);
    }
  }, []);

  const clearChat = useCallback(() => {
    ++loadChatIdRef.current;
    setConversations([]);
    setActiveConversationId('');
    setMessages([]);
    setIsLoadingChat(false);
    setRunPhase('idle');
  }, []);

  const appendDelta = useCallback((delta: string) => {
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

  const cancelActiveSend = useCallback(() => {
    sendRequestIdRef.current += 1;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    setMessages((current) => {
      return interruptActiveRun(current, runCancelledMessage, 'rejected');
    });
    setRunPhase('idle');
    setIsSending(false);
  }, [runCancelledMessage]);

  const handleStreamEvent = useCallback(
    (evt: { event: string; data: unknown }) => {
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
            if (!last.content) {
              return current.slice(0, -1);
            }
            return [...current.slice(0, -1), { ...last, status: 'done' as const }];
          }
          return current;
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
        const d = evt.data as { call_id: string; name: string; arguments: Record<string, any> };
        const toolMsg: ToolCallMessage = {
          type: 'tool_call',
          call_id: d.call_id,
          name: d.name,
          arguments: d.arguments,
          status: 'running',
        };
        setMessages((current) => {
          const withoutEmptyAssistant = current.filter((item, index) => {
            const isLast = index === current.length - 1;
            return !(isLast && 'role' in item && item.role === 'assistant' && item.status === 'streaming' && !item.content);
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
        const d = evt.data as { call_id: string; result: any };
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
        const d = evt.data as {
          call_id: string;
          agent_id: string;
          command: string;
          approval_id: string;
          cwd?: string;
          runtime: string;
          permission: string;
          justification?: string;
          background: boolean;
        };
        setMessages((current) => current.map((item) => {
          if ('type' in item && item.type === 'tool_call' && item.call_id === d.call_id) {
            return { ...item, status: 'pending_approval', arguments: { ...item.arguments, ...d } };
          }
          return item;
        }));
        return;
      }
    },
    [appendDelta, runInterruptedMessage],
  );

  const send = useCallback(
    async (content: string, agentId: string, isStreamMode: boolean) => {
      const requestId = ++sendRequestIdRef.current;
      const controller = new AbortController();
      const optimisticUserClientId = genClientId();
      const optimisticAssistantClientId = genClientId();
      sendAbortControllerRef.current?.abort();
      sendAbortControllerRef.current = controller;
      setIsSending(true);
      setRunPhase('waiting_for_model');
      setMessages((current) => [
        ...current,
        {
          id: '',
          agent_id: agentId,
          conversation_id: activeConversationId,
          role: 'user' as const,
          content,
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
          status: 'streaming' as const,
          created_at: new Date().toISOString(),
          clientId: optimisticAssistantClientId,
        },
      ]);

      try {
        let conversationId = activeConversationId;
        if (!conversationId) {
          const created = await createConversation(agentId, {
            title: content.slice(0, 40) || newConversationTitle,
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
        await streamMessage(conversationId, content, (evt) => {
          if (requestId !== sendRequestIdRef.current) return;
          handleStreamEvent(evt);
        }, timeoutSeconds, isStreamMode, controller.signal);
        void loadConversationsForAgent(agentId);
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
    [activeConversationId, handleStreamEvent, loadConversationsForAgent, newConversationTitle, runInterruptedMessage, timeoutSeconds],
  );

  const createNewConversation = useCallback(
    async (agentId: string) => {
      const existing = conversations.find(isEmptyConversation);
      if (existing) {
        setActiveConversationId(existing.id);
        return existing;
      }
      const created = await createConversation(agentId, { title: newConversationTitle });
      setConversations((current) => [created, ...current]);
      setActiveConversationId(created.id);
      return created;
    },
    [conversations, activeConversationId, newConversationTitle],
  );

  const removeConversation = useCallback(
    async (conversationId: string, agentId: string) => {
      await deleteConversation(conversationId);
      if (conversationId === activeConversationId) {
        setActiveConversationId('');
        setMessages([]);
      }
      await loadConversationsForAgent(agentId);
    },
    [activeConversationId, loadConversationsForAgent],
  );

  return {
    conversations,
    setConversations,
    activeConversationId,
    setActiveConversationId,
    messages,
    setMessages,
    isLoadingChat,
    isSending,
    runPhase,
    loadConversationsForAgent,
    loadMessages,
    cancelActiveSend,
    clearChat,
    send,
    createNewConversation,
    removeConversation,
  };
}
