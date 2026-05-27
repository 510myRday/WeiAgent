import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Bot,
  Check,
  Clock,
  Globe,
  MessageSquare,
  Plus,
  RefreshCw,
  Save,
  Send,
  Settings,
  Trash2,
  X,
} from 'lucide-react';
import {
  createAgent,
  deleteAgent,
  listAgents,
  listModels,
  readChat,
  sendMessage,
  streamMessage,
  updateAgent,
} from './api';
import { useI18n } from './i18n';
import type { Agent, ChatMessage, CreateAgentPayload, StreamEvent, UiMessage } from './types';

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  system_prompt: 'You are a helpful AI assistant.',
  model: '',
};

export function App() {
  const { t, locale, setLocale } = useI18n();
  const [agents, setAgents] = useState<Agent[]>([]);
  const [activeAgentId, setActiveAgentId] = useState<string>('');
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [isStreamMode, setIsStreamMode] = useState(true);
  const [isLoadingAgents, setIsLoadingAgents] = useState(true);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState('');
  const [modelOptions, setModelOptions] = useState<string[]>([]);
  const [modelLoadError, setModelLoadError] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(window.innerWidth > 920);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [isDeletingAgent, setIsDeletingAgent] = useState(false);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [activeNav, setActiveNav] = useState<'chat' | 'agents' | 'history'>('chat');
  const [historyAgentId, setHistoryAgentId] = useState('');
  const [modelCustom, setModelCustom] = useState(false);
  const loadChatIdRef = useRef(0);
  const sendRequestIdRef = useRef(0);
  const activeAgentIdRef = useRef('');
  const syncedAgentIdRef = useRef('');
  const sendAbortControllerRef = useRef<AbortController | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  const activeAgent = useMemo(
    () => agents.find((agent) => agent.id === activeAgentId),
    [agents, activeAgentId],
  );

  useEffect(() => {
    activeAgentIdRef.current = activeAgentId;
  }, [activeAgentId]);

  useEffect(() => {
    void refreshAgents();
    void refreshModels();
  }, []);

  useEffect(() => {
    if (!activeAgentId) {
      setMessages([]);
      return;
    }
    void loadChat(activeAgentId);
  }, [activeAgentId]);

  useEffect(() => {
    if (!activeAgent) {
      syncedAgentIdRef.current = '';
      if (agentFormDirty) return;
      setAgentForm({ ...defaultAgentForm, model: modelOptions[0] ?? '' });
      return;
    }
    const isSwitchingAgent = syncedAgentIdRef.current !== activeAgent.id;
    if (agentFormDirty && !isSwitchingAgent) return;
    setAgentForm({
      name: activeAgent.name,
      system_prompt: activeAgent.system_prompt,
      model: activeAgent.model,
    });
    setAgentFormDirty(false);
    setModelCustom(modelOptions.length > 0 && !modelOptions.includes(activeAgent.model));
    syncedAgentIdRef.current = activeAgent.id;
  }, [activeAgent, agentFormDirty, modelOptions]);

  useEffect(() => {
    if (!activeAgent && !agentFormDirty) {
      setAgentForm((prev) => {
        if (prev.model.trim()) return prev;
        return { ...prev, model: modelOptions[0] ?? '' };
      });
    }
  }, [modelOptions]);

  function cancelActiveSend() {
    ++sendRequestIdRef.current;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    setIsSending(false);
  }

  function invalidateChatLoad() {
    ++loadChatIdRef.current;
    setIsLoadingChat(false);
    setMessages([]);
  }

  async function refreshAgents() {
    setIsLoadingAgents(true);
    setError('');
    try {
      const nextAgents = await listAgents();
      const hasCurrentAgent = Boolean(
        activeAgentId && nextAgents.some((agent) => agent.id === activeAgentId),
      );
      setAgents(nextAgents);
      if (hasCurrentAgent) {
        return;
      }
      cancelActiveSend();
      invalidateChatLoad();
      const nextId = nextAgents[0]?.id ?? '';
      activeAgentIdRef.current = nextId;
      setActiveAgentId(nextId);
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsLoadingAgents(false);
    }
  }

  async function refreshModels() {
    setModelLoadError(false);
    try {
      setModelOptions(await listModels());
    } catch {
      setModelOptions([]);
      setModelLoadError(true);
    }
  }

  async function loadChat(agentId: string) {
    const requestId = ++loadChatIdRef.current;
    setMessages([]);
    setIsLoadingChat(true);
    setError('');
    try {
      const raw = await readChat(agentId);
      if (requestId !== loadChatIdRef.current) return;
      setMessages(raw.map(toUiMessage));
      requestAnimationFrame(() => scrollToBottom(true));
    } catch (cause) {
      if (requestId !== loadChatIdRef.current) return;
      setError(toErrorMessage(cause));
    } finally {
      if (requestId === loadChatIdRef.current) setIsLoadingChat(false);
    }
  }

  function switchToAgent(id: string) {
    if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return;
    cancelActiveSend();
    invalidateChatLoad();
    activeAgentIdRef.current = id;
    setActiveAgentId(id);
  }

  async function handleAgentSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = agentForm.name.trim();
    const model = agentForm.model.trim();
    const system_prompt = agentForm.system_prompt.trim();
    if (!name || !model || !system_prompt) {
      setError(t('error.validation'));
      return;
    }
    setError('');
    setIsSubmittingAgent(true);
    try {
      if (activeAgent) {
        const updated = await updateAgent(activeAgent.id, { name, model, system_prompt });
        setAgents((current) => current.map((agent) => (agent.id === updated.id ? updated : agent)));
        setAgentFormDirty(false);
        return;
      }
      const created = await createAgent({ name, model, system_prompt });
      setAgents((current) => [...current, created]);
      invalidateChatLoad();
      activeAgentIdRef.current = created.id;
      setActiveAgentId(created.id);
      setAgentFormDirty(false);
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsSubmittingAgent(false);
    }
  }

  async function handleDeleteAgent() {
    if (!activeAgent) return;
    const msg = t('deleteConfirm.message').replace('{name}', activeAgent.name);
    if (!window.confirm(msg)) return;
    setError('');
    cancelActiveSend();
    setIsDeletingAgent(true);
    try {
      await deleteAgent(activeAgent.id);
      invalidateChatLoad();
      const remaining = agents.filter((agent) => agent.id !== activeAgent.id);
      setAgents(remaining);
      const nextId = remaining[0]?.id ?? '';
      activeAgentIdRef.current = nextId;
      setActiveAgentId(nextId);
      setAgentFormDirty(false);
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsDeletingAgent(false);
    }
  }

  async function handleSend(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const content = draft.trim();
    if (!activeAgent || !content || isSending) return;
    const agentId = activeAgent.id;
    const requestId = ++sendRequestIdRef.current;
    const controller = new AbortController();
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = controller;
    activeAgentIdRef.current = agentId;
    setDraft('');
    setError('');
    setIsSending(true);
    try {
      if (isStreamMode) {
        await streamMessage(agentId, content, (evt) => {
          if (requestId !== sendRequestIdRef.current) return;
          handleStreamEvent(evt);
        }, controller.signal);
      } else {
        const response = await sendMessage(agentId, content, controller.signal);
        if (requestId !== sendRequestIdRef.current) return;
        setMessages((current) => [
          ...current,
          toUiMessage(response.user),
          toUiMessage(response.assistant),
        ]);
        requestAnimationFrame(() => scrollToBottom(true));
      }
    } catch (cause) {
      if (requestId === sendRequestIdRef.current && !isAbortError(cause)) {
        setError(toErrorMessage(cause));
      }
    } finally {
      if (requestId === sendRequestIdRef.current) {
        sendAbortControllerRef.current = null;
        setIsSending(false);
      }
    }
  }

  function handleStreamEvent(event: StreamEvent) {
    if (event.event === 'error') {
      setError(event.data);
      setIsSending(false);
      return;
    }

    if (event.event === 'done') {
      setIsSending(false);
      return;
    }

    if (event.event === 'user') {
      setMessages((current) => [...current, toUiMessage(event.data)]);
      requestAnimationFrame(() => scrollToBottom(true));
      return;
    }

    if (event.event === 'delta') {
      const shouldStickToBottom = isMessagesNearBottom();
      setMessages((current) => {
        const last = current[current.length - 1];
        if (last?.role === 'assistant') {
          return [
            ...current.slice(0, -1),
            { ...last, content: last.content + event.data },
          ];
        }
        return [
          ...current,
          {
            role: 'assistant',
            content: event.data,
            created_at: new Date().toISOString(),
            clientId: genClientId(),
          },
        ];
      });
      requestAnimationFrame(() => scrollToBottom(shouldStickToBottom));
      return;
    }

    if (event.event === 'assistant') {
      const shouldStickToBottom = isMessagesNearBottom();
      setMessages((current) => {
        const last = current[current.length - 1];
        const msg = toUiMessage(event.data);
        if (last?.role === 'assistant') {
          return [...current.slice(0, -1), { ...msg, clientId: last.clientId }];
        }
        return [...current, msg];
      });
      requestAnimationFrame(() => scrollToBottom(shouldStickToBottom));
    }
  }

  const messagesContainerRef = useRef<HTMLDivElement>(null);

  const isMessagesNearBottom = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  }, []);

  const scrollToBottom = useCallback((force = false) => {
    if (!force && !isMessagesNearBottom()) return;
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    messagesEndRef.current?.scrollIntoView({ behavior: reduced ? 'auto' : 'smooth' });
  }, [isMessagesNearBottom]);

  useEffect(() => {
    scrollToBottom();
  }, [messages, scrollToBottom]);

  return (
    <main className="workspace-shell">
      <button className="sidebar-toggle" type="button" onClick={() => setSidebarOpen((v) => !v)} aria-label={sidebarOpen ? t('mobile.hideSidebar') : t('mobile.showSidebar')}>
        <MessageSquare size={20} />
      </button>
      <aside className={`sidebar${sidebarOpen ? ' open' : ''}`} aria-label={t('sidebar.ariaLabel')}>
        <div className="brand-row">
          <div className="brand-mark" aria-hidden="true">
            <Bot size={22} />
          </div>
          <div>
            <h1>{t('brand.name')}</h1>
            <p>{t('brand.tagline')}</p>
          </div>
        </div>

        <nav className="sidebar-nav">
          <button className={'nav-item' + (activeNav === 'chat' ? ' active' : '')} onClick={() => { if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return; setActiveNav('chat'); }}>
            <MessageSquare size={18} />
            <span>{t('nav.chat')}</span>
          </button>
          <button className={'nav-item' + (activeNav === 'agents' ? ' active' : '')} onClick={() => { if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return; setActiveNav('agents'); }}>
            <Settings size={18} />
            <span>{t('nav.agents')}</span>
          </button>
          <button className={'nav-item' + (activeNav === 'history' ? ' active' : '')} onClick={() => { if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return; setActiveNav('history'); }}>
            <Clock size={18} />
            <span>{t('nav.history')}</span>
          </button>
        </nav>

        <div className="sidebar-spacer" />

        <div className="sidebar-footer">
          <button
            className="lang-switch"
            type="button"
            onClick={() => setLocale(locale === 'zh' ? 'en' : 'zh')}
            aria-label={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}
            title={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}
          >
            <Globe size={17} />
            <span>{locale === 'zh' ? 'EN' : '中'}</span>
          </button>
        </div>

      </aside>

      {activeNav === 'chat' && (
        <section className="chat-area" aria-label={t('chat.ariaLabel')}>
          <header className="chat-header">
            <div className="chat-header-left">
              <select
                className="agent-select"
                value={activeAgentId}
                onChange={(e) => switchToAgent(e.target.value)}
              >
                <option value="">{t('chat.selectAgent')}</option>
                {agents.map((a) => (
                  <option key={a.id} value={a.id}>{a.name}</option>
                ))}
              </select>
              <label className="mode-toggle">
                <input
                  type="checkbox"
                  checked={isStreamMode}
                  onChange={(e) => setIsStreamMode(e.target.checked)}
                />
                <span>{isStreamMode ? t('chat.stream') : t('chat.complete')}</span>
                <Check size={16} />
              </label>
            </div>
          </header>

          {error ? (
            <div className="error-banner">
              <span>{error}</span>
              <button className="error-dismiss" type="button" onClick={() => setError('')} aria-label={t('error.dismiss')}>&times;</button>
            </div>
          ) : null}

          <div className="messages" aria-live="polite" ref={messagesContainerRef}>
            {isLoadingChat ? <p className="muted centered">{t('chat.loading')}</p> : null}
            {!isLoadingChat && messages.length === 0 ? (
              <div className="empty-chat">
                <Bot size={34} />
                <h3>{t('chat.emptyTitle')}</h3>
                <p>{t('chat.emptyHint')}</p>
              </div>
            ) : null}
            {messages.map((message) => (
              <article className={'message ' + message.role} key={message.clientId}>
                <div className="message-meta">
                  <strong>{message.role === 'user' ? t('chat.you') : activeAgent?.name ?? 'Assistant'}</strong>
                  <time dateTime={message.created_at}>{formatTime(message.created_at, locale)}</time>
                </div>
                <p>{message.content}</p>
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
      )}

      {activeNav === 'agents' && (
        <section className="agents-panel">
          <div className="agents-panel-sidebar">
            <div className="panel-header">
              <h2>{t('nav.agents')}</h2>
              <button className="icon-button" type="button" onClick={() => { if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return; setActiveAgentId(''); setAgentForm({ ...defaultAgentForm, model: modelOptions[0] ?? '' }); setAgentFormDirty(false); }}>
                <Plus size={17} />
              </button>
            </div>
            <div className="agent-list">
              {isLoadingAgents && agents.length === 0 ? <p className="muted">{t('chat.loading')}</p> : null}
              {!isLoadingAgents && agents.length === 0 ? (
                <p className="muted">{t('agentConfig.noAgents')}</p>
              ) : null}
              {agents.map((agent) => (
                <button
                  className={agent.id === activeAgentId ? 'agent-item active' : 'agent-item'}
                  key={agent.id}
                  type="button"
                  onClick={() => { if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return; setActiveAgentId(agent.id); }}
                >
                  <Bot size={17} />
                  <span>
                    <strong>{agent.name}</strong>
                    <small>{agent.model}</small>
                  </span>
                </button>
              ))}
            </div>
          </div>
          <div className="agents-panel-main">
            {activeAgent ? (
              <>
                <div className="panel-header">
                  <h2>{activeAgent.name}</h2>
                  <button className="icon-button danger" type="button" onClick={handleDeleteAgent} disabled={isDeletingAgent}>
                    <Trash2 size={17} />
                  </button>
                </div>
                <form className="agent-form" onSubmit={handleAgentSubmit}>
                  <label htmlFor="agent-name">{t('agentConfig.name')}</label>
                  <input
                    id="agent-name"
                    value={agentForm.name}
                    onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                    placeholder={t('agentConfig.placeholder_name')}
                    disabled={isSubmittingAgent}
                  />
                  <label htmlFor="agent-model">{t('agentConfig.model')}</label>
                  {modelCustom ? (
                    <input
                      id="agent-model"
                      value={agentForm.model}
                      onChange={(e) => { setAgentForm({ ...agentForm, model: e.target.value }); setAgentFormDirty(true); }}
                      placeholder={t('agentConfig.placeholder_model')}
                      disabled={isSubmittingAgent}
                    />
                  ) : (
                    <select
                      id="agent-model"
                      className="model-select"
                      value={agentForm.model}
                      onChange={(e) => {
                        if (e.target.value === '__custom__') { setModelCustom(true); return; }
                        setAgentForm({ ...agentForm, model: e.target.value });
                        setAgentFormDirty(true);
                      }}
                      disabled={isSubmittingAgent}
                    >
                      <option value="">{t('agentConfig.placeholder_model')}</option>
                      {modelOptions.map((m) => (<option key={m} value={m}>{m}</option>))}
                      <option value="__custom__">Other (manual input)</option>
                    </select>
                  )}
                  {modelCustom ? (
                    <button type="button" className="text-button" onClick={() => setModelCustom(false)}>Choose from list</button>
                  ) : null}
                  {modelLoadError ? <p className="muted model-error">{t('model.loadFailed')}</p> : null}
                  <label htmlFor="agent-system-prompt">{t('agentConfig.systemPrompt')}</label>
                  <textarea
                    id="agent-system-prompt"
                    value={agentForm.system_prompt}
                    onChange={(e) => { setAgentForm({ ...agentForm, system_prompt: e.target.value }); setAgentFormDirty(true); }}
                    rows={6}
                    disabled={isSubmittingAgent}
                  />
                  <div className="agent-form-actions">
                    <button className="primary-button" type="submit" disabled={isSubmittingAgent}>
                      <Save size={17} />
                      {t('agentConfig.save')}
                    </button>
                  </div>
                </form>
              </>
            ) : (
              <>
                <div className="panel-header">
                  <h2>{t('agentConfig.newTitle')}</h2>
                </div>
                <form className="agent-form" onSubmit={handleAgentSubmit}>
                  <label htmlFor="agent-name">{t('agentConfig.name')}</label>
                  <input
                    id="agent-name"
                    value={agentForm.name}
                    onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                    placeholder={t('agentConfig.placeholder_name')}
                    disabled={isSubmittingAgent}
                  />
                  <label htmlFor="agent-model">{t('agentConfig.model')}</label>
                  {modelCustom ? (
                    <input
                      id="agent-model"
                      value={agentForm.model}
                      onChange={(e) => { setAgentForm({ ...agentForm, model: e.target.value }); setAgentFormDirty(true); }}
                      placeholder={t('agentConfig.placeholder_model')}
                      disabled={isSubmittingAgent}
                    />
                  ) : (
                    <select
                      id="agent-model"
                      className="model-select"
                      value={agentForm.model}
                      onChange={(e) => {
                        if (e.target.value === '__custom__') { setModelCustom(true); return; }
                        setAgentForm({ ...agentForm, model: e.target.value });
                        setAgentFormDirty(true);
                      }}
                      disabled={isSubmittingAgent}
                    >
                      <option value="">{t('agentConfig.placeholder_model')}</option>
                      {modelOptions.map((m) => (<option key={m} value={m}>{m}</option>))}
                      <option value="__custom__">Other (manual input)</option>
                    </select>
                  )}
                  {modelCustom ? (
                    <button type="button" className="text-button" onClick={() => setModelCustom(false)}>Choose from list</button>
                  ) : null}
                  {modelLoadError ? <p className="muted model-error">{t('model.loadFailed')}</p> : null}
                  <label htmlFor="agent-system-prompt">{t('agentConfig.systemPrompt')}</label>
                  <textarea
                    id="agent-system-prompt"
                    value={agentForm.system_prompt}
                    onChange={(e) => { setAgentForm({ ...agentForm, system_prompt: e.target.value }); setAgentFormDirty(true); }}
                    rows={6}
                    disabled={isSubmittingAgent}
                  />
                  <div className="agent-form-actions">
                    <button className="primary-button" type="submit" disabled={isSubmittingAgent}>
                      <Plus size={17} />
                      {t('agentConfig.create')}
                    </button>
                  </div>
                </form>
              </>
            )}
          </div>
        </section>
      )}

      {activeNav === 'history' && (
        <section className="history-panel">
          {historyAgentId ? (
            <>
              <div className="panel-header">
                <button className="secondary-button" type="button" onClick={() => { setHistoryAgentId(''); setMessages([]); }}>
                  <X size={16} />
                  {t('history.back')}
                </button>
              </div>
              <div className="messages" ref={messagesContainerRef}>
                {messages.length === 0 ? <p className="muted centered">{t('chat.loading')}</p> : null}
                {messages.map((msg) => (
                  <article className={'message ' + msg.role} key={msg.clientId}>
                    <div className="message-meta">
                      <strong>{msg.role === 'user' ? t('chat.you') : 'Assistant'}</strong>
                      <time dateTime={msg.created_at}>{formatTime(msg.created_at, locale)}</time>
                    </div>
                    <p>{msg.content}</p>
                  </article>
                ))}
              </div>
            </>
          ) : (
            <>
              <div className="panel-header">
                <h2>{t('history.title')}</h2>
              </div>
              <div className="history-list">
                {agents.length === 0 ? <p className="muted centered">{t('history.empty')}</p> : null}
                {agents.map((agent) => (
                  <button
                    key={agent.id}
                    className="history-card"
                    type="button"
                    onClick={async () => {
                      setHistoryAgentId(agent.id);
                      try {
                        const raw = await readChat(agent.id);
                        setMessages(raw.map(toUiMessage));
                      } catch {
                        setMessages([]);
                      }
                    }}
                  >
                    <Bot size={20} />
                    <div>
                      <strong>{agent.name}</strong>
                      <small>{agent.model}</small>
                    </div>
                  </button>
                ))}
              </div>
            </>
          )}
        </section>
      )}
    </main>
  );
}

let nextClientId = 1;
function genClientId(): string {
  return 'c' + String(nextClientId++);
}

function toUiMessage(msg: ChatMessage): UiMessage {
  return { ...msg, clientId: msg.id ?? genClientId() };
}

function toErrorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : 'Error';
}

function isAbortError(cause: unknown) {
  return cause instanceof DOMException && cause.name === 'AbortError';
}

function formatTime(value: string, locale: string) {
  return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value));
}
