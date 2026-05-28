import { useRef, useEffect, useCallback, useState, FormEvent } from 'react';
import { ArrowLeft, Bot, Check, MessageSquare, Plus, Search, Send, Trash2 } from 'lucide-react';
import type { Agent, Conversation } from '../types';
import { useI18n } from '../i18n';
import { useConversations } from '../hooks';
import { AvatarBadge } from './AvatarPicker';

interface ChatPanelProps {
  agents: Agent[];
  activeAgentId: string;
  onAgentChange: (id: string) => void;
  onError: (message: string) => void;
  onOpenMain: () => void;
  onBackToList: () => void;
  onBackToRail: () => void;
}

export function ChatPanel({
  agents,
  activeAgentId,
  onAgentChange,
  onError,
  onOpenMain,
  onBackToList,
  onBackToRail,
}: ChatPanelProps) {
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
    removeConversation,
  } = useConversations({ newConversationTitle: t('conversation.newTitle'), onError });

  const [draft, setDraft] = useState('');
  const [isStreamMode, setIsStreamMode] = useState(true);
  const [conversationFilter, setConversationFilter] = useState('');

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const activeConversation = conversations.find((c) => c.id === activeConversationId);
  const filteredConversations = conversations.filter((conversation) =>
    conversation.title.toLowerCase().includes(conversationFilter.trim().toLowerCase()),
  );

  useEffect(() => {
    cancelActiveSend();
    clearChat();
    setConversationFilter('');
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

  function handleAgentSelect(agentId: string) {
    onError('');
    onAgentChange(agentId);
  }

  function handleConversationSelect(conversationId: string) {
    onError('');
    setActiveConversationId(conversationId);
    onOpenMain();
  }

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
    if (activeConversationId && messages.length === 0) {
      onOpenMain();
      return;
    }
    onError('');
    void createNewConversation(activeAgent.id)
      .then(() => onOpenMain())
      .catch((cause) => {
        onError(cause instanceof Error ? cause.message : 'Failed to create conversation');
      });
  }

  async function handleDeleteConversation(conversationId: string) {
    if (!activeAgent) return;
    if (!window.confirm(t('conversation.deleteConfirm'))) return;
    try {
      await removeConversation(conversationId, activeAgent.id);
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : 'Failed to delete conversation');
    }
  }

  function formatTime(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  function formatDate(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  return (
    <>
      <section className="context-pane chat-list-pane" aria-label={t('chat.selectAgent')}>
        {!activeAgent ? (
          <>
            <header className="pane-header mobile-pane-header">
              <button className="back-button" type="button" onClick={onBackToRail}>{t('common.back')}</button>
              <div>
                <h2>{t('nav.chat')}</h2>
                <p>{t('chat.selectAgent')}</p>
              </div>
            </header>
            <div className="list-stack">
              {agents.length === 0 ? <p className="muted centered">{t('agentConfig.noAgents')}</p> : null}
              {agents.map((agent) => (
                <button
                  className="list-row"
                  key={agent.id}
                  type="button"
                  onClick={() => handleAgentSelect(agent.id)}
                >
                  <AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} />
                  <span className="row-main">
                    <strong>{agent.name}</strong>
                    <small>{agent.is_builtin ? agent.display_label ?? agent.model_id : agent.model_id}</small>
                  </span>
                </button>
              ))}
            </div>
          </>
        ) : (
          <>
            <header className="pane-header conversation-pane-header">
              <button
                className="icon-button light"
                type="button"
                onClick={() => onAgentChange('')}
                title={t('chat.selectAgent')}
                aria-label={t('chat.selectAgent')}
              >
                <ArrowLeft size={17} />
              </button>
              <div>
                <h2>{activeAgent.name}</h2>
                <p>{activeAgent.model_id}</p>
              </div>
              <button className="icon-button light" type="button" onClick={handleNewConversation} title={t('conversation.create')} aria-label={t('conversation.create')}>
                <Plus size={17} />
              </button>
            </header>
            <label className="search-box">
              <Search size={16} />
              <input
                value={conversationFilter}
                onChange={(e) => setConversationFilter(e.target.value)}
                placeholder={t('conversation.search')}
              />
            </label>
            <div className="list-stack conversation-list">
              {conversations.length === 0 ? <p className="muted centered">{t('conversation.empty')}</p> : null}
              {filteredConversations.map((conversation) => (
                <ConversationRow
                  key={conversation.id}
                  conversation={conversation}
                  active={conversation.id === activeConversationId}
                  formatDate={formatDate}
                  deleteLabel={t('conversation.delete')}
                  onSelect={() => handleConversationSelect(conversation.id)}
                  onDelete={() => handleDeleteConversation(conversation.id)}
                />
              ))}
            </div>
          </>
        )}
      </section>

      <section className="main-pane chat-area" aria-label={t('chat.ariaLabel')}>
        <header className="chat-header">
          <button className="back-button main-back-button" type="button" onClick={onBackToList}>
            {t('common.back')}
          </button>
          <div className="chat-title-block">
            <h2>{activeConversation?.title ?? activeAgent?.name ?? t('chat.noAgent')}</h2>
            <span>{activeAgent ? activeAgent.model_id : t('chat.noAgentHint')}</span>
          </div>
          <label className="mode-toggle">
            <input type="checkbox" checked={isStreamMode} onChange={(e) => setIsStreamMode(e.target.checked)} />
            <span>{isStreamMode ? t('chat.stream') : t('chat.complete')}</span>
            <Check size={16} />
          </label>
        </header>

        <div className="messages" aria-live="polite" ref={messagesContainerRef}>
          {isLoadingChat ? <p className="muted centered">{t('chat.loading')}</p> : null}
          {!isLoadingChat && messages.length === 0 ? (
            <div className="empty-chat">
              {activeAgent ? <MessageSquare size={34} /> : <Bot size={34} />}
              <h3>{activeAgent ? t('chat.emptyTitle') : t('chat.noAgent')}</h3>
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
    </>
  );
}

function ConversationRow({
  conversation,
  active,
  formatDate,
  deleteLabel,
  onSelect,
  onDelete,
}: {
  conversation: Conversation;
  active: boolean;
  formatDate: (value: string) => string;
  deleteLabel: string;
  onSelect: () => void;
  onDelete: () => void;
}) {
  return (
    <div className={'list-row conversation-row' + (active ? ' active' : '')}>
      <button className="row-content" type="button" onClick={onSelect}>
        <span className="row-avatar muted-avatar"><MessageSquare size={17} /></span>
        <span className="row-main">
          <strong>{conversation.title}</strong>
          <small>{formatDate(conversation.updated_at)}</small>
        </span>
      </button>
      <span className="row-actions">
        <button className="icon-button danger light" type="button" onClick={onDelete} title={deleteLabel} aria-label={deleteLabel}>
          <Trash2 size={15} />
        </button>
      </span>
    </div>
  );
}
