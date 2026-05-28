import type { ReactNode } from 'react';

type PaneHeaderProps = {
  title: ReactNode;
  description?: ReactNode;
  eyebrow?: ReactNode;
  leading?: ReactNode;
  actions?: ReactNode;
  compact?: boolean;
  className?: string;
};

export function PaneHeader({
  title,
  description,
  eyebrow,
  leading,
  actions,
  compact = false,
  className = '',
}: PaneHeaderProps) {
  return (
    <header className={['pane-header', compact ? 'pane-header-compact' : '', className].filter(Boolean).join(' ')}>
      {leading}
      <div className="pane-title-block">
        {eyebrow ? <p className="pane-eyebrow">{eyebrow}</p> : null}
        <h2>{title}</h2>
        {description ? <p>{description}</p> : null}
      </div>
      {actions ? <div className="pane-actions">{actions}</div> : null}
    </header>
  );
}
