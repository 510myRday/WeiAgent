import { useState, useEffect, useCallback } from 'react';
import { MessageSquare, Trash2, X } from 'lucide-react';

import type { Agent, Conversation, UiMessage } from '../../types';
import { useI18n } from '../../i18n';
import { listConversations, readConversationMessages, deleteConversation } from '../../api';
import { Button, IconButton, ListItem, PaneHeader } from '../../components/ui';

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
      setHistoryMessages(raw.map((msg) => ({ ...msg, clientId: msg.id ?? genClientId() })));
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
    } catch {
      // History is secondary UI; keep the current view if deletion fails.
    }
  }

  function agentName(agentId: string) {
    return agents.find((agent) => agent.id === agentId)?.name ?? 'Assistant';
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
        <PaneHeader
          title={t('history.title')}
          leading={(
            <Button
              variant="secondary"
              type="button"
              onClick={() => { setHistoryConversationId(''); setHistoryMessages([]); }}
              icon={<X size={16} />}
            >
              {t('history.back')}
            </Button>
          )}
        />
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
      <PaneHeader title={t('history.title')} />
      <div className="history-list history-groups">
        {agents.length === 0 ? <p className="muted centered">{t('history.empty')}</p> : null}
        {agents.map((agent) => (
          <section className="history-agent-group" key={agent.id}>
            <h3>{agent.name}</h3>
            {(historyConversations[agent.id] ?? []).length === 0 ? <p className="muted">{t('conversation.empty')}</p> : null}
            {(historyConversations[agent.id] ?? []).map((conversation) => (
              <ListItem
                key={conversation.id}
                type="button"
                title={conversation.title}
                subtitle={formatDate(conversation.updated_at)}
                leading={<MessageSquare size={20} />}
                trailing={(
                  <span onClick={(event) => event.stopPropagation()}>
                    <IconButton variant="danger" type="button" onClick={() => handleDeleteConversation(conversation.id)} label={t('conversation.delete')}>
                      <Trash2 size={15} />
                    </IconButton>
                  </span>
                )}
                onClick={() => openHistoryConversation(conversation.id)}
              />
            ))}
          </section>
        ))}
      </div>
    </section>
  );
}
