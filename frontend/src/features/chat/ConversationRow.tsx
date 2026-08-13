import { memo } from 'react';
import { LoaderCircle, Pin, Trash2 } from 'lucide-react';

import type { Conversation } from '../../types';
import { IconButton, ListItem } from '../../components/ui';

function ConversationRowImpl({
  conversation,
  active,
  isRunning,
  isUnread,
  unreadLabel,
  formatDate,
  runStatusLabel,
  deleteLabel,
  pinLabel,
  isPinning,
  onSelect,
  onTogglePin,
  onDelete,
}: {
  conversation: Conversation;
  active: boolean;
  isRunning: boolean;
  isUnread: boolean;
  unreadLabel: string;
  formatDate: (value: string) => string;
  runStatusLabel: string;
  deleteLabel: string;
  pinLabel: string;
  isPinning: boolean;
  onSelect: (conversationId: string) => void;
  onTogglePin: (conversation: Conversation) => void;
  onDelete: (conversationId: string) => void;
}) {
  return (
    <div className={[
      'conversation-row',
      active ? 'is-active' : '',
      conversation.is_pinned ? 'is-pinned' : '',
    ].filter(Boolean).join(' ')}>
      <ListItem
        className="conversation-row-main"
        type="button"
        title={(
          <span className="conversation-row-title">
            <span>{conversation.title}</span>
            {isUnread ? <span className="unread-dot" aria-label={unreadLabel} title={unreadLabel} /> : null}
          </span>
        )}
        subtitle={(
          <span className={isRunning ? 'conversation-run-status' : undefined}>
            {isRunning ? <LoaderCircle size={12} aria-hidden="true" /> : null}
            <span>{isRunning ? runStatusLabel : formatDate(conversation.updated_at)}</span>
          </span>
        )}
        onClick={() => onSelect(conversation.id)}
      />
      <IconButton
        className="conversation-action conversation-pin-action"
        variant="ghost"
        type="button"
        active={conversation.is_pinned}
        disabled={isPinning}
        aria-pressed={conversation.is_pinned}
        onClick={() => onTogglePin(conversation)}
        label={pinLabel}
      >
        <Pin size={14} fill={conversation.is_pinned ? 'currentColor' : 'none'} />
      </IconButton>
      <IconButton
        className="conversation-action conversation-delete-action"
        variant="ghost"
        type="button"
        onClick={() => onDelete(conversation.id)}
        label={deleteLabel}
      >
        <Trash2 size={14} />
      </IconButton>
    </div>
  );
}

export const ConversationRow = memo(ConversationRowImpl);
