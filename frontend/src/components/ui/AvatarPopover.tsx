import { cloneElement, useEffect, useId, useRef, useState } from 'react';
import type { MouseEventHandler, ReactElement } from 'react';

import type { AvatarIcon } from '../../types';
import { AvatarPicker } from '../AvatarPicker';

export function AvatarPopover({
  icon,
  color,
  title,
  pickerTitle,
  trigger,
  onChange,
  className = '',
}: {
  icon: AvatarIcon;
  color: string;
  title: string;
  pickerTitle: string;
  trigger: (open: boolean) => ReactElement;
  onChange: (next: { icon: AvatarIcon; color: string }) => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const triggerElement = trigger(open);
  const clickableTrigger = triggerElement as ReactElement<{
    onClick?: MouseEventHandler;
    'aria-controls'?: string;
    'aria-expanded'?: boolean;
    'aria-haspopup'?: 'dialog';
  }>;
  const triggerWithToggle = cloneElement(
    clickableTrigger,
    {
      'aria-controls': panelId,
      'aria-expanded': open,
      'aria-haspopup': 'dialog',
      onClick: (event) => {
        clickableTrigger.props.onClick?.(event);
        if (!event.defaultPrevented) setOpen((current) => !current);
      },
    },
  );

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: PointerEvent) => {
      if (!popoverRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', handlePointerDown, true);
    window.addEventListener('keydown', handleEscape);
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown, true);
      window.removeEventListener('keydown', handleEscape);
    };
  }, [open]);

  return (
    <div ref={popoverRef} className={['avatar-popover', className].filter(Boolean).join(' ')}>
      {triggerWithToggle}
      {open ? (
        <div id={panelId} className="avatar-popover-panel" role="dialog" aria-label={pickerTitle}>
          <p className="avatar-popover-title">{pickerTitle}</p>
          <AvatarPicker icon={icon} color={color} title={title} onChange={onChange} />
        </div>
      ) : null}
    </div>
  );
}
