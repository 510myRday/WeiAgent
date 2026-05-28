import { useRef, useEffect, useCallback, useState, FormEvent } from 'react';
import { Bot, Check, Plus, Send } from 'lucide-react';
import type { Agent } from '../types';
import { useI18n } from '../i18n';
import { useConversations } from '../hooks';

interface ChatPanelProps {
  agents: Agent[];
  activeAgentId: string;
  onAgentChange: (id: string) => void;
  onError: (message: string) => void;
}

export function ChatPanel({ agents, activeAgentId, onAgentChange, onError }: ChatPanelProps) {
  const { t, locale } = useI18n();
  const {
    conversations,
    activeConversationId,
    setActiveConversationId,
    messages,
    isLoadingChat,
    isSending,
    loadConversationsForAgent,
    loadMessages,
    cancelActiveSend,
    clearChat,
    send,
    createNewConversation,
  } = useConversations({ newConversationTitle: t('conversation.newTitle'), onError });

  const [draft, setDraft] = useState('');
  const [isStreamMode, setIsStreamMode] = useState(true);

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);

  const activeAgent = agents.find((a) => a.id === activeAgentId);

  useEffect(() => {
    cancelActiveSend();
    clearChat();
    if (!activeAgentId) return;
    void loadConversationsForAgent(activeAgentId);
  }, [activeAgentId, cancelActiveSend, clearChat, loadConversationsForAgent]);

  useEffect(() => {
    if (!activeConversationId) return;
    void loadMessages(activeConversationId);
  }, [activeConversationId, loadMessages]);

  const isMessagesNearBottom = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  }, []);

  const scrollToBottom = useCallback(
    (force = false) => {
      if (!force && !isMessagesNearBottom()) return;
      const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
      messagesEndRef.current?.scrollIntoView({ behavior: reduced ? 'auto' : 'smooth' });
    },
    [isMessagesNearBottom],
  );

  useEffect(() => {
    scrollToBottom();
  }, [messages, scrollToBottom]);

  function handleSend(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const content = draft.trim();
    if (!activeAgent || !content || isSending) return;
    setDraft('');
    onError('');
    void send(content, activeAgent.id, isStreamMode);
  }

  function handleNewConversation() {
    if (!activeAgent) return;
    if (activeConversationId && messages.length === 0) return;
    onError('');
    void createNewConversation(activeAgent.id).catch((cause) => {
      onError(cause instanceof Error ? cause.message : 'Failed to create conversation');
    });
  }

  function formatTime(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  return (
    <section className="chat-area" aria-label={t('chat.ariaLabel')}>
      <header className="chat-header">
        <div className="chat-header-left">
          <select className="agent-select" value={activeAgentId} onChange={(e) => onAgentChange(e.target.value)}>
            <option value="">{t('chat.selectAgent')}</option>
            {agents.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
          </select>
          <select className="agent-select conversation-select" value={activeConversationId} onChange={(e) => setActiveConversationId(e.target.value)} disabled={!activeAgent}>
            <option value="">{t('conversation.select')}</option>
            {conversations.map((c) => <option key={c.id} value={c.id}>{c.title}</option>)}
          </select>
          <button className="icon-button" type="button" onClick={handleNewConversation} disabled={!activeAgent} title={t('conversation.create')}>
            <Plus size={17} />
          </button>
          <label className="mode-toggle">
            <input type="checkbox" checked={isStreamMode} onChange={(e) => setIsStreamMode(e.target.checked)} />
            <span>{isStreamMode ? t('chat.stream') : t('chat.complete')}</span>
            <Check size={16} />
          </label>
        </div>
      </header>

      <div className="messages" aria-live="polite" ref={messagesContainerRef}>
        {isLoadingChat ? <p className="muted centered">{t('chat.loading')}</p> : null}
        {!isLoadingChat && messages.length === 0 ? (
          <div className="empty-chat">
            <Bot size={34} />
            <h3>{t('chat.emptyTitle')}</h3>
            <p>{activeAgent ? t('chat.emptyHint') : t('chat.noAgentHint')}</p>
          </div>
        ) : null}
        {messages.map((msg) => (
          <article className={'message ' + msg.role} key={msg.clientId}>
            <div className="message-meta">
              <strong>{msg.role === 'user' ? t('chat.you') : activeAgent?.name ?? 'Assistant'}</strong>
              <time dateTime={msg.created_at}>{formatTime(msg.created_at)}</time>
            </div>
            <p>{msg.content}</p>
          </article>
        ))}
        <div ref={messagesEndRef} />
      </div>

      <form className="composer" onSubmit={handleSend}>
        <label htmlFor="chat-input">{t('chat.messageLabel')}</label>
        <textarea
          id="chat-input"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder={activeAgent ? t('chat.placeholder') : t('chat.placeholderDisabled')}
          disabled={!activeAgent || isSending}
          rows={3}
        />
        <button className="send-button" type="submit" disabled={!activeAgent || !draft.trim() || isSending}>
          <Send size={18} />
          {isSending ? t('chat.sending') : t('chat.send')}
        </button>
      </form>
    </section>
  );
}
