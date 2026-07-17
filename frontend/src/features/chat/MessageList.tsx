import {
  forwardRef,
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import {
  Bot,
  ChevronDown,
  LoaderCircle,
  MessageSquare,
  Pencil,
  RotateCcw,
  Send,
  X,
} from 'lucide-react';

import type {
  Agent,
  AgentRunPhase,
  AppSettings,
  ToolCallMessage,
  UiMessage,
  UiMessageItem,
} from '../../types';
import { ToolCallCard } from '../../components/ToolCallCard';
import { AvatarBadge } from '../../components/AvatarPicker';
import { agentDisplayName } from '../../components/AgentListItem';
import { Button, EmptyState, useFeedback } from '../../components/ui';
import { useI18n } from '../../i18n';
import { MarkdownContent } from './MarkdownContent';

interface MessageListProps {
  conversationId: string;
  messages: UiMessageItem[];
  activeAgent?: Agent;
  settings: AppSettings;
  isLoading: boolean;
  hasOlderMessages: boolean;
  isLoadingOlderMessages: boolean;
  isSending: boolean;
  runPhase: AgentRunPhase;
  terminalPermissionMode?: 'request_approval' | 'full_access' | null;
  timelineMessageIndexes: readonly number[];
  onLoadOlder: () => Promise<boolean>;
  onRestart: (messageId: string, content: string) => void;
  onApproveToolCall: (
    approvalId: string,
    agentId: string,
    decision: 'approve' | 'reject' | 'always_approve',
  ) => Promise<void>;
  onActiveTimelineMessageChange: (messageIndex: number | null) => void;
  formatTimelineTime: (value: string) => string;
  onCreateAgent: () => void;
}

export type MessageListHandle = {
  scrollToMessage: (messageIndex: number, behavior?: ScrollBehavior) => void;
};

type VirtualRow =
  | {
    key: string;
    kind: 'load_older';
  }
  | {
    key: string;
    kind: 'message';
    messageIndex: number;
    item: UiMessageItem;
  }
  | {
    key: string;
    kind: 'waiting';
  };

const MESSAGE_ESTIMATE = 156;
const TOOL_ESTIMATE = 250;
const LOAD_OLDER_ESTIMATE = 56;
const WAITING_ESTIMATE = 90;
const INITIAL_POSITION_MAX_FRAMES = 12;
const INITIAL_POSITION_STABLE_FRAMES = 2;
const BOTTOM_POSITION_TOLERANCE = 1;

export const MessageList = forwardRef<MessageListHandle, MessageListProps>(function MessageList({
  conversationId,
  messages,
  activeAgent,
  settings,
  isLoading,
  hasOlderMessages,
  isLoadingOlderMessages,
  isSending,
  runPhase,
  terminalPermissionMode,
  timelineMessageIndexes,
  onLoadOlder,
  onRestart,
  onApproveToolCall,
  onActiveTimelineMessageChange,
  formatTimelineTime,
  onCreateAgent,
}, ref) {
  const { t } = useI18n();
  const scrollRef = useRef<HTMLDivElement>(null);
  const initialPositionFrameRef = useRef<number | undefined>(undefined);
  const scrollFrameRef = useRef<number | undefined>(undefined);
  const pendingAnchorKeyRef = useRef('');
  const [isAwayFromBottom, setIsAwayFromBottom] = useState(false);
  const [positionedConversationId, setPositionedConversationId] = useState('');
  const timelineIndexSet = useMemo(() => new Set(timelineMessageIndexes), [timelineMessageIndexes]);
  const rows = useMemo<VirtualRow[]>(() => {
    const next: VirtualRow[] = [];
    if (activeAgent && hasOlderMessages) {
      next.push({ key: 'load-older', kind: 'load_older' });
    }
    messages.forEach((item, messageIndex) => {
      next.push({
        key: 'role' in item ? `message-${item.clientId}` : `tool-${item.call_id}`,
        kind: 'message',
        messageIndex,
        item,
      });
    });
    if (activeAgent && shouldShowAgentWaitingBubble(messages, isSending, runPhase)) {
      next.push({ key: 'waiting', kind: 'waiting' });
    }
    return next;
  }, [activeAgent, hasOlderMessages, isSending, messages, runPhase]);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => {
      const row = rows[index];
      if (row?.kind === 'load_older') return LOAD_OLDER_ESTIMATE;
      if (row?.kind === 'waiting') return WAITING_ESTIMATE;
      return row && 'type' in row.item && row.item.type === 'tool_call'
        ? TOOL_ESTIMATE
        : MESSAGE_ESTIMATE;
    },
    getItemKey: (index) => rows[index]?.key ?? index,
    anchorTo: 'end',
    followOnAppend: 'auto',
    scrollEndThreshold: 120,
    overscan: 6,
  });
  const isInitialPositioning = Boolean(
    conversationId
    && !isLoading
    && messages.length > 0
    && positionedConversationId !== conversationId,
  );

  const isMessagesNearBottom = useCallback(() => {
    const element = scrollRef.current;
    if (!element) return true;
    return element.scrollHeight - element.scrollTop - element.clientHeight < 120;
  }, []);

  const updateScrollButtonVisibility = useCallback(() => {
    const element = scrollRef.current;
    if (!element) {
      setIsAwayFromBottom(false);
      return;
    }
    setIsAwayFromBottom(
      element.scrollHeight > element.clientHeight + 8 && !isMessagesNearBottom(),
    );
  }, [isMessagesNearBottom]);

  const scheduleScrollButtonUpdate = useCallback(() => {
    if (scrollFrameRef.current !== undefined) return;
    scrollFrameRef.current = window.requestAnimationFrame(() => {
      scrollFrameRef.current = undefined;
      updateScrollButtonVisibility();
    });
  }, [updateScrollButtonVisibility]);

  const scrollToBottom = useCallback((force = false, behavior: ScrollBehavior = 'auto') => {
    if (!force && !isMessagesNearBottom()) return;
    virtualizer.scrollToEnd({ behavior });
    setIsAwayFromBottom(false);
  }, [isMessagesNearBottom, virtualizer]);

  useImperativeHandle(ref, () => ({
    scrollToMessage(messageIndex, behavior = 'smooth') {
      const rowIndex = rows.findIndex(
        (row) => row.kind === 'message' && row.messageIndex === messageIndex,
      );
      if (rowIndex < 0) return;
      virtualizer.scrollToIndex(rowIndex, { align: 'start', behavior });
    },
  }), [rows, virtualizer]);

  useLayoutEffect(() => {
    if (!isInitialPositioning) return;

    let cancelled = false;
    let checkedFrames = 0;
    let stableFrames = 0;
    let previousScrollHeight = -1;

    const finishPositioning = () => {
      virtualizer.scrollToEnd({ behavior: 'auto' });
      initialPositionFrameRef.current = undefined;
      setPositionedConversationId(conversationId);
    };

    const checkPosition = () => {
      if (cancelled) return;
      const element = scrollRef.current;
      if (!element) {
        finishPositioning();
        return;
      }

      const distanceFromBottom = element.scrollHeight - element.scrollTop - element.clientHeight;
      const isAtBottom = Math.abs(distanceFromBottom) <= BOTTOM_POSITION_TOLERANCE;
      const isHeightStable = element.scrollHeight === previousScrollHeight;
      stableFrames = isAtBottom && isHeightStable ? stableFrames + 1 : 0;
      previousScrollHeight = element.scrollHeight;
      checkedFrames += 1;

      if (!isAtBottom) {
        virtualizer.scrollToEnd({ behavior: 'auto' });
      }

      if (
        stableFrames >= INITIAL_POSITION_STABLE_FRAMES
        || checkedFrames >= INITIAL_POSITION_MAX_FRAMES
      ) {
        finishPositioning();
        return;
      }

      initialPositionFrameRef.current = window.requestAnimationFrame(checkPosition);
    };

    virtualizer.scrollToEnd({ behavior: 'auto' });
    initialPositionFrameRef.current = window.requestAnimationFrame(checkPosition);

    return () => {
      cancelled = true;
      if (initialPositionFrameRef.current !== undefined) {
        window.cancelAnimationFrame(initialPositionFrameRef.current);
        initialPositionFrameRef.current = undefined;
      }
    };
  }, [conversationId, isInitialPositioning, virtualizer]);

  useLayoutEffect(() => {
    if (isSending) scrollToBottom(true);
  }, [isSending, scrollToBottom]);

  useEffect(() => {
    scheduleScrollButtonUpdate();
    return () => {
      if (scrollFrameRef.current !== undefined) {
        window.cancelAnimationFrame(scrollFrameRef.current);
        scrollFrameRef.current = undefined;
      }
    };
  }, [messages, scheduleScrollButtonUpdate]);

  useEffect(() => {
    const container = scrollRef.current;
    if (!container || timelineIndexSet.size === 0) {
      onActiveTimelineMessageChange(null);
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      const visible = entries
        .filter((entry) => entry.isIntersecting)
        .sort((left, right) => left.boundingClientRect.top - right.boundingClientRect.top);
      const messageIndex = Number((visible[0]?.target as HTMLElement | undefined)?.dataset.messageIndex);
      if (Number.isInteger(messageIndex)) onActiveTimelineMessageChange(messageIndex);
    }, {
      root: container,
      rootMargin: '-20% 0px -65% 0px',
      threshold: 0,
    });
    const anchors = container.querySelectorAll<HTMLElement>('[data-message-index]');
    anchors.forEach((anchor) => {
      const messageIndex = Number(anchor.dataset.messageIndex);
      if (timelineIndexSet.has(messageIndex)) observer.observe(anchor);
    });
    return () => observer.disconnect();
  }, [onActiveTimelineMessageChange, rows, timelineIndexSet]);

  useLayoutEffect(() => {
    const anchorKey = pendingAnchorKeyRef.current;
    if (!anchorKey) return;
    const rowIndex = rows.findIndex((row) => row.key === anchorKey);
    if (rowIndex < 0) return;
    pendingAnchorKeyRef.current = '';
    virtualizer.scrollToIndex(rowIndex, { align: 'start' });
  }, [rows, virtualizer]);

  const handleLoadOlder = useCallback(async () => {
    if (isLoadingOlderMessages) return;
    const firstVisibleMessage = virtualizer.getVirtualItems()
      .map((item) => rows[item.index])
      .find((row): row is Extract<VirtualRow, { kind: 'message' }> => row?.kind === 'message');
    pendingAnchorKeyRef.current = firstVisibleMessage?.key ?? '';
    const loaded = await onLoadOlder();
    if (!loaded) pendingAnchorKeyRef.current = '';
  }, [isLoadingOlderMessages, onLoadOlder, rows, virtualizer]);

  const handleScroll = useCallback(() => {
    scheduleScrollButtonUpdate();
  }, [scheduleScrollButtonUpdate]);

  if (isLoading || messages.length === 0) {
    return (
      <div
        className="messages"
        aria-busy={isLoading || undefined}
        aria-live="polite"
        ref={scrollRef}
        onScroll={handleScroll}
      >
        {activeAgent && isLoading ? <p className="muted centered">{t('chat.loading')}</p> : null}
        {(!activeAgent || !isLoading) && messages.length === 0 ? (
          <EmptyState
            icon={activeAgent ? <MessageSquare size={34} /> : <Bot size={34} />}
            title={activeAgent ? t('chat.emptyTitle') : t('chat.noAgent')}
            description={activeAgent ? t('chat.emptyHint') : t('chat.noAgentHint')}
            action={!activeAgent ? (
              <Button variant="primary" type="button" onClick={onCreateAgent}>
                {t('agentConfig.create')}
              </Button>
            ) : undefined}
          />
        ) : null}
      </div>
    );
  }

  return (
    <>
      <div
        className={`messages${isInitialPositioning ? ' is-initial-positioning' : ''}`}
        aria-busy={isInitialPositioning || undefined}
        aria-live="polite"
        ref={scrollRef}
        onScroll={handleScroll}
      >
        <div className="message-list-virtualizer" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((virtualItem) => {
            const row = rows[virtualItem.index];
            if (!row) return null;
            return (
              <div
                key={virtualItem.key}
                ref={virtualizer.measureElement}
                data-index={virtualItem.index}
                data-message-index={row.kind === 'message' && timelineIndexSet.has(row.messageIndex)
                  ? row.messageIndex
                  : undefined}
                className="message-list-virtual-row"
                style={{ transform: `translateY(${virtualItem.start}px)` }}
              >
                {row.kind === 'load_older' ? (
                  <div className="load-earlier-messages">
                    <Button
                      variant="secondary"
                      type="button"
                      disabled={isLoadingOlderMessages}
                      onClick={() => void handleLoadOlder()}
                      icon={isLoadingOlderMessages ? <LoaderCircle size={15} className="is-spinning" /> : undefined}
                    >
                      {t(isLoadingOlderMessages ? 'conversation.loadingEarlier' : 'conversation.loadEarlier')}
                    </Button>
                  </div>
                ) : row.kind === 'waiting' ? (
                  <WaitingBubble agent={activeAgent} settings={settings} />
                ) : (
                  <MessageListItem
                    item={row.item}
                    previous={messages[row.messageIndex - 1]}
                    activeAgent={activeAgent}
                    settings={settings}
                    terminalPermissionMode={terminalPermissionMode}
                    isSending={isSending}
                    onRestart={onRestart}
                    onApproveToolCall={onApproveToolCall}
                    formatTimelineTime={formatTimelineTime}
                  />
                )}
              </div>
            );
          })}
        </div>
      </div>
      {isAwayFromBottom ? (
        <button
          className="scroll-to-bottom-button"
          type="button"
          onClick={() => scrollToBottom(true, 'smooth')}
          aria-label={t('chat.scrollToBottom')}
          title={t('chat.scrollToBottom')}
        >
          <ChevronDown size={22} strokeWidth={2.4} />
        </button>
      ) : null}
    </>
  );
});

