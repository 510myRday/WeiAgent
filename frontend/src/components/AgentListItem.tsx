import { ShieldCheck } from 'lucide-react';

import type { Agent } from '../types';
import { useI18n } from '../i18n';
import { AvatarBadge } from './AvatarPicker';
import { ListItem } from './ui';

type Translate = (path: string, params?: Record<string, string | number>) => string;

const BUILTIN_DEFAULT_NAME = 'Wei Sir';
const BUILTIN_LEGACY_ZH_NAME = '喂先生';
const BUILTIN_DEFAULT_DESCRIPTION = 'Coordinates tasks, delegates work, and drives complex jobs to completion.';
const BUILTIN_LEGACY_ZH_DESCRIPTION = '负责统筹任务、协调各智能体并推进复杂工作。';

export function agentDisplayName(agent: Agent, t: Translate): string {
  return agent.is_builtin && (agent.name === BUILTIN_DEFAULT_NAME || agent.name === BUILTIN_LEGACY_ZH_NAME)
    ? t('agentConfig.builtinAgentName')
    : agent.name;
}

export function agentDisplayDescription(agent: Agent, t: Translate): string {
  return agent.is_builtin
    && (agent.description === BUILTIN_DEFAULT_DESCRIPTION || agent.description === BUILTIN_LEGACY_ZH_DESCRIPTION)
    ? t('agentConfig.builtinAgentDescription')
    : agent.description;
}

export function AgentListItem({
  agent,
  active = false,
  unreadCount = 0,
  onClick,
}: {
  agent: Agent;
  active?: boolean;
  unreadCount?: number;
  onClick: () => void;
}) {
  const { t } = useI18n();

  return (
    <ListItem
      type="button"
      active={active}
      title={(
        <span className="agent-list-title">
          <span>{agentDisplayName(agent, t)}</span>
          {agent.is_builtin || unreadCount > 0 ? (
            <span className="agent-list-title-meta">
              {agent.is_builtin ? (
                <span className="mini-status-badge">
                  <ShieldCheck size={12} />
                  {t('agentConfig.builtinBadge')}
                </span>
              ) : null}
              {unreadCount > 0 ? (
                <span
                  className="unread-count-badge"
                  aria-label={t('conversation.unreadCount', { count: unreadCount })}
                  title={t('conversation.unreadCount', { count: unreadCount })}
                >
                  {unreadCount > 99 ? '99+' : unreadCount}
                </span>
              ) : null}
            </span>
          ) : null}
        </span>
      )}
      subtitle={agentDisplayDescription(agent, t)}
      leading={<AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} />}
      onClick={onClick}
    />
  );
}
