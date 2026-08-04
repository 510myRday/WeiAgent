import type { TableHTMLAttributes } from 'react';
import { useI18n } from '../../i18n';

export function MarkdownTable(props: TableHTMLAttributes<HTMLTableElement>) {
  const { t } = useI18n();

  return (
    <div className="markdown-table-wrap" role="region" aria-label={t('chat.scrollableTable')} tabIndex={0}>
      <table {...props} />
    </div>
  );
}
