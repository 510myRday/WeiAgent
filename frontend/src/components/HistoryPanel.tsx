import { useState, useEffect, useCallback } from 'react';
import { MessageSquare, Trash2, X } from 'lucide-react';
import type { Agent, Conversation, UiMessage } from '../types';
import { useI18n } from '../i18n';
import { listConversations, readConversationMessages, deleteConversation } from '../api';

function genClientId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return 'c' + Math.random().toString(36).slice(2, 10);
}

interface HistoryPanelProps {
  agents: Agent[];
}

export function HistoryPanel({ agents }: HistoryPanelProps) {
  const { t, locale } = useI18n();
  const [historyConversations, setHistoryConversations] = useState<Record<string, Conversation[]>>({});
  const [historyConversationId, setHistoryConversationId] = useState('');
  const [historyMessages, setHistoryMessages] = useState<UiMessage[]>([]);

  const refreshHistory = useCallback(async () => {
    const entries = await Promise.all(
      agents.map(async (agent) => [agent.id, await listConversations(agent.id)] as const),
    );
    setHistoryConversations(Object.fromEntries(entries));
  }, [agents]);

  useEffect(() => {
    void refreshHistory();
  }, [refreshHistory]);

  async function openHistoryConversation(id: string) {
    setHistoryConversationId(id);
    try {
      const raw = await readConversationMessages(id);
      setHistoryMessages(
        raw.map((msg) => ({ ...msg, clientId: msg.id ?? genClientId() })),
      );
    } catch {
      setHistoryMessages([]);
    }
  }

  async function handleDeleteConversation(id: string) {
    if (!window.confirm(t('conversation.deleteConfirm'))) return;
    try {
      await deleteConversation(id);
      if (id === historyConversationId) {
        setHistoryConversationId('');
        setHistoryMessages([]);
      }
      await refreshHistory();
    } catch { /* ignore */ }
  }

  function agentName(agentId: string) {
    return agents.find((a) => a.id === agentId)?.name ?? 'Assistant';
  }

  function formatDate(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  function formatTime(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  if (historyConversationId) {
    return (
      <section className="history-panel">
        <div className="panel-header">
          <button
            className="secondary-button"
            type="button"
            onClick={() => { setHistoryConversationId(''); setHistoryMessages([]); }}
          >
            <X size={16} />
            {t('history.back')}
          </button>
        </div>
        <div className="messages">
          {historyMessages.length === 0 ? <p className="muted centered">{t('history.empty')}</p> : null}
          {historyMessages.map((msg) => (
            <article className={'message ' + msg.role} key={msg.clientId}>
              <div className="message-meta">
                <strong>{msg.role === 'user' ? t('chat.you') : agentName(msg.agent_id)}</strong>
                <time dateTime={msg.created_at}>{formatTime(msg.created_at)}</time>
              </div>
              <p>{msg.content}</p>
            </article>
          ))}
        </div>
      </section>
    );
  }

  return (
    <section className="history-panel">
      <div className="panel-header"><h2>{t('history.title')}</h2></div>
      <div className="history-list history-groups">
        {agents.length === 0 ? <p className="muted centered">{t('history.empty')}</p> : null}
        {agents.map((agent) => (
          <section className="history-agent-group" key={agent.id}>
            <h3>{agent.name}</h3>
            {(historyConversations[agent.id] ?? []).length === 0 ? <p className="muted">{t('conversation.empty')}</p> : null}
            {(historyConversations[agent.id] ?? []).map((conv) => (
              <button
                key={conv.id}
                className="history-card"
                type="button"
                onClick={() => openHistoryConversation(conv.id)}
              >
                <MessageSquare size={20} />
                <div>
                  <strong>{conv.title}</strong>
                  <small>{formatDate(conv.updated_at)}</small>
                </div>
                <span className="history-card-actions" onClick={(e) => e.stopPropagation()}>
                  <button className="icon-button danger" type="button" onClick={() => handleDeleteConversation(conv.id)}>
                    <Trash2 size={15} />
                  </button>
                </span>
              </button>
            ))}
          </section>
        ))}
      </div>
    </section>
  );
}

