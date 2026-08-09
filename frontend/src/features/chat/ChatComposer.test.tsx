import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';
import type { ComponentProps } from 'react';

import { FeedbackProvider } from '../../components/ui';
import { I18nProvider } from '../../i18n';
import { CUSTOM_IMAGE_SIZE } from '../../components/ImageSizeControl';
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

function renderComposer(overrides: Partial<ComponentProps<typeof ChatComposer>> = {}) {
  return render(
    <I18nProvider>
      <FeedbackProvider>
        <ChatComposer
          disabled={false}
          placeholder="输入消息"
          inputLabel="消息"
          isSending={false}
          isSavingModel={false}
          isModelSavePending={false}
          draftKey="chat-composer-test"
          supportsImages
          supportsImageGeneration={false}
          generationMode="chat"
          imageOptions={{}}
          isSavingImageOptions={false}
          supportsThinking
          thinkingLevel="medium"
          usage={usage}
          contextSize={128000}
          onGenerationModeChange={vi.fn()}
          onImageOptionsChange={vi.fn()}
          onThinkingLevelChange={vi.fn()}
          onUnsupportedImage={vi.fn()}
          onSend={vi.fn()}
          onStop={vi.fn()}
          {...overrides}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );
}

test('locks image mode on and hides streaming configuration for an image provider', () => {
  renderComposer({
    inputLabel: '图片提示词',
    placeholder: '描述你想生成的画面，可添加参考图进行图生图',
    supportsImages: false,
    supportsImageGeneration: true,
    generationMode: 'image',
  });

  const imageMode = screen.getByRole('button', { name: '图片模式' });
  expect(imageMode).toHaveAttribute('aria-pressed', 'true');
  expect(imageMode).toBeDisabled();

  expect(screen.queryByRole('switch', { name: '流式' })).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: '图片提示词' })).toHaveAttribute(
    'placeholder',
    '描述你想生成的画面，可添加参考图进行图生图',
  );
});

test('shows the thinking prefix beside the brain icon', () => {
  renderComposer();

  expect(screen.getByRole('button', { name: '思考' })).toHaveTextContent('思考·中');
});

test('shows and updates persisted image settings for an image provider', () => {
  const onImageOptionsChange = vi.fn();
  renderComposer({
    supportsImageGeneration: true,
    generationMode: 'image',
    imageOptions: { size: '1024x1024', quality: 'high', n: 2 },
    onImageOptionsChange,
  });

  expect(screen.getByRole('combobox', { name: '尺寸' })).toHaveValue('1024x1024');
  expect(screen.getByRole('combobox', { name: '质量' })).toHaveValue('high');
  expect(screen.getByRole('combobox', { name: '数量' })).toHaveValue('2');

  fireEvent.change(screen.getByRole('combobox', { name: '质量' }), {
    target: { value: 'medium' },
  });
  expect(onImageOptionsChange).toHaveBeenCalledWith({ quality: 'medium' });
});

test('accepts a custom image size in the size control and persists valid dimensions', () => {
  const onImageOptionsChange = vi.fn();
  renderComposer({
    supportsImageGeneration: true,
    generationMode: 'image',
    imageOptions: { size: '1024x1024' },
    onImageOptionsChange,
  });

  const size = screen.getByRole('combobox', { name: '尺寸' });
  fireEvent.change(size, { target: { value: CUSTOM_IMAGE_SIZE } });
  const dialog = screen.getByRole('dialog', { name: '自定义尺寸' });
  const customSize = within(dialog).getByRole('textbox', { name: '像素尺寸' });
  fireEvent.change(customSize, { target: { value: '2048x1536' } });
  fireEvent.click(within(dialog).getByRole('button', { name: '确认' }));

  expect(onImageOptionsChange).toHaveBeenLastCalledWith({ size: '2048x1536' });
  expect(screen.queryByRole('dialog', { name: '自定义尺寸' })).not.toBeInTheDocument();
});

test('accepts a custom aspect ratio in the size control', () => {
  const onImageOptionsChange = vi.fn();
  renderComposer({
    supportsImageGeneration: true,
    generationMode: 'image',
    onImageOptionsChange,
  });

  const size = screen.getByRole('combobox', { name: '尺寸' });
  fireEvent.change(size, { target: { value: CUSTOM_IMAGE_SIZE } });
  const dialog = screen.getByRole('dialog', { name: '自定义尺寸' });
  fireEvent.click(within(dialog).getByRole('tab', { name: '宽高比' }));
  const customRatio = within(dialog).getByRole('textbox', { name: '宽高比' });
  fireEvent.change(customRatio, { target: { value: '5:4' } });
  fireEvent.click(within(dialog).getByRole('button', { name: '确认' }));

  expect(onImageOptionsChange).toHaveBeenLastCalledWith({ size: '5:4' });
});

test('keeps no-value and custom entries in the native image size select', () => {
  const onImageOptionsChange = vi.fn();
  renderComposer({
    supportsImageGeneration: true,
    generationMode: 'image',
    imageOptions: { size: '1024x1024' },
    onImageOptionsChange,
  });

  const size = screen.getByRole('combobox', { name: '尺寸' });
  expect(within(size).getByRole('option', { name: '无' })).toBeInTheDocument();
  expect(within(size).getByRole('option', { name: '自定义' })).toBeInTheDocument();

  fireEvent.change(size, { target: { value: '' } });
  expect(onImageOptionsChange).toHaveBeenLastCalledWith({ size: undefined });
});

test('shows common aspect ratio presets in the image size select', () => {
  renderComposer({
    supportsImageGeneration: true,
    generationMode: 'image',
  });

  const size = screen.getByRole('combobox', { name: '尺寸' });
  for (const ratio of ['1:1', '3:4', '4:3', '16:9', '9:16']) {
    expect(within(size).getByRole('option', { name: ratio })).toBeInTheDocument();
  }
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
