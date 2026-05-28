import { useState } from 'react';
import type { ReactNode } from 'react';

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
  trigger: (open: boolean) => ReactNode;
  onChange: (next: { icon: AvatarIcon; color: string }) => void;
  className?: string;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div className={['avatar-popover', className].filter(Boolean).join(' ')}>
      <div onClick={() => setOpen((current) => !current)}>{trigger(open)}</div>
      {open ? (
        <>
          <div className="popover-backdrop" onClick={() => setOpen(false)} />
          <div className="avatar-popover-panel">
            <p className="avatar-popover-title">{pickerTitle}</p>
            <AvatarPicker icon={icon} color={color} title={title} onChange={onChange} />
          </div>
        </>
      ) : null}
    </div>
  );
}
