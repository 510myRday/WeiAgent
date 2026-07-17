import {
  memo,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Clock3 } from 'lucide-react';

export type MessageTimelineItem = {
  messageIndex: number;
  dateLabel: string;
  timeLabel: string;
  preview: string;
};

interface MessageTimelineProps {
  items: MessageTimelineItem[];
  activeMessageIndex: number | null;
  label: string;
  onSelect: (messageIndex: number) => void;
}

const ITEM_HEIGHT = 48;
const WHEEL_PADDING = 118;

export const MessageTimeline = memo(function MessageTimeline({
  items,
  activeMessageIndex,
  label,
  onSelect,
}: MessageTimelineProps) {
  const listboxId = useId();
  const wheelRef = useRef<HTMLDivElement>(null);
  const dragStateRef = useRef<{
    pointerId: number;
    startY: number;
    startScrollTop: number;
    moved: boolean;
  } | null>(null);
  const suppressClickRef = useRef(false);
  const [isDragging, setIsDragging] = useState(false);
  const activeItemIndex = useMemo(
    () => items.findIndex((item) => item.messageIndex === activeMessageIndex),
    [activeMessageIndex, items],
  );
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => wheelRef.current,
    estimateSize: () => ITEM_HEIGHT,
    getItemKey: (index) => items[index]?.messageIndex ?? index,
    overscan: 4,
    paddingStart: WHEEL_PADDING,
    paddingEnd: WHEEL_PADDING,
  });

  useEffect(() => {
    if (activeItemIndex < 0) return;
    virtualizer.scrollToIndex(activeItemIndex, {
      align: 'center',
      behavior: 'smooth',
    });
  }, [activeItemIndex, virtualizer]);

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.button !== 0 || !wheelRef.current) return;
    dragStateRef.current = {
      pointerId: event.pointerId,
      startY: event.clientY,
      startScrollTop: wheelRef.current.scrollTop,
      moved: false,
    };
    wheelRef.current.setPointerCapture(event.pointerId);
    setIsDragging(true);
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    const dragState = dragStateRef.current;
    const wheel = wheelRef.current;
    if (!dragState || !wheel || dragState.pointerId !== event.pointerId) return;
    const delta = event.clientY - dragState.startY;
    if (Math.abs(delta) > 3) dragState.moved = true;
    if (dragState.moved) event.preventDefault();
    wheel.scrollTop = dragState.startScrollTop - delta;
  }

  function finishPointerDrag(event: ReactPointerEvent<HTMLDivElement>) {
    const dragState = dragStateRef.current;
    const wheel = wheelRef.current;
    if (!dragState || !wheel || dragState.pointerId !== event.pointerId) return;
    if (wheel.hasPointerCapture(event.pointerId)) wheel.releasePointerCapture(event.pointerId);
    dragStateRef.current = null;
    setIsDragging(false);
    if (!dragState.moved) return;

    suppressClickRef.current = true;
    window.setTimeout(() => { suppressClickRef.current = false; }, 0);
    const centerOffset = wheel.scrollTop + (wheel.clientHeight / 2) - WHEEL_PADDING;
    const targetIndex = Math.max(
      0,
      Math.min(items.length - 1, Math.round((centerOffset - (ITEM_HEIGHT / 2)) / ITEM_HEIGHT)),
    );
    const item = items[targetIndex];
    if (item) onSelect(item.messageIndex);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    let nextIndex: number | null = null;
    const currentIndex = activeItemIndex >= 0 ? activeItemIndex : 0;
    if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
      nextIndex = Math.min(items.length - 1, currentIndex + 1);
    }
    if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
      nextIndex = Math.max(0, currentIndex - 1);
    }
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = items.length - 1;
    if (nextIndex === null || !items[nextIndex]) return;
    event.preventDefault();
    onSelect(items[nextIndex].messageIndex);
  }

  return (
    <aside className={`message-time-navigator${isDragging ? ' is-dragging' : ''}`} aria-label={label}>
      <div className="message-time-navigator__marker" aria-hidden="true">
        <Clock3 size={13} />
        <span />
      </div>
      <div
        className="message-time-wheel"
        ref={wheelRef}
        role="listbox"
        aria-label={label}
        aria-activedescendant={activeItemIndex >= 0
          ? `${listboxId}-option-${items[activeItemIndex].messageIndex}`
          : undefined}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={finishPointerDrag}
        onPointerCancel={finishPointerDrag}
      >
        <div className="message-time-virtual-list" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((virtualItem) => {
            const item = items[virtualItem.index];
            if (!item) return null;
            return (
              <button
                key={virtualItem.key}
                className={`message-time-option${item.messageIndex === activeMessageIndex ? ' is-active' : ''}`}
                type="button"
                role="option"
                id={`${listboxId}-option-${item.messageIndex}`}
                tabIndex={-1}
                aria-selected={item.messageIndex === activeMessageIndex}
                aria-setsize={items.length}
                aria-posinset={virtualItem.index + 1}
                title={item.preview || `${item.dateLabel} ${item.timeLabel}`}
                style={{ transform: `translateY(${virtualItem.start}px)` }}
                onClick={() => {
                  if (suppressClickRef.current) {
                    suppressClickRef.current = false;
                    return;
                  }
                  onSelect(item.messageIndex);
                  wheelRef.current?.focus();
                }}
              >
                <span>{item.dateLabel}</span>
                <strong>{item.timeLabel}</strong>
              </button>
            );
          })}
        </div>
      </div>
    </aside>
  );
});
