import type { ButtonHTMLAttributes, ReactNode } from 'react';

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  icon?: ReactNode;
};

export function Button({
  variant = 'secondary',
  icon,
  className = '',
  children,
  ...props
}: ButtonProps) {
  return (
    <button className={['button', `button-${variant}`, className].filter(Boolean).join(' ')} {...props}>
      {icon}
      {children ? <span>{children}</span> : null}
    </button>
  );
}

type IconButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'default' | 'ghost' | 'danger' | 'rail';
  label: string;
  active?: boolean;
};

export function IconButton({
  variant = 'default',
  label,
  active = false,
  className = '',
  children,
  title,
  ...props
}: IconButtonProps) {
  return (
    <button
      className={[
        'icon-button',
        `icon-button-${variant}`,
        active ? 'is-active' : '',
        className,
      ].filter(Boolean).join(' ')}
      title={title ?? label}
      aria-label={label}
      {...props}
    >
      {children}
    </button>
  );
}
