import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import { I18nProvider } from '../../i18n';
import { MarkdownContent } from './MarkdownContent';

function renderMarkdown(content: string) {
  return render(
    <I18nProvider>
      <MarkdownContent content={content} />
    </I18nProvider>,
  );
}

afterEach(() => {
  document.head.querySelector('meta[name="weiagent-api-token"]')?.remove();
  vi.restoreAllMocks();
});

test('loads a local Windows image emitted in Markdown', async () => {
  const tokenMeta = document.createElement('meta');
  tokenMeta.name = 'weiagent-api-token';
  tokenMeta.content = 'test-token';
  document.head.appendChild(tokenMeta);

  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(new Blob(['png-data'], { type: 'image/png' }), { status: 200 }),
  );
  vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:test-image');

  renderMarkdown(String.raw`![Dashi PPT 风格图](C:\\Users\\Administrator\\.WeiAgent\\skills\\dashi-ppt\\assets\\skill\\theme-style-grid.png)`);

  const image = await screen.findByRole('img', { name: 'Dashi PPT 风格图' });
  expect(image).toHaveAttribute('src', 'blob:test-image');

  const [requestUrl, requestInit] = fetchMock.mock.calls[0];
  const parsedUrl = new URL(String(requestUrl), window.location.origin);
  expect(parsedUrl.pathname).toBe('/api/assets/local');
  expect(parsedUrl.searchParams.get('path')).toBe(
    'C:\\Users\\Administrator\\.WeiAgent\\skills\\dashi-ppt\\assets\\skill\\theme-style-grid.png',
  );
  expect(parsedUrl.searchParams.get('token')).toBeNull();
  expect(requestInit).toMatchObject({
    credentials: 'same-origin',
    headers: { Authorization: 'Bearer test-token' },
  });
});

test('loads a local Windows image with single backslashes emitted in Markdown', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(new Blob(['png-data'], { type: 'image/png' }), { status: 200 }),
  );
  vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:single-slash-image');

  renderMarkdown(String.raw`![Windows image](C:\Users\Administrator\.WeiAgent\skills\dashi-ppt\assets\skill\theme-style-grid.png)`);

  expect(await screen.findByRole('img', { name: 'Windows image' })).toHaveAttribute(
    'src',
    'blob:single-slash-image',
  );
});

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