const MessageListItem = memo(function MessageListItem({
  item,
  previous,
  activeAgent,
  settings,
  terminalPermissionMode,
  isSending,
  onRestart,
  onApproveToolCall,
  formatTimelineTime,
}: {
  item: UiMessageItem;
  previous?: UiMessageItem;
  activeAgent?: Agent;
  settings: AppSettings;
  terminalPermissionMode?: 'request_approval' | 'full_access' | null;
  isSending: boolean;
  onRestart: (messageId: string, content: string) => void;
  onApproveToolCall: MessageListProps['onApproveToolCall'];
  formatTimelineTime: (value: string) => string;
}) {
  const { t } = useI18n();

  if ('type' in item && item.type === 'tool_call') {
    return (
      <ToolMessageRow
        item={item}
        activeAgent={activeAgent}
        settings={settings}
        terminalPermissionMode={terminalPermissionMode}
        onApproveToolCall={onApproveToolCall}
      />
    );
  }

  const message = item as UiMessage;
  const previousMessage = previous && 'role' in previous ? previous as UiMessage : undefined;
  return (
    <MessageRow
      message={message}
      showTimeSeparator={shouldShowTimeSeparator(message, previousMessage)}
      timeSeparatorLabel={formatTimelineTime(message.created_at)}
      participantName={message.role === 'user' ? t('chat.you') : activeAgent ? agentDisplayName(activeAgent, t) : 'Assistant'}
      agent={activeAgent}
      settings={settings}
      failedLabel={t('chat.failed')}
      actionsDisabled={isSending}
      onRestart={onRestart}
    />
  );
});

