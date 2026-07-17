import type { KeyboardEvent } from 'react';
import { Hand, ShieldAlert } from 'lucide-react';

export type PermissionMode = 'request_approval' | 'full_access';

interface PermissionModeControlProps {
  value: PermissionMode;
  label: string;
  requestApprovalLabel: string;
  requestApprovalDescription: string;
  fullAccessLabel: string;
  fullAccessDescription: string;
  requestApprovalDisabled: boolean;
  fullAccessDisabled: boolean;
  onChange: (mode: PermissionMode) => void;
}

export function PermissionModeControl({
  value,
  label,
  requestApprovalLabel,
  requestApprovalDescription,
  fullAccessLabel,
  fullAccessDescription,
  requestApprovalDisabled,
  fullAccessDisabled,
  onChange,
}: PermissionModeControlProps) {
  const options = [
    { mode: 'request_approval' as const, disabled: requestApprovalDisabled },
    { mode: 'full_access' as const, disabled: fullAccessDisabled },
  ];

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (!['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      return;
    }
    event.preventDefault();
    const enabled = options.filter((option) => !option.disabled);
    if (enabled.length === 0) return;
    const currentIndex = Math.max(0, enabled.findIndex((option) => option.mode === value));
    const nextIndex = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? enabled.length - 1
        : event.key === 'ArrowRight' || event.key === 'ArrowDown'
          ? (currentIndex + 1) % enabled.length
          : (currentIndex - 1 + enabled.length) % enabled.length;
    const nextMode = enabled[nextIndex].mode;
    onChange(nextMode);
    event.currentTarget.querySelector<HTMLElement>(`[data-permission-mode="${nextMode}"]`)?.focus();
  }

  return (
    <div
      className="permission-mode-control"
      role="radiogroup"
      aria-label={label}
      tabIndex={-1}
      onKeyDown={handleKeyDown}
    >
      <button
        type="button"
        role="radio"
        data-permission-mode="request_approval"
        disabled={requestApprovalDisabled}
        tabIndex={value === 'request_approval' ? 0 : -1}
        aria-checked={value === 'request_approval'}
        className={`${value === 'request_approval' ? 'is-active' : ''}${requestApprovalDisabled ? ' is-disabled' : ''}`}
        onClick={() => onChange('request_approval')}
      >
        <Hand size={20} />
        <span><strong>{requestApprovalLabel}</strong><small>{requestApprovalDescription}</small></span>
      </button>
      <button
        type="button"
        role="radio"
        data-permission-mode="full_access"
        disabled={fullAccessDisabled}
        tabIndex={value === 'full_access' ? 0 : -1}
        aria-checked={value === 'full_access'}
        className={value === 'full_access' ? 'is-active is-danger' : ''}
        onClick={() => onChange('full_access')}
      >
        <ShieldAlert size={20} />
        <span><strong>{fullAccessLabel}</strong><small>{fullAccessDescription}</small></span>
      </button>
    </div>
  );
}
