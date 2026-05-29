import type { ReactNode } from 'react';

export function FormField({
  label,
  htmlFor,
  action,
  required = false,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  action?: ReactNode;
  required?: boolean;
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
    </div>
  );
}
