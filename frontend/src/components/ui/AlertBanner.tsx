import { X } from 'lucide-react';

import { IconButton } from './Button';

export function AlertBanner({
  message,
  onDismiss,
  dismissLabel,
  global = false,
}: {
  message: string;
  onDismiss: () => void;
  dismissLabel: string;
  global?: boolean;
}) {
  return (
    <div className={['alert-banner', global ? 'alert-banner-global' : ''].filter(Boolean).join(' ')}>
      <span>{message}</span>
      <IconButton className="alert-dismiss" variant="ghost" label={dismissLabel} type="button" onClick={onDismiss}>
        <X size={16} />
      </IconButton>
    </div>
  );
}
