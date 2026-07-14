import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { CircleAlert, HelpCircle, ShieldAlert } from 'lucide-react';

import { useI18n } from '../../i18n';
import { Button } from './Button';
import { Dialog } from './Dialog';
import { ToastViewport, type ToastItem, type ToastTone } from './ToastViewport';

export type ConfirmTone = 'default' | 'danger' | 'warning';

export interface ConfirmOptions {
  title?: ReactNode;
  message: ReactNode;
  description?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: ConfirmTone;
}

export interface ToastOptions {
  title?: ReactNode;
  duration?: number;
}

interface ConfirmRequest extends ConfirmOptions {
  id: number;
  resolve: (confirmed: boolean) => void;
}

interface ToastApi {
  show: (message: ReactNode, tone?: ToastTone, options?: ToastOptions) => number;
  success: (message: ReactNode, options?: ToastOptions) => number;
  error: (message: ReactNode, options?: ToastOptions) => number;
  warning: (message: ReactNode, options?: ToastOptions) => number;
  info: (message: ReactNode, options?: ToastOptions) => number;
  dismiss: (id: number) => void;
  clear: () => void;
}

interface FeedbackContextValue {
  confirm: (options: ConfirmOptions) => Promise<boolean>;
  toast: ToastApi;
}

const FeedbackContext = createContext<FeedbackContextValue | null>(null);

const confirmIcons = {
  default: HelpCircle,
  danger: CircleAlert,
  warning: ShieldAlert,
};

export function FeedbackProvider({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  const nextId = useRef(1);
  const [confirmQueue, setConfirmQueue] = useState<ConfirmRequest[]>([]);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const activeConfirm = confirmQueue[0];

  const confirm = useCallback((options: ConfirmOptions) => (
    new Promise<boolean>((resolve) => {
      setConfirmQueue((current) => [
        ...current,
        { ...options, id: nextId.current++, resolve },
      ]);
    })
  ), []);

  const settleConfirm = useCallback((confirmed: boolean) => {
    if (!activeConfirm) return;
    activeConfirm.resolve(confirmed);
    setConfirmQueue((current) => (
      current[0]?.id === activeConfirm.id ? current.slice(1) : current
    ));
  }, [activeConfirm]);

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((item) => item.id !== id));
  }, []);

  const show = useCallback((
    message: ReactNode,
    tone: ToastTone = 'info',
    options: ToastOptions = {},
  ) => {
    const id = nextId.current++;
    const duration = options.duration ?? (tone === 'error' ? 6000 : 3600);
    setToasts((current) => [
      ...current.slice(-4),
      { id, tone, message, title: options.title, duration },
    ]);
    return id;
  }, []);

  const toast = useMemo<ToastApi>(() => ({
    show,
    success: (message, options) => show(message, 'success', options),
    error: (message, options) => show(message, 'error', options),
    warning: (message, options) => show(message, 'warning', options),
    info: (message, options) => show(message, 'info', options),
    dismiss,
    clear: () => setToasts([]),
  }), [dismiss, show]);

  const value = useMemo(() => ({ confirm, toast }), [confirm, toast]);
  const confirmTone = activeConfirm?.tone ?? 'default';
  const ConfirmIcon = confirmIcons[confirmTone];

  return (
    <FeedbackContext.Provider value={value}>
      {children}
      <Dialog
        open={Boolean(activeConfirm)}
        title={activeConfirm?.title ?? t('dialog.confirmTitle')}
        description={activeConfirm?.description}
        icon={<ConfirmIcon size={21} />}
        closeLabel={activeConfirm?.cancelLabel ?? t('common.cancel')}
        onClose={() => settleConfirm(false)}
        size="sm"
        role="alertdialog"
        closeOnBackdrop={false}
        className={`confirm-dialog confirm-dialog-${confirmTone}`}
        footer={(
          <>
            <Button type="button" variant="secondary" onClick={() => settleConfirm(false)}>
              {activeConfirm?.cancelLabel ?? t('common.cancel')}
            </Button>
            <Button
              type="button"
              variant={confirmTone === 'danger' ? 'danger' : 'primary'}
              onClick={() => settleConfirm(true)}
            >
              {activeConfirm?.confirmLabel ?? t('common.confirm')}
            </Button>
          </>
        )}
      >
        <div className="confirm-dialog-message">
          {activeConfirm?.message}
        </div>
      </Dialog>
      <ToastViewport
        items={toasts}
        label={t('notification.label')}
        closeLabel={t('common.close')}
        onDismiss={dismiss}
      />
    </FeedbackContext.Provider>
  );
}

export function useFeedback(): FeedbackContextValue {
  const context = useContext(FeedbackContext);
  if (!context) throw new Error('useFeedback must be used within <FeedbackProvider>');
  return context;
}
