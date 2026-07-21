import type { ReactNode } from 'react';

export function FormField({
  label,
  htmlFor,
  action,
  required = false,
  hint,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  action?: ReactNode;
  required?: boolean;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="form-field">
      <label htmlFor={htmlFor}>
        <span>
          {label}
          {required ? <span className="required-marker" aria-hidden="true">*</span> : null}
        </span>
        {action}
      </label>
      {children}
      {hint ? <p className="form-field-hint">{hint}</p> : null}
    </div>
  );
}
