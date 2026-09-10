import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, test } from 'vitest';

import { I18nProvider } from '../../i18n';
import { FeedbackProvider } from '../../components/ui';
import { MarkdownCodeBlock } from './MarkdownCodeBlock';
import { HtmlPreviewProvider } from './HtmlPreviewProvider';

function renderCode(className: string, content: string) {
  return render(
    <I18nProvider>
      <FeedbackProvider>
        <HtmlPreviewProvider>
          <MarkdownCodeBlock>
            <code className={className}>{content}</code>
          </MarkdownCodeBlock>
        </HtmlPreviewProvider>
      </FeedbackProvider>
    </I18nProvider>,
  );
}

describe('MarkdownCodeBlock', () => {
  test('adds an HTML render action and isolates the preview in an iframe', () => {
    const html = '<!doctype html><button>Run</button>';
    renderCode('language-html', html);

    expect(screen.getByRole('button', { name: '渲染 HTML' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '复制代码' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '渲染 HTML' }));

    const frame = screen.getByTitle('HTML 渲染预览');
    expect(frame).toHaveAttribute('srcdoc', html);
    expect(frame).toHaveAttribute('sandbox', 'allow-forms allow-modals allow-popups allow-scripts');
  });

  test('minimizes to a restore button and closes the preview', () => {
    renderCode('language-html', '<main>Preview</main>');
    fireEvent.click(screen.getByRole('button', { name: '渲染 HTML' }));

    fireEvent.click(screen.getByRole('button', { name: '缩小预览' }));
    const minimizedFrame = screen.getByTitle('HTML 渲染预览');
    expect(minimizedFrame).toBeInTheDocument();
    expect(minimizedFrame.parentElement).toHaveClass('html-preview-window', 'is-minimized');
    expect(screen.getByRole('button', { name: '展开 HTML 预览' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '展开 HTML 预览' }));
    expect(screen.getByTitle('HTML 渲染预览')).toBeInTheDocument();

    fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: '关闭 HTML 预览' }));
    expect(screen.queryByTitle('HTML 渲染预览')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '展开 HTML 预览' })).not.toBeInTheDocument();
  });

  test('does not add the render action to non-HTML code', () => {
    renderCode('language-javascript', 'console.log(1);');

    expect(screen.queryByRole('button', { name: '渲染 HTML' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '复制代码' })).toBeInTheDocument();
  });

  test('keeps up to five minimized previews and warns when the sixth is minimized', async () => {
    renderCode('language-html', '<main>Preview</main>');
    const renderButton = () => screen.getByRole('button', { name: '渲染 HTML' });

    for (let index = 0; index < 5; index += 1) {
      fireEvent.click(renderButton());
      fireEvent.click(screen.getByRole('button', { name: '缩小预览' }));
    }

    expect(document.querySelectorAll('.html-preview-window.is-minimized iframe')).toHaveLength(5);
    expect(screen.getAllByTitle('HTML 渲染预览')).toHaveLength(5);

    fireEvent.click(renderButton());
    fireEvent.click(screen.getByRole('button', { name: '缩小预览' }));

    expect(document.querySelectorAll('.html-preview-window.is-minimized iframe')).toHaveLength(5);
    expect(screen.getAllByTitle('HTML 渲染预览')).toHaveLength(6);
    await waitFor(() => {
      expect(screen.getByText('最多只能同时保留 5 个 HTML 预览窗口')).toBeInTheDocument();
    });
  });
});
