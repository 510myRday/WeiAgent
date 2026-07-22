import type { TableHTMLAttributes } from 'react';

export function MarkdownTable(props: TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className="markdown-table-wrap" role="region" aria-label="Scrollable Markdown table" tabIndex={0}>
      <table {...props} />
    </div>
  );
}
