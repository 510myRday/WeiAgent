import { FormEvent, useEffect, useMemo, useState } from 'react';
import {
  Bot,
  Check,
  Globe,
  MessageSquare,
  Plus,
  RefreshCw,
  Save,
  Send,
  Trash2,
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
import type { Agent, ChatMessage, CreateAgentPayload, StreamEvent } from './types';

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  system_prompt: 'You are a helpful AI assistant.',
  model: '',
};

export function App() {
  const { t, locale, setLocale } = useI18n();
  const [agents, setAgents] = useState<Agent[]>([]);
  const [activeAgentId, setActiveAgentId] = useState<string>('');
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [isStreamMode, setIsStreamMode] = useState(true);
  const [isLoadingAgents, setIsLoadingAgents] = useState(true);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState('');
  const [modelOptions, setModelOptions] = useState<string[]>([]);

  const activeAgent = useMemo(
    () => agents.find((agent) => agent.id === activeAgentId),
    [agents, activeAgentId],
  );

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
      setAgentForm((current) => {
        if (current.model.trim()) {
          return current;
        }
        return {
          ...current,
          model: modelOptions[0] ?? '',
        };
      });
      return;
    }

    setAgentForm({
      name: activeAgent.name,
      system_prompt: activeAgent.system_prompt,
      model: activeAgent.model,
    });
  }, [activeAgent, modelOptions]);

  async function refreshAgents() {
    setIsLoadingAgents(true);
    setError('');
    try {
      const nextAgents = await listAgents();
      setAgents(nextAgents);
      setActiveAgentId((current) => {
        if (current && nextAgents.some((agent) => agent.id === current)) {
          return current;
        }
        return nextAgents[0]?.id ?? '';
      });
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsLoadingAgents(false);
    }
  }

  async function refreshModels() {
    try {
      setModelOptions(await listModels());
    } catch {
      setModelOptions([]);
    }
  }

  async function loadChat(agentId: string) {
    setIsLoadingChat(true);
    setError('');
    try {
      setMessages(await readChat(agentId));
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsLoadingChat(false);
    }
  }

  async function handleAgentSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError('');

    try {
      if (activeAgent) {
        const updated = await updateAgent(activeAgent.id, agentForm);
        setAgents((current) => current.map((agent) => (agent.id === updated.id ? updated : agent)));
        return;
      }

      const created = await createAgent(agentForm);
      setAgents((current) => [...current, created]);
      setActiveAgentId(created.id);
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function handleDeleteAgent() {
    if (!activeAgent) {
      return;
    }

    setError('');
    try {
      await deleteAgent(activeAgent.id);
      const remaining = agents.filter((agent) => agent.id !== activeAgent.id);
      setAgents(remaining);
      setActiveAgentId(remaining[0]?.id ?? '');
    } catch (cause) {
      setError(toErrorMessage(cause));
    }
  }

  async function handleSend(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const content = draft.trim();
    if (!activeAgent || !content || isSending) {
      return;
    }

    setDraft('');
    setError('');
    setIsSending(true);

    try {
      if (isStreamMode) {
        await streamMessage(activeAgent.id, content, handleStreamEvent);
      } else {
        const response = await sendMessage(activeAgent.id, content);
        setMessages((current) => [...current, response.user, response.assistant]);
      }
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setIsSending(false);
    }
  }

  function handleStreamEvent(event: StreamEvent) {
    if (event.event === 'error') {
      setError(event.data);
      return;
    }

    if (event.event === 'user') {
      setMessages((current) => [...current, event.data]);
      return;
    }

    if (event.event === 'delta') {
      setMessages((current) => {
        const last = current[current.length - 1];
        if (last?.role === 'assistant') {
          return [
            ...current.slice(0, -1),
            {
              ...last,
              content: last.content + event.data,
            },
          ];
        }
        return [
          ...current,
          {
            role: 'assistant',
            content: event.data,
            created_at: new Date().toISOString(),
          },
        ];
      });
      return;
    }

    if (event.event === 'assistant') {
      setMessages((current) => {
        const last = current[current.length - 1];
        if (last?.role === 'assistant') {
          return [...current.slice(0, -1), event.data];
        }
        return [...current, event.data];
      });
    }
  }

  return (
    <main className="workspace-shell">
      <aside className="sidebar" aria-label="Agent sidebar">
        <div className="brand-row">
          <div className="brand-mark" aria-hidden="true">
            <Bot size={22} />
          </div>
          <div>
            <h1>{t('brand.name')}</h1>
            <p>{t('brand.tagline')}</p>
          </div>
          <button
            className="lang-switch"
            type="button"
            onClick={() => setLocale(locale === 'zh' ? 'en' : 'zh')}
            aria-label="Switch language"
            title={locale === 'zh' ? 'Switch to English' : '切换到中文'}
          >
            <Globe size={17} />
            <span>{locale === 'zh' ? 'EN' : '中'}</span>
          </button>
        </div>

        <section className="panel agent-list-panel">
          <div className="section-header">
            <h2>{t('sidebar.agents')}</h2>
            <button className="icon-button" type="button" onClick={refreshAgents} aria-label={t('sidebar.refresh')}>
              <RefreshCw size={17} />
            </button>
          </div>

          <div className="agent-list">
            {isLoadingAgents ? <p className="muted">{t('sidebar.loading')}</p> : null}
            {!isLoadingAgents && agents.length === 0 ? (
              <p className="muted">{t('sidebar.empty')}</p>
            ) : null}
            {agents.map((agent) => (
              <button
                className={agent.id === activeAgentId ? 'agent-item active' : 'agent-item'}
                key={agent.id}
                type="button"
                onClick={() => setActiveAgentId(agent.id)}
              >
                <MessageSquare size={17} />
                <span>
                  <strong>{agent.name}</strong>
                  <small>{agent.model}</small>
                </span>
              </button>
            ))}
          </div>
        </section>

        <section className="panel">
          <div className="section-header">
            <h2>{activeAgent ? t('agentConfig.title') : t('agentConfig.newTitle')}</h2>
            {activeAgent ? (
              <button className="icon-button danger" type="button" onClick={handleDeleteAgent} aria-label={t('agentConfig.delete')}>
                <Trash2 size={17} />
              </button>
            ) : null}
          </div>

          <form className="agent-form" onSubmit={handleAgentSubmit}>
            <label htmlFor="agent-name">{t('agentConfig.name')}</label>
            <input
              id="agent-name"
              value={agentForm.name}
              onChange={(event) => setAgentForm({ ...agentForm, name: event.target.value })}
              placeholder={t('agentConfig.placeholder_name')}
            />

            <label htmlFor="agent-model">{t('agentConfig.model')}</label>
            <input
              id="agent-model"
              list="agent-model-options"
              value={agentForm.model}
              onChange={(event) => setAgentForm({ ...agentForm, model: event.target.value })}
              placeholder={modelOptions[0] ?? t('agentConfig.placeholder_model')}
            />
            <datalist id="agent-model-options">
              {modelOptions.map((model) => (
                <option value={model} key={model} />
              ))}
            </datalist>

            <label htmlFor="agent-system-prompt">{t('agentConfig.systemPrompt')}</label>
            <textarea
              id="agent-system-prompt"
              value={agentForm.system_prompt}
              onChange={(event) =>
                setAgentForm({ ...agentForm, system_prompt: event.target.value })
              }
              rows={5}
            />

            <button className="primary-button" type="submit">
              {activeAgent ? <Save size={17} /> : <Plus size={17} />}
              {activeAgent ? t('agentConfig.save') : t('agentConfig.create')}
            </button>

            {activeAgent ? (
              <button
                className="secondary-button"
                type="button"
                onClick={() => {
                  setActiveAgentId('');
                  setAgentForm({
                    ...defaultAgentForm,
                    model: modelOptions[0] ?? '',
                  });
                  setMessages([]);
                }}
              >
                <Plus size={17} />
                {t('agentConfig.new')}
              </button>
            ) : null}
          </form>
        </section>
      </aside>

      <section className="chat-area" aria-label="Conversation">
        <header className="chat-header">
          <div>
            <p className="eyebrow">{t('chat.currentAgent')}</p>
            <h2>{activeAgent?.name ?? t('chat.noAgent')}</h2>
            <span>{activeAgent?.system_prompt ?? t('chat.noAgentHint')}</span>
          </div>
          <label className="mode-toggle">
            <input
              type="checkbox"
              checked={isStreamMode}
              onChange={(event) => setIsStreamMode(event.target.checked)}
            />
            <span>{isStreamMode ? t('chat.stream') : t('chat.complete')}</span>
            <Check size={16} />
          </label>
        </header>

        {error ? <div className="error-banner">{error}</div> : null}

        <div className="messages" aria-live="polite">
          {isLoadingChat ? <p className="muted centered">{t('chat.loading')}</p> : null}
          {!isLoadingChat && messages.length === 0 ? (
            <div className="empty-chat">
              <Bot size={34} />
              <h3>{t('chat.emptyTitle')}</h3>
              <p>{t('chat.emptyHint')}</p>
            </div>
          ) : null}
          {messages.map((message, index) => (
            <article className={`message ${message.role}`} key={`${message.created_at}-${index}`}>
              <div className="message-meta">
                <strong>{message.role === 'user' ? t('chat.you') : activeAgent?.name ?? 'Assistant'}</strong>
                <time>{formatTime(message.created_at)}</time>
              </div>
              <p>{message.content}</p>
            </article>
          ))}
        </div>

        <form className="composer" onSubmit={handleSend}>
          <label htmlFor="chat-input">Message</label>
          <textarea
            id="chat-input"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
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
    </main>
  );
}

function toErrorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : 'Error';
}

function formatTime(value: string) {
  return new Intl.DateTimeFormat('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value));
}
