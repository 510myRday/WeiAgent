import type { ReactNode } from 'react';

export function FormField({
  label,
  htmlFor,
  action,
  children,
}: {
  label: ReactNode;
  htmlFor?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="form-field">
      <label htmlFor={htmlFor}>
        <span>{label}</span>
        {action}
      </label>
      {children}
    </div>
  );
}
