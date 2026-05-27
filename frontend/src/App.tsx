import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Bot,
  Check,
  Clock,
  Download,
  Globe,
  KeyRound,
  MessageSquare,
  Plus,
  Save,
  Send,
  Server,
  Settings,
  SlidersHorizontal,
  Trash2,
  X,
} from 'lucide-react';
import {
  createAgent,
  createConversation,
  createModelProvider,
  deleteAgent,
  deleteConversation,
  deleteModelProvider,
  fetchModels,
  listAgents,
  listConversations,
  listModelProviders,
  readConversationMessages,
  readSettings,
  sendMessage,
  streamMessage,
  updateAgent,
  updateModelProvider,
  updateSettings,
} from './api';
import { useI18n } from './i18n';
import type {
  Agent,
  AppSettings,
  ChatMessage,
  Conversation,
  CreateAgentPayload,
  ModelProvider,
  StreamEvent,
  UiMessage,
} from './types';

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  system_prompt: 'You are a helpful AI assistant.',
  model_provider_id: '',
  model_id: '',
};

const defaultSettings: AppSettings = {
  host: '',
  port: 0,
  log_level: 'info',
};

const defaultProviderForm = {
  name: '',
  base_url: 'https://api.openai.com/v1',
  api_key: '',
  modelsText: '',
};

type NavKey = 'chat' | 'agents' | 'history' | 'settings';
type SettingsSection = 'runtime' | 'providers';