const ToolMessageRow = memo(function ToolMessageRow({
  item,
  activeAgent,
  settings,
  terminalPermissionMode,
  onApproveToolCall,
}: {
  item: ToolCallMessage;
  activeAgent?: Agent;
  settings: AppSettings;
  terminalPermissionMode?: 'request_approval' | 'full_access' | null;
  onApproveToolCall: MessageListProps['onApproveToolCall'];
}) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const handleApprove = useCallback(async (
    decision: 'approve' | 'reject' | 'always_approve',
  ) => {
    const approvalId = typeof item.arguments.approval_id === 'string'
      ? item.arguments.approval_id
      : '';
    const argumentAgentId = typeof item.arguments.agent_id === 'string'
      ? item.arguments.agent_id
      : '';
    const agentId = argumentAgentId || activeAgent?.id;
    if (!approvalId || !agentId) return;
    try {
      await onApproveToolCall(approvalId, agentId, decision);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
      throw cause;
    }
  }, [activeAgent?.id, item.arguments.agent_id, item.arguments.approval_id, onApproveToolCall, t, toast]);

  return (
    <div className="message-row assistant message-row--tool">
      <MessageAvatar agent={activeAgent} settings={settings} isUser={false} />
      <div className="message-body">
        <div className="message-meta"><strong>{activeAgent ? agentDisplayName(activeAgent, t) : 'Assistant'}</strong></div>
        <div className="message-bubble message-bubble--tool">
          <ToolCallCard
            agentId={activeAgent?.id ?? ''}
            callId={item.call_id}
            name={item.name}
            arguments={item.arguments}
            status={item.status}
            result={item.result}
            permissionMode={item.permission_mode ?? terminalPermissionMode}
            onApprove={handleApprove}
          />
        </div>
      </div>
    </div>
  );
});

