import {
  useEffect,
  useId,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

import { IconButton } from './Button';

export type DialogSize = 'sm' | 'md' | 'lg' | 'xl';

interface DialogProps {
  open: boolean;
  title: ReactNode;
  description?: ReactNode;
  icon?: ReactNode;
  children?: ReactNode;
  footer?: ReactNode;
  headerActions?: ReactNode;
  closeLabel: string;
  onClose?: () => void;
  size?: DialogSize;
  role?: 'dialog' | 'alertdialog';
  closeOnBackdrop?: boolean;
  closeOnEscape?: boolean;
  className?: string;
  bodyClassName?: string;
}

const focusableSelector = [
  'button:not([disabled])',
  '[href]',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function Dialog({
  open,
  title,
  description,
  icon,
  children,
  footer,
  headerActions,
  closeLabel,
  onClose,
  size = 'md',
  role = 'dialog',
  closeOnBackdrop = true,
  closeOnEscape = true,
  className = '',
  bodyClassName = '',
}: DialogProps) {
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const backdropRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;

    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousOverflow = document.body.style.overflow;
    const backgroundNodes = Array.from(document.body.children)
      .filter((node): node is HTMLElement => node instanceof HTMLElement && node !== backdropRef.current)
      .map((node) => ({
        node,
        hadInert: node.hasAttribute('inert'),
        hadAriaHidden: node.getAttribute('aria-hidden'),
      }));
    document.body.style.overflow = 'hidden';
    backgroundNodes.forEach(({ node }) => {
      node.setAttribute('inert', '');
      node.setAttribute('aria-hidden', 'true');
    });

    const focusDialog = window.requestAnimationFrame(() => {
      const firstFocusable = dialogRef.current?.querySelector<HTMLElement>(focusableSelector);
      (firstFocusable ?? dialogRef.current)?.focus();
    });

    return () => {
      window.cancelAnimationFrame(focusDialog);
      document.body.style.overflow = previousOverflow;
      backgroundNodes.forEach(({ node, hadInert, hadAriaHidden }) => {
        if (!hadInert) node.removeAttribute('inert');
        if (hadAriaHidden === null) node.removeAttribute('aria-hidden');
        else node.setAttribute('aria-hidden', hadAriaHidden);
      });
      previousFocus?.focus();
    };
  }, [open]);

  if (!open) return null;

  function handleKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Escape' && closeOnEscape && onClose) {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== 'Tab') return;

    const focusable = Array.from(
      dialogRef.current?.querySelectorAll<HTMLElement>(focusableSelector) ?? [],
    );
    if (focusable.length === 0) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }

    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return createPortal(
    <div
      ref={backdropRef}
      className="dialog-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && closeOnBackdrop) onClose?.();
      }}
    >
      <div
        ref={dialogRef}
        className={['dialog', `dialog-${size}`, className].filter(Boolean).join(' ')}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        tabIndex={-1}
        onKeyDown={handleKeyDown}
      >
        <header className="dialog-header">
          <div className="dialog-heading">
            {icon ? <span className="dialog-icon">{icon}</span> : null}
            <div className="dialog-heading-copy">
              <h2 id={titleId}>{title}</h2>
              {description ? <p id={descriptionId}>{description}</p> : null}
            </div>
          </div>
          <div className="dialog-header-actions">
            {headerActions}
            {onClose ? (
              <IconButton variant="ghost" type="button" onClick={onClose} label={closeLabel}>
                <X size={19} />
              </IconButton>
            ) : null}
          </div>
        </header>
        {children ? <div className={['dialog-body', bodyClassName].filter(Boolean).join(' ')}>{children}</div> : null}
        {footer ? <footer className="dialog-footer">{footer}</footer> : null}
      </div>
    </div>,
    document.body,
  );
}
