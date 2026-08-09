import { cloneElement, useId, useState } from 'react';
import type { MouseEventHandler, ReactElement } from 'react';

import type { AvatarIcon } from '../../types';
import { AvatarPicker } from '../AvatarPicker';
import { useDismissiblePopover } from './useDismissiblePopover';

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
  const popoverRef = useDismissiblePopover<HTMLDivElement>(open, () => setOpen(false));
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
