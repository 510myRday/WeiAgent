import type { ReactNode } from 'react';

export function PanelShell({
  sidebar,
  main,
  className = '',
}: {
  sidebar: ReactNode;
  main: ReactNode;
  className?: string;
}) {
  return (
    <section className={['panel-shell', className].filter(Boolean).join(' ')}>
      {sidebar}
      {main}
    </section>
  );
}