export function App() {
  const { t, locale, setLocale } = useI18n();
  const [agents, setAgents] = useState<Agent[]>([]);
  const [providers, setProviders] = useState<ModelProvider[]>([]);
  const [settingsForm, setSettingsForm] = useState<AppSettings>(defaultSettings);
  const [activeAgentId, setActiveAgentId] = useState('');
  const [activeConversationId, setActiveConversationId] = useState('');
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [providerForm, setProviderForm] = useState(defaultProviderForm);
  const [activeProviderId, setActiveProviderId] = useState('');
  const [isStreamMode, setIsStreamMode] = useState(true);
  const [isLoadingAgents, setIsLoadingAgents] = useState(true);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [isSubmittingProvider, setIsSubmittingProvider] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [activeSettingsSection, setActiveSettingsSection] = useState<SettingsSection>('runtime');
  const [sidebarOpen, setSidebarOpen] = useState(window.innerWidth > 920);
  const [historyConversations, setHistoryConversations] = useState<Record<string, Conversation[]>>({});
  const [historyConversationId, setHistoryConversationId] = useState('');
  const [historyMessages, setHistoryMessages] = useState<UiMessage[]>([]);
  const [error, setError] = useState('');

  const loadChatIdRef = useRef(0);
  const sendRequestIdRef = useRef(0);
  const sendAbortControllerRef = useRef<AbortController | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);

  const activeAgent = useMemo(
    () => agents.find((agent) => agent.id === activeAgentId),
    [agents, activeAgentId],
  );
  const activeProvider = useMemo(
    () => providers.find((provider) => provider.id === activeProviderId),
    [providers, activeProviderId],
  );
  const selectedAgentProvider = useMemo(
    () => providers.find((provider) => provider.id === agentForm.model_provider_id),
    [providers, agentForm.model_provider_id],
  );

  useEffect(() => {
    void refreshAll();
  }, []);

  useEffect(() => {
    if (!activeAgentId) {
      setConversations([]);
      setActiveConversationId('');
      setMessages([]);
      return;
    }
    void loadConversationsForAgent(activeAgentId);
  }, [activeAgentId]);

  useEffect(() => {
    if (!activeConversationId) {
      setMessages([]);
      return;
    }
    void loadMessages(activeConversationId);
  }, [activeConversationId]);

  useEffect(() => {
    if (!activeAgent) {
      if (agentFormDirty) return;
      const provider = providers[0];
      setAgentForm({
        ...defaultAgentForm,
        model_provider_id: provider?.id ?? '',
        model_id: provider?.models[0] ?? '',
      });
      return;
    }
    setAgentForm({
      name: activeAgent.name,
      system_prompt: activeAgent.system_prompt,
      model_provider_id: activeAgent.model_provider_id,
      model_id: activeAgent.model_id,
    });
    setAgentFormDirty(false);
  }, [activeAgent, providers]);

  useEffect(() => {
    if (!activeProvider) {
      setProviderForm(defaultProviderForm);
      return;
    }
    setProviderForm({
      name: activeProvider.name,
      base_url: activeProvider.base_url,
      api_key: '',
      modelsText: activeProvider.models.join('\n'),
    });
  }, [activeProvider]);

  async function refreshAll() {
    setError('');
    await Promise.all([refreshSettings(), refreshProviders(), refreshAgents()]);
  }

  async function refreshSettings() {
    try {
      setSettingsForm(await readSettings());
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function refreshProviders() {
    try {
      const next = await listModelProviders();
      setProviders(next);
      setActiveProviderId((current) => (current && next.some((p) => p.id === current) ? current : next[0]?.id ?? ''));
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function refreshAgents() {
    setIsLoadingAgents(true);
    try {
      const next = await listAgents();
      setAgents(next);
      setActiveAgentId((current) => (current && next.some((a) => a.id === current) ? current : next[0]?.id ?? ''));
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsLoadingAgents(false);
    }
  }

  async function loadConversationsForAgent(agentId: string) {
    setError('');
    try {
      const next = await listConversations(agentId);
      setConversations(next);
      setActiveConversationId((current) => (current && next.some((c) => c.id === current) ? current : next[0]?.id ?? ''));
    } catch (cause) {
      setError(toErrorMessage(cause));
      setConversations([]);
      setActiveConversationId('');
    }
  }

  async function loadMessages(conversationId: string) {
    const requestId = ++loadChatIdRef.current;
    setIsLoadingChat(true);
    setMessages([]);
    try {
      const raw = await readConversationMessages(conversationId);
      if (requestId !== loadChatIdRef.current) return;
      setMessages(raw.map(toUiMessage));
      requestAnimationFrame(() => scrollToBottom(true));
    } catch (cause) {
      if (requestId === loadChatIdRef.current) setError(toErrorMessage(cause));
    } finally {
      if (requestId === loadChatIdRef.current) setIsLoadingChat(false);
    }
  }

  function cancelActiveSend() {
    ++sendRequestIdRef.current;
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = null;
    setIsSending(false);
  }

  function switchNav(next: NavKey) {
    if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return;
    setActiveNav(next);
    if (next === 'history') void refreshHistory();
  }

  function switchAgent(id: string) {
    if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return;
    cancelActiveSend();
    setActiveAgentId(id);
  }

  async function handleCreateConversation() {
    if (!activeAgent) return;
    // If the current conversation is empty, reuse it instead of creating another
    if (activeConversationId && messages.length === 0) return;
    // If another empty conversation already exists, switch to it
    const newTitle = t('conversation.newTitle');
    const existing = conversations.find((c) => c.title === newTitle && c.id !== activeConversationId);
    if (existing) {
      setActiveConversationId(existing.id);
      return;
    }
    try {
      const created = await createConversation(activeAgent.id, { title: newTitle });
      setConversations((current) => [created, ...current]);
      setActiveConversationId(created.id);
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function ensureConversation(content: string): Promise<string | null> {
    if (activeConversationId) return activeConversationId;
    if (!activeAgent) return null;
    const created = await createConversation(activeAgent.id, {
      title: content.slice(0, 40) || t('conversation.newTitle'),
    });
    setConversations((current) => [created, ...current]);
    setActiveConversationId(created.id);
    return created.id;
  }

  async function handleAgentSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const payload = {
      name: agentForm.name.trim(),
      system_prompt: agentForm.system_prompt.trim(),
      model_provider_id: agentForm.model_provider_id,
      model_id: agentForm.model_id.trim(),
    };
    if (!payload.name || !payload.system_prompt || !payload.model_provider_id || !payload.model_id) {
      setError(t('error.validation'));
      return;
    }
    setIsSubmittingAgent(true);
    setError('');
    try {
      if (activeAgent) {
        const updated = await updateAgent(activeAgent.id, payload);
        setAgents((current) => current.map((agent) => (agent.id === updated.id ? updated : agent)));
      } else {
        const created = await createAgent(payload);
        setAgents((current) => [...current, created]);
        setActiveAgentId(created.id);
      }
      setAgentFormDirty(false);
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsSubmittingAgent(false);
    }
  }

  async function handleDeleteAgent() {
    if (!activeAgent) return;
    if (!window.confirm(t('deleteConfirm.message').replace('{name}', activeAgent.name))) return;
    try {
      await deleteAgent(activeAgent.id);
      setAgents((current) => current.filter((agent) => agent.id !== activeAgent.id));
      setActiveAgentId('');
      setActiveConversationId('');
      setConversations([]);
      setMessages([]);
      setAgentFormDirty(false);
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function handleProviderSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const payload = {
      name: providerForm.name.trim(),
      base_url: providerForm.base_url.trim(),
      api_key: providerForm.api_key.trim(),
      models: parseModels(providerForm.modelsText),
    };
    if (!payload.name || !payload.base_url || (!activeProvider && !payload.api_key) || payload.models.length === 0) {
      setError(t('error.providerValidation'));
      return;
    }
    setIsSubmittingProvider(true);
    setError('');
    try {
      if (activeProvider) {
        const updated = await updateModelProvider(activeProvider.id, payload);
        setProviders((current) => current.map((provider) => (provider.id === updated.id ? updated : provider)));
      } else {
        const created = await createModelProvider(payload);
        setProviders((current) => [...current, created]);
        setActiveProviderId(created.id);
      }
      setProviderForm((current) => ({ ...current, api_key: '' }));
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsSubmittingProvider(false);
    }
  }

  async function handleDeleteProvider() {
    if (!activeProvider) return;
    if (!window.confirm(t('provider.deleteConfirm').replace('{name}', activeProvider.name))) return;
    try {
      await deleteModelProvider(activeProvider.id);
      setProviders((current) => current.filter((provider) => provider.id !== activeProvider.id));
      setActiveProviderId('');
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function handleSettingsSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    try {
      setSettingsForm(await updateSettings(settingsForm));
      setError('');
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function handleSend(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const content = draft.trim();
    if (!activeAgent || !content || isSending) return;
    const requestId = ++sendRequestIdRef.current;
    const controller = new AbortController();
    sendAbortControllerRef.current?.abort();
    sendAbortControllerRef.current = controller;
    setDraft('');
    setError('');
    setIsSending(true);
    try {
      const conversationId = await ensureConversation(content);
      if (!conversationId) return;
      if (isStreamMode) {
        await streamMessage(conversationId, content, (evt) => {
          if (requestId !== sendRequestIdRef.current) return;
          handleStreamEvent(evt, conversationId);
        }, controller.signal);
      } else {
        const response = await sendMessage(conversationId, content, controller.signal);
        if (requestId !== sendRequestIdRef.current) return;
        setMessages((current) => [
          ...current,
          toUiMessage(response.user),
          toUiMessage(response.assistant),
        ]);
        requestAnimationFrame(() => scrollToBottom(true));
      }
      if (activeAgent) void loadConversationsForAgent(activeAgent.id);
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

  function handleStreamEvent(event: StreamEvent, conversationId: string) {
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
          return [...current.slice(0, -1), { ...last, content: last.content + event.data }];
        }
        return [
          ...current,
          {
            id: genClientId(),
            agent_id: activeAgentId,
            conversation_id: conversationId,
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
      setMessages((current) => {
        const last = current[current.length - 1];
        const msg = toUiMessage(event.data);
        if (last?.role === 'assistant') {
          return [...current.slice(0, -1), { ...msg, clientId: last.clientId }];
        }
        return [...current, msg];
      });
    }
  }

  async function refreshHistory() {
    const entries = await Promise.all(
      agents.map(async (agent) => [agent.id, await listConversations(agent.id)] as const),
    );
    setHistoryConversations(Object.fromEntries(entries));
  }

  async function openHistoryConversation(conversationId: string) {
    setHistoryConversationId(conversationId);
    try {
      const raw = await readConversationMessages(conversationId);
      setHistoryMessages(raw.map(toUiMessage));
    } catch {
      setHistoryMessages([]);
    }
  }

  async function handleDeleteConversation(conversationId: string) {
    if (!window.confirm(t('conversation.deleteConfirm'))) return;
    try {
      await deleteConversation(conversationId);
      if (conversationId === activeConversationId) {
        setActiveConversationId('');
        setMessages([]);
      }
      if (activeAgent) await loadConversationsForAgent(activeAgent.id);
      await refreshHistory();
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

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
          <div className="brand-mark" aria-hidden="true"><Bot size={22} /></div>
          <div>
            <h1>{t('brand.name')}</h1>
            <p>{t('brand.tagline')}</p>
          </div>
        </div>

        <nav className="sidebar-nav">
          <NavButton active={activeNav === 'chat'} onClick={() => switchNav('chat')} icon={<MessageSquare size={18} />} label={t('nav.chat')} />
          <NavButton active={activeNav === 'agents'} onClick={() => switchNav('agents')} icon={<Bot size={18} />} label={t('nav.agents')} />
          <NavButton active={activeNav === 'history'} onClick={() => switchNav('history')} icon={<Clock size={18} />} label={t('nav.history')} />
          <NavButton active={activeNav === 'settings'} onClick={() => switchNav('settings')} icon={<Settings size={18} />} label={t('nav.settings')} />
        </nav>

        <div className="sidebar-spacer" />
        <div className="sidebar-footer">
          <button className="lang-switch" type="button" onClick={() => setLocale(locale === 'zh' ? 'en' : 'zh')} aria-label={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')} title={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}>
            <Globe size={17} />
            <span>{locale === 'zh' ? 'EN' : '中'}</span>
          </button>
        </div>
      </aside>

      {error ? (
        <div className="error-banner global-error">
          <span>{error}</span>
          <button className="error-dismiss" type="button" onClick={() => setError('')} aria-label={t('error.dismiss')}>&times;</button>
        </div>
      ) : null}

      {activeNav === 'chat' && (
        <section className="chat-area" aria-label={t('chat.ariaLabel')}>
          <header className="chat-header">
            <div className="chat-header-left">
              <select className="agent-select" value={activeAgentId} onChange={(e) => switchAgent(e.target.value)}>
                <option value="">{t('chat.selectAgent')}</option>
                {agents.map((agent) => <option key={agent.id} value={agent.id}>{agent.name}</option>)}
              </select>
              <select className="agent-select conversation-select" value={activeConversationId} onChange={(e) => setActiveConversationId(e.target.value)} disabled={!activeAgent}>
                <option value="">{t('conversation.select')}</option>
                {conversations.map((conversation) => <option key={conversation.id} value={conversation.id}>{conversation.title}</option>)}
              </select>
              <button className="icon-button" type="button" onClick={handleCreateConversation} disabled={!activeAgent} title={t('conversation.create')}>
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
            <textarea id="chat-input" value={draft} onChange={(e) => setDraft(e.target.value)} placeholder={activeAgent ? t('chat.placeholder') : t('chat.placeholderDisabled')} disabled={!activeAgent || isSending} rows={3} />
            <button className="send-button" type="submit" disabled={!activeAgent || !draft.trim() || isSending}>
              <Send size={18} />
              {isSending ? t('chat.sending') : t('chat.send')}
            </button>
          </form>
        </section>
      )}

      {activeNav === 'agents' && (
        <section className="config-panel agents-panel">
          <div className="config-sidebar agents-panel-sidebar">
            <div className="panel-header">
              <h2>{t('nav.agents')}</h2>
              <button className="icon-button light" type="button" onClick={() => { setActiveAgentId(''); setAgentFormDirty(false); }} title={t('agentConfig.create')} aria-label={t('agentConfig.create')}>
                <Plus size={17} />
              </button>
            </div>
            <div className="agent-list">
              {isLoadingAgents && agents.length === 0 ? <p className="muted">{t('chat.loading')}</p> : null}
              {!isLoadingAgents && agents.length === 0 ? <p className="muted">{t('agentConfig.noAgents')}</p> : null}
              {agents.map((agent) => (
                <button className={agent.id === activeAgentId ? 'agent-item active' : 'agent-item'} key={agent.id} type="button" onClick={() => switchAgent(agent.id)}>
                  <Bot size={17} />
                  <span>
                    <strong>{agent.name}</strong>
                    <small>{agentModelLabel(agent, providers)}</small>
                  </span>
                </button>
              ))}
            </div>
          </div>
          <div className="config-main agents-panel-main">
            <div className="config-page-header">
              <div>
                <p className="eyebrow">{t('agentConfig.title')}</p>
                <h2>{activeAgent ? activeAgent.name : t('agentConfig.newTitle')}</h2>
                <p>{activeAgent ? agentModelLabel(activeAgent, providers) : t('agentConfig.selectAgent')}</p>
              </div>
              {activeAgent ? <button className="icon-button danger light" type="button" onClick={handleDeleteAgent} title={t('agentConfig.delete')} aria-label={t('agentConfig.delete')}><Trash2 size={17} /></button> : null}
            </div>
            <form className="agent-form form-card" onSubmit={handleAgentSubmit}>
              <div className="form-grid two-column">
                <div className="field">
                  <label htmlFor="agent-name">{t('agentConfig.name')}</label>
                  <input id="agent-name" value={agentForm.name} onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }} placeholder={t('agentConfig.placeholder_name')} disabled={isSubmittingAgent} />
                </div>
                <div className="field">
                  <label htmlFor="agent-provider">{t('agentConfig.provider')}</label>
                  <select id="agent-provider" value={agentForm.model_provider_id} onChange={(e) => {
                    const provider = providers.find((p) => p.id === e.target.value);
                    setAgentForm({ ...agentForm, model_provider_id: e.target.value, model_id: provider?.models[0] ?? '' });
                    setAgentFormDirty(true);
                  }} disabled={isSubmittingAgent}>
                    <option value="">{t('agentConfig.placeholder_provider')}</option>
                    {providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name}</option>)}
                  </select>
                </div>
                <div className="field">
                  <label htmlFor="agent-model">{t('agentConfig.model')}</label>
                  <select id="agent-model" value={agentForm.model_id} onChange={(e) => { setAgentForm({ ...agentForm, model_id: e.target.value }); setAgentFormDirty(true); }} disabled={isSubmittingAgent || !selectedAgentProvider}>
                    <option value="">{t('agentConfig.placeholder_model')}</option>
                    {selectedAgentProvider?.models.map((model) => <option key={model} value={model}>{model}</option>)}
                  </select>
                </div>
              </div>
              <div className="field">
                <label htmlFor="agent-system-prompt">{t('agentConfig.systemPrompt')}</label>
                <textarea id="agent-system-prompt" value={agentForm.system_prompt} onChange={(e) => { setAgentForm({ ...agentForm, system_prompt: e.target.value }); setAgentFormDirty(true); }} rows={10} disabled={isSubmittingAgent} />
              </div>
              <div className="agent-form-actions form-actions-sticky">
                <button className="primary-button" type="submit" disabled={isSubmittingAgent}>
                  <Save size={17} />
                  {activeAgent ? t('agentConfig.save') : t('agentConfig.create')}
                </button>
              </div>
            </form>
          </div>
        </section>
      )}

      {activeNav === 'history' && (
        <section className="history-panel">
          {historyConversationId ? (
            <>
              <div className="panel-header">
                <button className="secondary-button" type="button" onClick={() => { setHistoryConversationId(''); setHistoryMessages([]); }}>
                  <X size={16} />
                  {t('history.back')}
                </button>
              </div>
              <div className="messages">
                {historyMessages.length === 0 ? <p className="muted centered">{t('history.empty')}</p> : null}
                {historyMessages.map((msg) => (
                  <article className={'message ' + msg.role} key={msg.clientId}>
                    <div className="message-meta">
                      <strong>{msg.role === 'user' ? t('chat.you') : agentName(msg.agent_id, agents)}</strong>
                      <time dateTime={msg.created_at}>{formatTime(msg.created_at, locale)}</time>
                    </div>
                    <p>{msg.content}</p>
                  </article>
                ))}
              </div>
            </>
          ) : (
            <>
              <div className="panel-header"><h2>{t('history.title')}</h2></div>
              <div className="history-list history-groups">
                {agents.length === 0 ? <p className="muted centered">{t('history.empty')}</p> : null}
                {agents.map((agent) => (
                  <section className="history-agent-group" key={agent.id}>
                    <h3>{agent.name}</h3>
                    {(historyConversations[agent.id] ?? []).length === 0 ? <p className="muted">{t('conversation.empty')}</p> : null}
                    {(historyConversations[agent.id] ?? []).map((conversation) => (
                      <button key={conversation.id} className="history-card" type="button" onClick={() => openHistoryConversation(conversation.id)}>
                        <MessageSquare size={20} />
                        <div>
                          <strong>{conversation.title}</strong>
                          <small>{formatDate(conversation.updated_at, locale)}</small>
                        </div>
                        <span className="history-card-actions" onClick={(event) => event.stopPropagation()}>
                          <button className="icon-button danger" type="button" onClick={() => handleDeleteConversation(conversation.id)}><Trash2 size={15} /></button>
                        </span>
                      </button>
                    ))}
                  </section>
                ))}
              </div>
            </>
          )}
        </section>
      )}

      {activeNav === 'settings' && (
        <section className="settings-panel config-panel">
          <div className="config-sidebar settings-nav">
            <div className="panel-header">
              <h2>{t('nav.settings')}</h2>
            </div>
            <div className="settings-section-list">
              <button className={activeSettingsSection === 'runtime' ? 'settings-section-item active' : 'settings-section-item'} type="button" onClick={() => setActiveSettingsSection('runtime')}>
                <Server size={18} />
                <span>
                  <strong>{t('settings.title')}</strong>
                  <small>{t('settings.description')}</small>
                </span>
              </button>
              <button className={activeSettingsSection === 'providers' ? 'settings-section-item active' : 'settings-section-item'} type="button" onClick={() => setActiveSettingsSection('providers')}>
                <KeyRound size={18} />
                <span>
                  <strong>{t('provider.title')}</strong>
                  <small>{t('provider.description')}</small>
                </span>
              </button>
            </div>
          </div>
          <div className="config-main settings-main">
            <div className="config-page-header">
              <div>
                <p className="eyebrow">{t('nav.settings')}</p>
                <h2>{activeSettingsSection === 'runtime' ? t('settings.title') : t('provider.title')}</h2>
                <p>{activeSettingsSection === 'runtime' ? t('settings.description') : t('provider.description')}</p>
              </div>
              {activeSettingsSection === 'providers' ? (
                <button className="secondary-button light" type="button" onClick={() => setActiveProviderId('')}>
                  <Plus size={17} />
                  {t('provider.create')}
                </button>
              ) : null}
            </div>

            {activeSettingsSection === 'runtime' ? (
              <form className="agent-form form-card narrow-form" onSubmit={handleSettingsSubmit}>
                <div className="form-grid two-column">
                  <div className="field">
                    <label htmlFor="settings-host">{t('settings.host')}</label>
                    <input id="settings-host" value={settingsForm.host} onChange={(e) => setSettingsForm({ ...settingsForm, host: e.target.value })} />
                  </div>
                  <div className="field">
                    <label htmlFor="settings-port">{t('settings.port')}</label>
                    <input id="settings-port" type="number" min={1} max={65535} value={settingsForm.port} onChange={(e) => setSettingsForm({ ...settingsForm, port: Number(e.target.value) })} />
                  </div>
                  <div className="field">
                    <label htmlFor="settings-log">{t('settings.logLevel')}</label>
                    <select id="settings-log" value={settingsForm.log_level} onChange={(e) => setSettingsForm({ ...settingsForm, log_level: e.target.value })}>
                      {['trace', 'debug', 'info', 'warn', 'error'].map((level) => <option key={level} value={level}>{level}</option>)}
                    </select>
                  </div>
                </div>
                <p className="hint-text">{t('settings.restartHint')}</p>
                <div className="agent-form-actions form-actions-sticky">
                  <button className="primary-button" type="submit"><Save size={17} />{t('settings.save')}</button>
                </div>
              </form>
            ) : (
              <div className="provider-layout">
                <aside className="provider-list-panel">
                  <div className="panel-header compact">
                    <h3>{t('provider.title')}</h3>
                  </div>
                  <div className="agent-list">
                    {providers.length === 0 ? <p className="muted">{t('provider.empty')}</p> : null}
                    {providers.map((provider) => (
                      <button className={provider.id === activeProviderId ? 'agent-item active' : 'agent-item'} key={provider.id} type="button" onClick={() => setActiveProviderId(provider.id)}>
                        <SlidersHorizontal size={17} />
                        <span>
                          <strong>{provider.name}</strong>
                          <small>{provider.models.length} {t('provider.modelsCount')}</small>
                        </span>
                      </button>
                    ))}
                  </div>
                </aside>
                <form className="agent-form form-card" onSubmit={handleProviderSubmit}>
                  <div className="panel-header">
                    <div>
                      <h2>{activeProvider ? activeProvider.name : t('provider.newTitle')}</h2>
                      <p className="muted">{activeProvider ? activeProvider.base_url : t('provider.selectHint')}</p>
                    </div>
                    {activeProvider ? <button className="icon-button danger light" type="button" onClick={handleDeleteProvider} title={t('provider.delete')} aria-label={t('provider.delete')}><Trash2 size={17} /></button> : null}
                  </div>
                  <div className="form-grid two-column">
                    <div className="field">
                      <label htmlFor="provider-name">{t('provider.name')}</label>
                      <input id="provider-name" value={providerForm.name} onChange={(e) => setProviderForm({ ...providerForm, name: e.target.value })} disabled={isSubmittingProvider} />
                    </div>
                    <div className="field">
                      <label htmlFor="provider-base-url">{t('provider.baseUrl')}</label>
                      <input id="provider-base-url" value={providerForm.base_url} onChange={(e) => setProviderForm({ ...providerForm, base_url: e.target.value })} disabled={isSubmittingProvider} />
                    </div>
                    <div className="field">
                      <label htmlFor="provider-api-key">{t('provider.apiKey')}</label>
                      <input id="provider-api-key" type="password" value={providerForm.api_key} onChange={(e) => setProviderForm({ ...providerForm, api_key: e.target.value })} placeholder={activeProvider?.has_api_key ? activeProvider.masked_api_key : ''} disabled={isSubmittingProvider} />
                    </div>
                  </div>
                  <div className="field">
                    <label htmlFor="provider-models" className="label-row">
                      <span>{t('provider.models')}</span>
                      {activeProvider ? (
                        <button
                          className="inline-button"
                          type="button"
                          disabled={fetchingModels}
                          onClick={async () => {
                            setFetchingModels(true);
                            setError('');
                            try {
                              const models = await fetchModels(activeProvider.id);
                              setProviderForm((current) => ({
                                ...current,
                                modelsText: models.join('\n'),
                              }));
                            } catch (cause) {
                              setError(toErrorMessage(cause));
                            } finally {
                              setFetchingModels(false);
                            }
                          }}
                        >
                          <Download size={14} />
                          {fetchingModels ? t('provider.fetchingModels') : t('provider.fetchModels')}
                        </button>
                      ) : null}
                    </label>
                    <textarea id="provider-models" value={providerForm.modelsText} onChange={(e) => setProviderForm({ ...providerForm, modelsText: e.target.value })} rows={10} disabled={isSubmittingProvider || fetchingModels} />
                  </div>
                  <div className="agent-form-actions form-actions-sticky">
                    <button className="primary-button" type="submit" disabled={isSubmittingProvider}>
                      <Save size={17} />
                      {activeProvider ? t('provider.save') : t('provider.create')}
                    </button>
                  </div>
                </form>
              </div>
            )}
          </div>
        </section>
      )}
    </main>
  );
}

function NavButton({ active, onClick, icon, label }: { active: boolean; onClick: () => void; icon: React.ReactNode; label: string }) {
  return (
    <button className={'nav-item' + (active ? ' active' : '')} type="button" onClick={onClick}>
      {icon}
      <span>{label}</span>
    </button>
  );
}

let nextClientId = 1;
function genClientId(): string {
  return 'c' + String(nextClientId++);
}

function toUiMessage(msg: ChatMessage): UiMessage {
  return { ...msg, clientId: msg.id ?? genClientId() };
}

function parseModels(value: string): string[] {
  return Array.from(new Set(value.split(/\r?\n|,/).map((model) => model.trim()).filter(Boolean))).sort();
}

function agentModelLabel(agent: Agent, providers: ModelProvider[]) {
  const provider = providers.find((item) => item.id === agent.model_provider_id);
  return provider ? `${provider.name} / ${agent.model_id}` : agent.model_id;
}

function agentName(agentId: string, agents: Agent[]) {
  return agents.find((agent) => agent.id === agentId)?.name ?? 'Assistant';
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

function formatDate(value: string, locale: string) {
  return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value));
}
