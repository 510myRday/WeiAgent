import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import {
  ArrowLeft,
  Bot,
  ChevronDown,
  LoaderCircle,
  MessageSquare,
} from 'lucide-react';

import type {
  Agent,
  AgentRunPhase,
  AppSettings,
  ImageAttachment,
  UiMessageItem,
} from '../../types';
import { Button, EmptyState } from '../../components/ui';
import { useI18n } from '../../i18n';
import {
  MessageListItem,
  MessageAvatar,
  WaitingBubble,
  shouldShowAgentWaitingBubble,
} from './messageListItems';

export { MessageAvatar };

interface MessageListProps {
  conversationId: string;
  messages: UiMessageItem[];
  activeAgent?: Agent;
  settings: AppSettings;
  isLoading: boolean;
  hasOlderMessages: boolean;
  isLoadingOlderMessages: boolean;
  isSending: boolean;
  supportsImages: boolean;
  supportsImageMasks: boolean;
  runPhase: AgentRunPhase;
  retryStatus: { attempt: number; maxAttempts: number; reason: string } | null;
  timelineMessageIndexes: readonly number[];
  onLoadOlder: () => Promise<boolean>;
  onRestart: (messageId: string, content: string, attachments?: ImageAttachment[]) => void;
  onEditImage: (attachment: ImageAttachment) => void;
  onApproveToolCall: (
    approvalId: string,
    agentId: string,
    decision: 'approve' | 'reject' | 'always_approve',
  ) => Promise<void>;
  onActiveTimelineMessageChange: (messageIndex: number | null) => void;
  formatTimelineTime: (value: string) => string;
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
const LOAD_OLDER_ESTIMATE = 56;
const WAITING_ESTIMATE = 90;
const INITIAL_POSITION_MAX_FRAMES = 12;
const INITIAL_POSITION_STABLE_FRAMES = 2;
const BOTTOM_POSITION_TOLERANCE = 1;
const WAITING_SCROLL_PADDING = 48;
const STREAM_SCROLL_THROTTLE_MS = 800;

export const MessageList = forwardRef<MessageListHandle, MessageListProps>(function MessageList({
  conversationId,
  messages,
  activeAgent,
  settings,
  isLoading,
  hasOlderMessages,
  isLoadingOlderMessages,
  isSending,
  supportsImages,
  supportsImageMasks,
  runPhase,
  retryStatus,
  timelineMessageIndexes,
  onLoadOlder,
  onRestart,
  onEditImage,
  onApproveToolCall,
  onActiveTimelineMessageChange,
  formatTimelineTime,
}, ref) {
  const { t } = useI18n();
  const scrollRef = useRef<HTMLDivElement>(null);
  const initialPositionFrameRef = useRef<number | undefined>(undefined);
  const scrollFrameRef = useRef<number | undefined>(undefined);
  const pendingStreamScrollTimerRef = useRef<number | undefined>(undefined);
  const hasStreamScrolledRef = useRef(false);
  const lastStreamScrollAtRef = useRef(0);
  const pendingAnchorKeyRef = useRef('');
  const followOutputRef = useRef(true);
  const isAwayFromBottomRef = useRef(false);
  const wasSendingRef = useRef(isSending);
  const followConversationRef = useRef(conversationId);
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
        key: `message-${item.clientId}`,
        kind: 'message',
        messageIndex,
        item,
      });
    });
    if (activeAgent && shouldShowAgentWaitingBubble(messages, isSending, runPhase, retryStatus)) {
      next.push({ key: 'waiting', kind: 'waiting' });
    }
    return next;
  }, [activeAgent, hasOlderMessages, isSending, messages, retryStatus, runPhase]);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => {
      const row = rows[index];
      if (row?.kind === 'load_older') return LOAD_OLDER_ESTIMATE;
      if (row?.kind === 'waiting') return WAITING_ESTIMATE;
      return MESSAGE_ESTIMATE;
    },
    getItemKey: (index) => rows[index]?.key ?? index,
    anchorTo: 'end',
    // Streamed content changes row measurements without changing the row
    // count. Keep follow behavior in this component so a reader's scroll
    // position is never overridden by the virtualizer's implicit follow.
    followOnAppend: false,
    paddingEnd: isSending ? WAITING_SCROLL_PADDING : 0,
    scrollEndThreshold: BOTTOM_POSITION_TOLERANCE,
    overscan: 6,
  });

  useLayoutEffect(() => {
    virtualizer.shouldAdjustScrollPositionOnItemSizeChange = (item, _delta, instance) => {
      // Once the jump-to-latest button is visible, growing message bubbles
      // must not rewrite the reader's current scroll position.
      if (isAwayFromBottomRef.current) return false;

      // Preserve the virtualizer's normal anchoring for items above the
      // viewport while the conversation is following the live response.
      return item.start < (instance.scrollOffset ?? 0)
        && instance.scrollDirection !== 'backward';
    };
  }, [virtualizer]);

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
      isAwayFromBottomRef.current = false;
      setIsAwayFromBottom(false);
      return;
    }
    const isAwayFromBottom = element.scrollHeight > element.clientHeight + 8
      && !isMessagesNearBottom();
    isAwayFromBottomRef.current = isAwayFromBottom;
    setIsAwayFromBottom(isAwayFromBottom);
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
    followOutputRef.current = true;
    virtualizer.scrollToEnd({ behavior });
    const element = scrollRef.current;
    if (element) {
      const maxScrollTop = Math.max(element.scrollHeight - element.clientHeight, 0);
      if (behavior === 'auto') {
        element.scrollTop = maxScrollTop;
      } else {
        element.scrollTo({ top: maxScrollTop, behavior });
      }
    }
    isAwayFromBottomRef.current = false;
    setIsAwayFromBottom(false);
  }, [isMessagesNearBottom, virtualizer]);

  const scheduleStreamScroll = useCallback((immediate = false) => {
    if (
      !followOutputRef.current
      || isAwayFromBottomRef.current
      || !isMessagesNearBottom()
    ) return;

    const now = Date.now();
    const elapsed = hasStreamScrolledRef.current
      ? now - lastStreamScrollAtRef.current
      : STREAM_SCROLL_THROTTLE_MS;
    if (immediate || elapsed >= STREAM_SCROLL_THROTTLE_MS) {
      if (pendingStreamScrollTimerRef.current !== undefined) {
        window.clearTimeout(pendingStreamScrollTimerRef.current);
        pendingStreamScrollTimerRef.current = undefined;
      }
      hasStreamScrolledRef.current = true;
      lastStreamScrollAtRef.current = now;
      scrollToBottom(true);
      return;
    }

    if (pendingStreamScrollTimerRef.current !== undefined) return;
    pendingStreamScrollTimerRef.current = window.setTimeout(() => {
      pendingStreamScrollTimerRef.current = undefined;
      if (
        !followOutputRef.current
        || isAwayFromBottomRef.current
        || !isMessagesNearBottom()
      ) return;
      hasStreamScrolledRef.current = true;
      lastStreamScrollAtRef.current = Date.now();
      scrollToBottom(true);
    }, STREAM_SCROLL_THROTTLE_MS - elapsed);
  }, [isMessagesNearBottom, scrollToBottom]);

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
    const conversationChanged = followConversationRef.current !== conversationId;
    const sendJustStarted = isSending && !wasSendingRef.current;
    followConversationRef.current = conversationId;
    wasSendingRef.current = isSending;

    if (conversationChanged || sendJustStarted) {
      followOutputRef.current = true;
      isAwayFromBottomRef.current = false;
    }

    if (messages.length === 0 || !followOutputRef.current) return;

    // Keep the live response pinned without scrolling once per stream chunk.
    scheduleStreamScroll(!isSending || conversationChanged || sendJustStarted);
  }, [conversationId, isSending, messages, scheduleStreamScroll]);

  useEffect(() => () => {
    if (pendingStreamScrollTimerRef.current !== undefined) {
      window.clearTimeout(pendingStreamScrollTimerRef.current);
      pendingStreamScrollTimerRef.current = undefined;
    }
  }, []);

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
    // Depend on rows.length instead of rows: anchor positions only change when
    // rows are added/removed, so stream flushes (which replace the rows array
    // every ~32ms) must not rebuild the observer.
  }, [onActiveTimelineMessageChange, rows.length, timelineIndexSet]);

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
    const element = scrollRef.current;
    if (element) {
      const isAwayFromBottom = element.scrollHeight > element.clientHeight + 8
        && !isMessagesNearBottom();
      isAwayFromBottomRef.current = isAwayFromBottom;
      followOutputRef.current = !isAwayFromBottom;

      if (isAwayFromBottom) {
        if (pendingStreamScrollTimerRef.current !== undefined) {
          window.clearTimeout(pendingStreamScrollTimerRef.current);
          pendingStreamScrollTimerRef.current = undefined;
        }
      }
    }
    scheduleScrollButtonUpdate();
  }, [isMessagesNearBottom, scheduleScrollButtonUpdate]);

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
            description={activeAgent ? t('chat.emptyHint') : (
              <span className="empty-state-no-agent-hint">
                <ArrowLeft size={18} aria-hidden="true" />
                <span>{t('chat.noAgentHint')}</span>
              </span>
            )}
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
                  <WaitingBubble agent={activeAgent} settings={settings} retryStatus={retryStatus} runPhase={runPhase} />
                ) : (
                  <MessageListItem
                    item={row.item}
                    previous={messages[row.messageIndex - 1]}
                    activeAgent={activeAgent}
                    settings={settings}
                    isSending={isSending}
                    isLastMessage={row.messageIndex === messages.length - 1}
                    retryStatus={retryStatus}
                    runPhase={runPhase}
                    supportsImages={supportsImages}
                    supportsImageMasks={supportsImageMasks}
                    onRestart={onRestart}
                    onEditImage={onEditImage}
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