function shouldShowAgentWaitingBubble(
  messages: UiMessageItem[],
  isSending: boolean,
  runPhase: AgentRunPhase,
): boolean {
  if (!isSending) return false;
  if (runPhase === 'waiting_for_approval') return false;

  const last = messages[messages.length - 1];
  if (!last) return true;
  if ('type' in last && last.type === 'tool_call') return last.status !== 'pending_approval';
  if ('role' in last && last.role === 'assistant') {
    return last.status !== 'streaming' && runPhase === 'executing_tool';
  }
  return true;
}

function WaitingBubble({ agent, settings }: { agent?: Agent; settings: AppSettings }) {
  const { t } = useI18n();
  return (
    <article className="message-row assistant is-streaming is-waiting" aria-live="polite">
      <MessageAvatar agent={agent} settings={settings} isUser={false} />
      <div className="message-body">
        <div className="message-meta">
          <strong>{agent ? agentDisplayName(agent, t) : 'Assistant'}</strong>
        </div>
        <div className="message-bubble">
          <TypingIndicator label={t('chat.sending')} />
        </div>
      </div>
    </article>
  );
}

const MessageRow = memo(function MessageRow({
  message,
  showTimeSeparator,
  timeSeparatorLabel,
  participantName,
  agent,
  settings,
  failedLabel,
  actionsDisabled,
  onRestart,
}: {
  message: UiMessage;
  showTimeSeparator: boolean;
  timeSeparatorLabel: string;
  participantName: string;
  agent?: Agent;
  settings: AppSettings;
  failedLabel: string;
  actionsDisabled: boolean;
  onRestart: (messageId: string, content: string) => void;
}) {
  const { t } = useI18n();
  const isUser = message.role === 'user';
  const [isEditing, setIsEditing] = useState(false);
  const [editDraft, setEditDraft] = useState(message.content);
  const editInputRef = useRef<HTMLTextAreaElement>(null);
  const canRestart = isUser && Boolean(message.id) && message.status !== 'streaming';

  useLayoutEffect(() => {
    if (!isEditing) return;
    const input = editInputRef.current;
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = `${Math.min(Math.max(input.scrollHeight, 72), 240)}px`;
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }, [editDraft, isEditing]);

  function beginEditing() {
    setEditDraft(message.content);
    setIsEditing(true);
  }

  function cancelEditing() {
    setEditDraft(message.content);
    setIsEditing(false);
  }

  function submitEdit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const content = editDraft.trim();
    if (!content || actionsDisabled || !message.id) return;
    setIsEditing(false);
    onRestart(message.id, content);
  }

  function handleEditKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Escape') {
      event.preventDefault();
      cancelEditing();
      return;
    }
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  }

  return (
    <>
      {showTimeSeparator ? (
        <div className="message-time-separator">
          <time dateTime={message.created_at}>{timeSeparatorLabel}</time>
        </div>
      ) : null}
      <article
        className={[
          'message-row',
          message.role,
          message.status === 'streaming' ? 'is-streaming' : '',
          isEditing ? 'is-editing' : '',
        ].filter(Boolean).join(' ')}
      >
        {!isUser ? <MessageAvatar agent={agent} settings={settings} isUser={false} /> : null}
        <div className="message-body">
          <div className="message-meta">
            <strong>{participantName}</strong>
          </div>
          {isEditing ? (
            <form className="message-edit-form" onSubmit={submitEdit}>
              <div className="message-bubble message-edit-bubble">
                <textarea
                  ref={editInputRef}
                  className="message-edit-textarea"
                  value={editDraft}
                  onChange={(event) => setEditDraft(event.target.value)}
                  onKeyDown={handleEditKeyDown}
                  aria-label={t('chat.editMessage')}
                  disabled={actionsDisabled}
                  rows={3}
                />
              </div>
              <div className="message-actions message-edit-actions">
                <button
                  className="message-action-button"
                  type="button"
                  onClick={cancelEditing}
                  aria-label={t('chat.cancelEdit')}
                  title={t('chat.cancelEdit')}
                >
                  <X size={14} />
                  <span>{t('chat.cancelEdit')}</span>
                </button>
                <button
                  className="message-action-button is-primary"
                  type="submit"
                  disabled={actionsDisabled || !editDraft.trim()}
                  aria-label={t('chat.sendEdit')}
                  title={t('chat.sendEdit')}
                >
                  <Send size={14} />
                  <span>{t('chat.sendEdit')}</span>
                </button>
              </div>
            </form>
          ) : (
            <>
              <div className="message-bubble">
                <MessageContent message={message} />
                {message.status === 'failed' ? <span className="message-status error">{failedLabel}</span> : null}
              </div>
              {canRestart ? (
                <div className="message-actions">
                  <button
                    className="message-action-button"
                    type="button"
                    onClick={beginEditing}
                    disabled={actionsDisabled}
                    aria-label={t('chat.editMessage')}
                    title={t('chat.editMessage')}
                  >
                    <Pencil size={15} />
                  </button>
                  <button
                    className="message-action-button"
                    type="button"
                    onClick={() => onRestart(message.id, message.content)}
                    disabled={actionsDisabled}
                    aria-label={t('chat.retryMessage')}
                    title={t('chat.retryMessage')}
                  >
                    <RotateCcw size={15} />
                  </button>
                </div>
              ) : null}
            </>
          )}
        </div>
        {isUser ? <MessageAvatar agent={agent} settings={settings} isUser /> : null}
      </article>
    </>
  );
});

