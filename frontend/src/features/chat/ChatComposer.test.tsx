import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

import { FeedbackProvider } from '../../components/ui';
import { I18nProvider } from '../../i18n';
import { ChatComposer } from './ChatComposer';

const usage = {
  input_tokens: 0,
  output_tokens: 0,
  cache_write_tokens: 0,
  cache_read_tokens: 0,
  total_tokens: 0,
  cost_usd: null,
};

beforeEach(() => {
  localStorage.setItem('weiagent-locale', 'zh');
});

function renderComposer() {
  return render(
    <I18nProvider>
      <FeedbackProvider>
        <ChatComposer
          disabled={false}
          placeholder="输入消息"
          isSending={false}
          isSavingModel={false}
          isModelSavePending={false}
          draftKey="chat-composer-test"
          isStreamMode
          supportsImages
          supportsThinking
          thinkingLevel="medium"
          usage={usage}
          contextSize={128000}
          onStreamModeChange={vi.fn()}
          onThinkingLevelChange={vi.fn()}
          onUnsupportedImage={vi.fn()}
          onSend={vi.fn()}
          onStop={vi.fn()}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );
}

test('shows the thinking prefix beside the brain icon', () => {
  renderComposer();

  expect(screen.getByRole('button', { name: '思考' })).toHaveTextContent('思考·中');
});

test('adds an image dropped onto the composer', async () => {
  const { container } = renderComposer();
  const composerBox = container.querySelector('.composer-box');
  const image = new File(['image-data'], 'dropped.png', { type: 'image/png' });
  const dataTransfer = {
    types: ['Files'],
    files: [image],
    dropEffect: 'none',
  };

  expect(composerBox).not.toBeNull();
  fireEvent.dragEnter(composerBox!, { dataTransfer });
  expect(composerBox).toHaveClass('is-dragging-files');

  fireEvent.drop(composerBox!, { dataTransfer });

  await waitFor(() => {
    expect(screen.getByRole('img', { name: 'dropped.png' })).toBeVisible();
  });
  expect(composerBox).not.toHaveClass('is-dragging-files');
});
