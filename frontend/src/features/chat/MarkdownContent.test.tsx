import { render, screen } from '@testing-library/react';
import { describe, expect, test } from 'vitest';

import { MarkdownContent } from './MarkdownContent';

describe('MarkdownContent tables', () => {
  test('renders GFM tables inside a horizontal scroll container', () => {
    const { container } = render(
      <MarkdownContent content={'| 维度 | 评分 |\n| --- | ---: |\n| 编码性能 | 8 |'} />,
    );

    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getByText('编码性能')).toBeInTheDocument();
    expect(container.querySelector('.markdown-table-wrap')).toContainElement(
      screen.getByRole('table'),
    );
  });
});

describe('MarkdownContent links', () => {
  test('opens rendered Markdown links in a new tab', () => {
    render(<MarkdownContent content="[币安公告](https://example.com/announcement)" />);

    expect(screen.getByRole('link', { name: '币安公告' })).toHaveAttribute(
      'href',
      'https://example.com/announcement',
    );
    expect(screen.getByRole('link', { name: '币安公告' })).toHaveAttribute('target', '_blank');
    expect(screen.getByRole('link', { name: '币安公告' })).toHaveAttribute(
      'rel',
      'noopener noreferrer',
    );
  });

  test('opens links in a new tab when the message also contains math', async () => {
    render(<MarkdownContent content={'价格为 $x$，查看[合约教程](https://example.com/tutorial)'} />);

    expect(await screen.findByRole('link', { name: '合约教程' })).toHaveAttribute(
      'target',
      '_blank',
    );
  });
});
