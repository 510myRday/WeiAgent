import { useState, useCallback, useRef } from 'react';
import type { ChatMessage, Conversation, ToolCallMessage, ToolCallStatus, UiMessage, UiMessageItem } from '../types';
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

function rejectPendingApprovals(items: UiMessageItem[], reason: string): UiMessageItem[] {
  return items.map((item) => {
    if ('type' in item && item.type === 'tool_call' && item.status === 'pending_approval') {
      return {
        ...item,
        status: 'rejected',
        result: { reason },
      };
    }
    return item;
  });
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
  approvalDisconnectedMessage: string;
  onError?: (msg: string) => void;
  timeoutSeconds: number;
};

export function useConversations({
  newConversationTitle,
  approvalDisconnectedMessage,
  onError,
  timeoutSeconds,
}: UseConversationsOptions) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState('');
  const [messages, setMessages] = useState<UiMessageItem[]>([]);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);

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
  }, []);

  const appendDelta = useCallback((delta: string) => {
    setMessages((current) => {
      const last = current[current.length - 1];
      if (last && 'role' in last && last.role === 'assistant') {
        return [...current.slice(0, -1), { ...last, content: last.content + delta }];
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
      const next = rejectPendingApprovals(current, approvalDisconnectedMessage);
      const last = next[next.length - 1];
      if (last && 'role' in last && last.role === 'assistant' && last.status === 'streaming') {
        if (!last.content) return next.slice(0, -1);
        return [...next.slice(0, -1), { ...last, status: 'failed' }];
      }
      return next;
    });
    setIsSending(false);
  }, [approvalDisconnectedMessage]);

  const handleStreamEvent = useCallback(
    (evt: { event: string; data: unknown }) => {
      if (evt.event === 'error') {
        setIsSending(false);
        setMessages((current) => {
          const next = rejectPendingApprovals(current, approvalDisconnectedMessage);
          const last = next[next.length - 1];
          if (last && 'role' in last && last.role === 'assistant' && last.status === 'streaming' && !last.content) {
            return next.slice(0, -1);
          }
          return next;
        });
        onErrorRef.current?.(typeof evt.data === 'string' ? evt.data : 'Stream error');
        return;
      }
      if (evt.event === 'done') {
        setIsSending(false);
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
          if (last && 'role' in last && last.role === 'assistant') {
            return [...current.slice(0, -1), { ...last, id: msg.id || last.id, content: msg.content || last.content, status: msg.status ?? last.status }];
          }
          return [...current, msg];
        });
        return;
      }
      if (evt.event === 'tool_call') {
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
    [appendDelta, approvalDisconnectedMessage],
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
          if (!isAbortError(cause)) {
            setMessages((current) => {
              const next = rejectPendingApprovals(current, approvalDisconnectedMessage);
              const last = next[next.length - 1];
              if (last && 'clientId' in last && last.clientId === optimisticAssistantClientId) {
                return [...next.slice(0, -1), { ...last, status: 'failed' as const }];
              }
              return next;
            });
            onErrorRef.current?.(cause instanceof Error ? cause.message : 'Send failed');
          }
        }
      } finally {
        if (requestId === sendRequestIdRef.current) {
          sendAbortControllerRef.current = null;
          setIsSending(false);
        }
      }
    },
    [activeConversationId, approvalDisconnectedMessage, handleStreamEvent, loadConversationsForAgent, newConversationTitle, timeoutSeconds],
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
    loadConversationsForAgent,
    loadMessages,
    cancelActiveSend,
    clearChat,
    send,
    createNewConversation,
    removeConversation,
  };
}
