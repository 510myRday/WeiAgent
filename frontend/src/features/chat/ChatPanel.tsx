import { useRef, useEffect, useCallback, useState, isValidElement, FormEvent } from 'react';
import type { ReactNode } from 'react';
import { ArrowLeft, Bot, Check, Clipboard, MessageSquare, Plus, Send, Trash2 } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import remarkGfm from 'remark-gfm';

import type { Agent, AppSettings, Conversation, UiMessage } from '../../types';
import { useI18n } from '../../i18n';
import { useConversations } from '../../hooks';
import { AvatarBadge } from '../../components/AvatarPicker';
import { Button, EmptyState, IconButton, ListItem, PaneHeader, SearchInput } from '../../components/ui';

interface ChatPanelProps {
  agents: Agent[];
  activeAgentId: string;
  onAgentChange: (id: string) => void;
  onError: (message: string) => void;
  onOpenMain: () => void;
  onBackToList: () => void;
  settings: AppSettings;
}

export function ChatPanel({
  agents,
  activeAgentId,
  onAgentChange,
  onError,
  onOpenMain,
  onBackToList,
  settings,
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
  } = useConversations({
    newConversationTitle: t('conversation.newTitle'),
    onError,
    timeoutSeconds: settings.request_timeout_seconds,
  });

  const [draft, setDraft] = useState('');
  const [isStreamMode, setIsStreamMode] = useState(true);
  const [conversationFilter, setConversationFilter] = useState('');

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const displayAgentName = (agent: Agent) => agent.is_builtin ? t('agentConfig.builtinMasterName') : agent.name;
  const activeConversation = conversations.find((conversation) => conversation.id === activeConversationId);
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
            <PaneHeader
              className="mobile-pane-header"
              title={t('nav.chat')}
              description={t('chat.selectAgent')}
              leading={<Button className="back-button" type="button" onClick={onOpenMain}>{t('common.back')}</Button>}
            />
            <div className="list-stack">
              {agents.length === 0 ? <p className="muted centered">{t('agentConfig.noAgents')}</p> : null}
              {agents.map((agent) => (
                <ListItem
                  key={agent.id}
                  type="button"
                  title={displayAgentName(agent)}
                  subtitle={agent.model_id}
                  leading={<AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} />}
                  onClick={() => handleAgentSelect(agent.id)}
                />
              ))}
            </div>
          </>
        ) : (
          <>
            <PaneHeader
              className="conversation-pane-header"
              title={displayAgentName(activeAgent)}
              description={activeAgent.model_id}
              leading={(
                <IconButton variant="ghost" type="button" onClick={() => onAgentChange('')} label={t('chat.selectAgent')}>
                  <ArrowLeft size={17} />
                </IconButton>
              )}
              actions={(
                <IconButton variant="ghost" type="button" onClick={handleNewConversation} label={t('conversation.create')}>
                  <Plus size={17} />
                </IconButton>
              )}
            />
            <SearchInput value={conversationFilter} onChange={setConversationFilter} placeholder={t('conversation.search')} />
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
        <div className="messages" aria-live="polite" ref={messagesContainerRef}>
          {isLoadingChat ? <p className="muted centered">{t('chat.loading')}</p> : null}
          {!isLoadingChat && messages.length === 0 ? (
            <EmptyState
              icon={activeAgent ? <MessageSquare size={34} /> : <Bot size={34} />}
              title={activeAgent ? t('chat.emptyTitle') : t('chat.noAgent')}
              description={activeAgent ? t('chat.emptyHint') : t('chat.noAgentHint')}
            />
          ) : null}
          {messages.map((msg) => (
            <article className={'message ' + msg.role} key={msg.clientId}>
              <div className="message-meta">
                <strong>{msg.role === 'user' ? t('chat.you') : activeAgent ? displayAgentName(activeAgent) : 'Assistant'}</strong>
                <time dateTime={msg.created_at}>{formatTime(msg.created_at)}</time>
              </div>
              <MessageContent message={msg} />
              {msg.status === 'failed' ? <span className="message-status error">{t('chat.failed')}</span> : null}
            </article>
          ))}
          <div ref={messagesEndRef} />
        </div>

        <form className="composer" onSubmit={handleSend}>
          <div className="composer-toolbar">
            <label className="mode-toggle">
              <input type="checkbox" checked={isStreamMode} onChange={(e) => setIsStreamMode(e.target.checked)} />
              <span>{isStreamMode ? t('chat.stream') : t('chat.complete')}</span>
            </label>
            <Button className="back-button" type="button" onClick={onBackToList}>{t('common.back')}</Button>
          </div>
          <div className="composer-input-row">
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder={activeAgent ? t('chat.placeholder') : t('chat.placeholderDisabled')}
              disabled={!activeAgent || isSending}
              rows={3}
            />
            <Button className="send-button" variant="primary" type="submit" disabled={!activeAgent || !draft.trim() || isSending} icon={<Send size={18} />}>
              {isSending ? t('chat.sending') : t('chat.send')}
            </Button>
          </div>
        </form>
      </section>
    </>
  );
}

