import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test } from 'vitest';

import { I18nProvider } from '../../i18n';
import { MarkdownContent } from './MarkdownContent';

function renderMarkdown(content: string) {
  return render(
    <I18nProvider>
      <MarkdownContent content={content} />
    </I18nProvider>,
  );
}

describe('MarkdownContent tables', () => {
  test('renders GFM tables inside a horizontal scroll container', async () => {
    const { container } = renderMarkdown('| 维度 | 评分 |\n| --- | ---: |\n| 编码性能 | 8 |');

    await waitFor(() => expect(container.querySelector('table')).toBeInTheDocument());
    const table = container.querySelector('table');
    expect(table).toBeInTheDocument();
    expect(screen.getByText('编码性能')).toBeInTheDocument();
    expect(container.querySelector('.markdown-table-wrap')).toContainElement(
      table,
    );
  });
});

describe('MarkdownContent links', () => {
  test('opens rendered Markdown links in a new tab', async () => {
    renderMarkdown('[币安公告](https://example.com/announcement)');

    const link = await screen.findByRole('link', { name: '币安公告' });
    expect(link).toHaveAttribute(
      'href',
      'https://example.com/announcement',
    );
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute(
      'rel',
      'noopener noreferrer',
    );
  });

  test('opens links in a new tab when the message also contains math', async () => {
    renderMarkdown('价格为 $x$，查看[合约教程](https://example.com/tutorial)');

    expect(await screen.findByRole('link', { name: '合约教程' })).toHaveAttribute(
      'target',
      '_blank',
    );
  });
});

describe('MarkdownContent line breaks', () => {
  test('keeps a single newline in the rendered paragraph', async () => {
    const { container } = renderMarkdown('哈哈哈哈\n谢谢');

    await waitFor(() => expect(container.querySelector('p')).toBeInTheDocument());
    expect(container.querySelector('p')?.textContent).toBe('哈哈哈哈\n谢谢');
  });
});
