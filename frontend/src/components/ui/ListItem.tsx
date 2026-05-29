import type { ButtonHTMLAttributes, ReactNode } from 'react';

type ListItemProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'title'> & {
  title: ReactNode;
  subtitle?: ReactNode;
  leading?: ReactNode;
  trailing?: ReactNode;
  active?: boolean;
};

export function ListItem({
  title,
  subtitle,
  leading,
  trailing,
  active = false,
  className = '',
  ...props
}: ListItemProps) {
  return (
    <button className={['list-item', active ? 'is-active' : '', className].filter(Boolean).join(' ')} {...props}>
      {leading ? <span className="list-item-leading">{leading}</span> : null}
      <span className="list-item-body">
        <strong>{title}</strong>
        {subtitle ? <small>{subtitle}</small> : null}
      </span>
      {trailing ? <span className="list-item-trailing">{trailing}</span> : null}
    </button>
  );
}