export function MessageAvatar({
  agent,
  settings,
  isUser,
}: {
  agent?: Agent;
  settings: AppSettings;
  isUser: boolean;
}) {
  if (isUser) {
    return <AvatarBadge icon={settings.avatar_icon} color={settings.avatar_color} size={23} className="chat-avatar" />;
  }
  if (agent) {
    return <AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} size={23} className="chat-avatar" />;
  }
  return <span className="chat-avatar avatar-muted"><Bot size={23} /></span>;
}

function MessageContent({ message }: { message: UiMessage }) {
  const { t } = useI18n();
  if (message.status === 'streaming' && !message.content) {
    return <TypingIndicator label={t('chat.sending')} />;
  }
  return (
    <div className={`message-markdown${message.status === 'streaming' ? ' streaming-content' : ''}`}>
      <MarkdownContent content={message.content} />
    </div>
  );
}

function TypingIndicator({ label }: { label: string }) {
  return (
    <div className="typing-indicator" role="status" aria-label={label}>
      <span />
      <span />
      <span />
    </div>
  );
}

function shouldShowTimeSeparator(message: UiMessage, previous?: UiMessage) {
  if (!previous) return true;
  const currentTime = new Date(message.created_at).getTime();
  const previousTime = new Date(previous.created_at).getTime();
  if (!Number.isFinite(currentTime) || !Number.isFinite(previousTime)) return false;
  return currentTime - previousTime >= 5 * 60 * 1000;
}
