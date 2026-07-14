import { useEffect, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { CheckCircle2, CircleAlert, Info, TriangleAlert, X } from 'lucide-react';

import { IconButton } from './Button';

export type ToastTone = 'success' | 'error' | 'warning' | 'info';

export interface ToastItem {
  id: number;
  tone: ToastTone;
  title?: ReactNode;
  message: ReactNode;
  duration: number;
}

interface ToastViewportProps {
  items: ToastItem[];
  label: string;
  closeLabel: string;
  onDismiss: (id: number) => void;
}

const toneIcons = {
  success: CheckCircle2,
  error: CircleAlert,
  warning: TriangleAlert,
  info: Info,
};

function Toast({ item, closeLabel, onDismiss }: {
  item: ToastItem;
  closeLabel: string;
  onDismiss: (id: number) => void;
}) {
  const Icon = toneIcons[item.tone];

  useEffect(() => {
    if (item.duration <= 0) return;
    const timer = window.setTimeout(() => onDismiss(item.id), item.duration);
    return () => window.clearTimeout(timer);
  }, [item.duration, item.id, onDismiss]);

  return (
    <article
      className={`toast toast-${item.tone}`}
      role={item.tone === 'error' ? 'alert' : 'status'}
      aria-live={item.tone === 'error' ? 'assertive' : 'polite'}
    >
      <span className="toast-icon"><Icon size={19} /></span>
      <div className="toast-content">
        {item.title ? <strong>{item.title}</strong> : null}
        <div>{item.message}</div>
      </div>
      <IconButton className="toast-dismiss" variant="ghost" type="button" onClick={() => onDismiss(item.id)} label={closeLabel}>
        <X size={16} />
      </IconButton>
    </article>
  );
}

export function ToastViewport({ items, label, closeLabel, onDismiss }: ToastViewportProps) {
  if (items.length === 0) return null;

  return createPortal(
    <div className="toast-viewport" aria-label={label}>
      {items.map((item) => (
        <Toast key={item.id} item={item} closeLabel={closeLabel} onDismiss={onDismiss} />
      ))}
    </div>,
    document.body,
  );
}
