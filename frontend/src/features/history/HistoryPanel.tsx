import { useState, useEffect, useCallback } from 'react';
import { Pin, Trash2, X } from 'lucide-react';

import type { Agent, Conversation, UiMessage } from '../../types';
import { useI18n } from '../../i18n';
import {
  deleteConversation,
  listConversations,
  readConversationMessages,
  updateConversation,
} from '../../api';
import { Button, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';

function genClientId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return 'c' + Math.random().toString(36).slice(2, 10);
}

function sortConversationList(conversations: Conversation[]): Conversation[] {
  return [...conversations].sort((left, right) => {
    const pinOrder = Number(right.is_pinned) - Number(left.is_pinned);
    if (pinOrder !== 0) return pinOrder;
    return new Date(right.updated_at).getTime() - new Date(left.updated_at).getTime();
  });
}

interface HistoryPanelProps {
  agents: Agent[];
}

export function HistoryPanel({ agents }: HistoryPanelProps) {
  const { t, locale } = useI18n();
  const { confirm, toast } = useFeedback();
  const [historyConversations, setHistoryConversations] = useState<Record<string, Conversation[]>>({});
  const [historyConversationId, setHistoryConversationId] = useState('');
  const [historyMessages, setHistoryMessages] = useState<UiMessage[]>([]);
  const [pinningConversationIds, setPinningConversationIds] = useState<Set<string>>(() => new Set());

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
    } catch (cause) {
      setHistoryMessages([]);
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    }
  }

  async function handleDeleteConversation(id: string) {
    const confirmed = await confirm({
      title: t('conversation.delete'),
      message: t('conversation.deleteConfirm'),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;
    try {
      await deleteConversation(id);
      if (id === historyConversationId) {
        setHistoryConversationId('');
        setHistoryMessages([]);
      }
      await refreshHistory();
      toast.success(t('notification.conversationDeleted'));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    }
  }

  async function handlePinConversation(conversation: Conversation) {
    setPinningConversationIds((current) => new Set(current).add(conversation.id));
    try {
      const updated = await updateConversation(conversation.id, {
        is_pinned: !conversation.is_pinned,
      });
      setHistoryConversations((current) => ({
        ...current,
        [updated.agent_id]: sortConversationList(
          (current[updated.agent_id] ?? []).map((item) => (
            item.id === updated.id ? updated : item
          )),
        ),
      }));
      toast.success(t(conversation.is_pinned
        ? 'notification.conversationUnpinned'
        : 'notification.conversationPinned'));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setPinningConversationIds((current) => {
        const next = new Set(current);
        next.delete(conversation.id);
        return next;
      });
    }
  }

  function agentName(agentId: string) {
    const agent = agents.find((item) => item.id === agentId);
    if (!agent) return 'Assistant';
    return agent.is_builtin ? t('agentConfig.builtinMasterName') : agent.name;
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
            <h3>{agent.is_builtin ? t('agentConfig.builtinMasterName') : agent.name}</h3>
            {(historyConversations[agent.id] ?? []).length === 0 ? <p className="muted">{t('conversation.empty')}</p> : null}
            {(historyConversations[agent.id] ?? []).map((conversation) => (
              <div
                className={[
                  'conversation-row',
                  'history-conversation-row',
                  conversation.is_pinned ? 'is-pinned' : '',
                ].filter(Boolean).join(' ')}
                key={conversation.id}
              >
                <ListItem
                  className="conversation-row-main"
                  type="button"
                  title={conversation.title}
                  subtitle={formatDate(conversation.updated_at)}
                  onClick={() => openHistoryConversation(conversation.id)}
                />
                <IconButton
                  className="conversation-action conversation-pin-action"
                  variant="ghost"
                  type="button"
                  active={conversation.is_pinned}
                  disabled={pinningConversationIds.has(conversation.id)}
                  aria-pressed={conversation.is_pinned}
                  onClick={() => handlePinConversation(conversation)}
                  label={conversation.is_pinned ? t('conversation.unpin') : t('conversation.pin')}
                >
                  <Pin size={14} fill={conversation.is_pinned ? 'currentColor' : 'none'} />
                </IconButton>
                <IconButton
                  className="conversation-action conversation-delete-action"
                  variant="ghost"
                  type="button"
                  onClick={() => handleDeleteConversation(conversation.id)}
                  label={t('conversation.delete')}
                >
                  <Trash2 size={14} />
                </IconButton>
              </div>
            ))}
          </section>
        ))}
      </div>
    </section>
  );
}
