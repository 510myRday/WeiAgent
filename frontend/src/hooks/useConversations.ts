import { useState, useCallback, useRef } from 'react';
import type { Conversation, UiMessage } from '../types';
import {
  listConversations,
  createConversation,
  deleteConversation,
  readConversationMessages,
  sendMessage,
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

function toUiMessage(msg: { id: string; agent_id: string; conversation_id: string; role: 'user' | 'assistant'; content: string; created_at: string }): UiMessage {
  return { ...msg, clientId: msg.id ?? genClientId() };
}

function isEmptyConversation(conversation: Conversation): boolean {
  return (conversation.message_count ?? 0) === 0;
}

type UseConversationsOptions = {
  newConversationTitle: string;
  onError?: (msg: string) => void;
  timeoutSeconds: number;
};

export function useConversations({ newConversationTitle, onError, timeoutSeconds }: UseConversationsOptions) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState('');
  const [messages, setMessages] = useState<UiMessage[]>([]);
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
      setMessages(raw.map(toUiMessage));
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
      if (last?.role === 'assistant') {
        return [...current.slice(0, -1), { ...last, content: last.content + delta }];
      }
      return [
        ...current,
        {
          id: '',
          agent_id: last?.agent_id ?? activeStreamRef.current.agentId,
          conversation_id: last?.conversation_id ?? activeStreamRef.current.conversationId,
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
      const last = current[current.length - 1];
      if (last?.role === 'assistant' && last.status === 'streaming') {
        if (!last.content) return current.slice(0, -1);
        return [...current.slice(0, -1), { ...last, status: 'failed' }];
      }
      return current;
    });
    setIsSending(false);
  }, []);

  const handleStreamEvent = useCallback(
    (evt: { event: string; data: unknown }) => {
      if (evt.event === 'error') {
        setIsSending(false);
        setMessages((current) => {
          const last = current[current.length - 1];
          if (last?.role === 'assistant' && last.status === 'streaming' && !last.content) {
            return current.slice(0, -1);
          }
          return current;
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
          if (current.some((item) => item.id && item.id === msg.id)) return current;
          const optimisticIndex = current.findIndex((item) => item.role === 'user' && !item.id && item.content === msg.content);
          if (optimisticIndex >= 0) {
            return current.map((item, index) => index === optimisticIndex ? { ...msg, clientId: item.clientId } : item);
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
          if (last?.role === 'assistant') {
            return [...current.slice(0, -1), { ...last, id: msg.id || last.id, content: msg.content || last.content, status: msg.status ?? last.status }];
          }
          return [...current, msg];
        });
      }
    },
    [appendDelta],
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
            msg.clientId === optimisticUserClientId || msg.clientId === optimisticAssistantClientId
              ? { ...msg, conversation_id: conversationId }
              : msg
          )));
        }

        if (isStreamMode) {
          activeStreamRef.current = { agentId, conversationId };
          await streamMessage(conversationId, content, (evt) => {
            if (requestId !== sendRequestIdRef.current) return;
            handleStreamEvent(evt);
          }, timeoutSeconds, controller.signal);
        } else {
          const response = await sendMessage(conversationId, content, timeoutSeconds, controller.signal);
          if (requestId !== sendRequestIdRef.current) return;
          setMessages((current) => current.map((msg) => {
            if (msg.clientId === optimisticUserClientId) {
              return { ...toUiMessage(response.user), clientId: optimisticUserClientId };
            }
            if (msg.clientId === optimisticAssistantClientId) {
              return { ...toUiMessage(response.assistant), clientId: optimisticAssistantClientId };
            }
            return msg;
          }));
          setIsSending(false);
        }
        void loadConversationsForAgent(agentId);
      } catch (cause) {
        if (requestId === sendRequestIdRef.current) {
          setIsSending(false);
          if (!isAbortError(cause)) {
            setMessages((current) => {
              const last = current[current.length - 1];
              if (last?.clientId === optimisticAssistantClientId) {
                return [...current.slice(0, -1), { ...last, status: 'failed' }];
              }
              return current;
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
    [activeConversationId, handleStreamEvent, loadConversationsForAgent, newConversationTitle, timeoutSeconds],
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