function MessageContent({ message }: { message: UiMessage }) {
  if (message.status === 'streaming') {
    return <p className="message-plain">{message.content}</p>;
  }

  return (
    <div className="message-markdown">
      <MarkdownContent content={message.content} />
    </div>
  );
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const text = extractText(children);
  const language = extractLanguage(children);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="code-block-wrap">
      <span className="code-language-label">{language}</span>
      <button className="code-copy-button" type="button" onClick={handleCopy} disabled={!text} aria-label={t('chat.copyCode')}>
        {copied ? <Check size={14} /> : <Clipboard size={14} />}
        <span>{copied ? t('chat.copied') : t('chat.copy')}</span>
      </button>
      <pre>{children}</pre>
    </div>
  );
}

function extractLanguage(node: ReactNode): string {
  const className = findCodeClassName(node);
  const raw = className?.match(/language-([\w-]+)/)?.[1] ?? 'text';
  return formatLanguage(raw);
}

function findCodeClassName(node: ReactNode): string | undefined {
  if (Array.isArray(node)) {
    return node.map(findCodeClassName).find(Boolean);
  }
  if (!isValidElement<{ className?: string; children?: ReactNode }>(node)) {
    return undefined;
  }
  return node.props.className ?? findCodeClassName(node.props.children);
}

function formatLanguage(language: string): string {
  const normalized = language.toLowerCase();
  const labels: Record<string, string> = {
    js: 'JS',
    javascript: 'JS',
    jsx: 'JSX',
    ts: 'TS',
    typescript: 'TS',
    tsx: 'TSX',
    html: 'HTML',
    css: 'CSS',
    json: 'JSON',
    md: 'MD',
    markdown: 'MD',
    py: 'Python',
    python: 'Python',
    rs: 'Rust',
    rust: 'Rust',
    sh: 'Shell',
    bash: 'Shell',
    powershell: 'PowerShell',
    ps1: 'PowerShell',
    text: 'Text',
    txt: 'Text',
  };
  return labels[normalized] ?? normalized.toUpperCase();
}

function extractText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(extractText).join('');
  if (isValidElement<{ children?: ReactNode }>(node)) return extractText(node.props.children);
  return '';
}

function MarkdownContent({ content }: { content: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[rehypeHighlight]}
      components={{ pre: CodeBlock }}
    >
      {content}
    </ReactMarkdown>
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
    <div className={['conversation-row', active ? 'is-active' : ''].filter(Boolean).join(' ')}>
      <ListItem
        className="conversation-row-main"
        type="button"
        title={conversation.title}
        subtitle={formatDate(conversation.updated_at)}
        leading={<span className="avatar-badge avatar-muted"><MessageSquare size={17} /></span>}
        onClick={onSelect}
      />
      <span className="row-actions">
        <IconButton variant="danger" type="button" onClick={onDelete} label={deleteLabel}>
          <Trash2 size={15} />
        </IconButton>
      </span>
    </div>
  );
}
