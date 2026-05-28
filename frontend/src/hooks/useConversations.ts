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

type UseConversationsOptions = {
  newConversationTitle: string;
  onError?: (msg: string) => void;
};

export function useConversations({ newConversationTitle, onError }: UseConversationsOptions) {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState('');
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);

  const loadChatIdRef = useRef(0);
  const sendRequestIdRef = useRef(0);
  const sendAbortControllerRef = useRef<AbortController | null>(null);
  const pendingDeltaRef = useRef('');
  const rafIdRef = useRef(0);
  const activeStreamRef = useRef({ agentId: '', conversationId: '' });
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

  const cancelActiveSend = useCallback(() => {
    sendRequestIdRef.current += 1;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    setIsSending(false);
  }, []);

  const clearChat = useCallback(() => {
    ++loadChatIdRef.current;
    pendingDeltaRef.current = '';
    if (rafIdRef.current) {
      cancelAnimationFrame(rafIdRef.current);
      rafIdRef.current = 0;
    }
    setConversations([]);
    setActiveConversationId('');
    setMessages([]);
    setIsLoadingChat(false);
  }, []);

  const flushPendingDelta = useCallback(() => {
    const delta = pendingDeltaRef.current;
    pendingDeltaRef.current = '';
    rafIdRef.current = 0;
    if (!delta) return;
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

  const handleStreamEvent = useCallback(
    (evt: { event: string; data: unknown }) => {
      if (evt.event === 'error') {
        setIsSending(false);
        onErrorRef.current?.(typeof evt.data === 'string' ? evt.data : 'Stream error');
        return;
      }
      if (evt.event === 'done') {
        if (pendingDeltaRef.current) flushPendingDelta();
        setIsSending(false);
        return;
      }
      if (evt.event === 'user') {
        setMessages((current) => [...current, toUiMessage(evt.data as Parameters<typeof toUiMessage>[0])]);
        return;
      }
      if (evt.event === 'delta') {
        pendingDeltaRef.current += evt.data as string;
        if (!rafIdRef.current) {
          rafIdRef.current = requestAnimationFrame(() => flushPendingDelta());
        }
        return;
      }
      if (evt.event === 'assistant') {
        if (pendingDeltaRef.current) flushPendingDelta();
        const msg = toUiMessage(evt.data as Parameters<typeof toUiMessage>[0]);
        setMessages((current) => {
          const last = current[current.length - 1];
          if (last?.role === 'assistant') {
            return [...current.slice(0, -1), { ...msg, clientId: last.clientId }];
          }
          return [...current, msg];
        });
      }
    },
    [flushPendingDelta],
  );

  const send = useCallback(
    async (content: string, agentId: string, isStreamMode: boolean) => {
      const requestId = ++sendRequestIdRef.current;
      const controller = new AbortController();
      sendAbortControllerRef.current?.abort();
      sendAbortControllerRef.current = controller;
      setIsSending(true);

      try {
        let conversationId = activeConversationId;
        if (!conversationId) {
          const created = await createConversation(agentId, {
            title: content.slice(0, 40) || newConversationTitle,
          });
          setConversations((current) => [created, ...current]);
          setActiveConversationId(created.id);
          conversationId = created.id;
        }

        if (isStreamMode) {
          activeStreamRef.current = { agentId, conversationId };
          await streamMessage(conversationId, content, (evt) => {
            if (requestId !== sendRequestIdRef.current) return;
            handleStreamEvent(evt);
          }, controller.signal);
        } else {
          const response = await sendMessage(conversationId, content, controller.signal);
          if (requestId !== sendRequestIdRef.current) return;
          setMessages((current) => [
            ...current,
            toUiMessage(response.user),
            toUiMessage(response.assistant),
          ]);
          setIsSending(false);
        }
        void loadConversationsForAgent(agentId);
      } catch (cause) {
        if (requestId === sendRequestIdRef.current) {
          setIsSending(false);
          if (!isAbortError(cause)) {
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
    [activeConversationId, handleStreamEvent, loadConversationsForAgent, newConversationTitle],
  );

  const createNewConversation = useCallback(
    async (agentId: string) => {
      const newTitle = newConversationTitle;
      const existing = conversations.find(
        (c) => c.title === newTitle && c.id !== activeConversationId,
      );
      if (existing) {
        setActiveConversationId(existing.id);
        return existing;
      }
      const created = await createConversation(agentId, { title: newTitle });
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
