import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';
import { useState, type ComponentProps } from 'react';

import { FeedbackProvider } from '../../components/ui';
import { I18nProvider } from '../../i18n';
import { CUSTOM_IMAGE_SIZE } from '../../components/ImageSizeControl';
import { ChatComposer, QueuedMessages } from './ChatComposer';

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
  const defaults = getComposerDefaults();
  return render(
    <I18nProvider>
      <FeedbackProvider>
        <ChatComposer
          {...defaults}
          {...overrides}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );
}

function getComposerDefaults(): ComponentProps<typeof ChatComposer> {
  return {
    disabled: false,
    placeholder: '输入消息',
    inputLabel: '消息',
    isSending: false,
    isSavingModel: false,
    isModelSavePending: false,
    draftKey: 'chat-composer-test',
    supportsImages: true,
    supportsImageGeneration: false,
    generationMode: 'chat',
    imageOptions: {},
    isSavingImageOptions: false,
    supportsThinking: true,
    thinkingLevel: 'medium',
    usage,
    contextSize: 128000,
    onGenerationModeChange: vi.fn(),
    onImageOptionsChange: vi.fn(),
    onThinkingLevelChange: vi.fn(),
    onUnsupportedImage: vi.fn(),
    onSend: vi.fn(),
    onSteer: vi.fn(),
    onStop: vi.fn(),
  };
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

test('stopping a send keeps the draft from being submitted during the state change', () => {
  const onSend = vi.fn();
  const onStop = vi.fn();
  function ControlledComposer() {
    const [isSending, setIsSending] = useState(true);
    return (
      <ChatComposer
        {...getComposerDefaults()}
        isSending={isSending}
        onSend={onSend}
        onStop={() => {
          onStop();
          setIsSending(false);
        }}
      />
    );
  }

  render(
    <I18nProvider>
      <FeedbackProvider>
        <ControlledComposer />
      </FeedbackProvider>
    </I18nProvider>,
  );
  const input = screen.getByRole('textbox', { name: '消息' });
  fireEvent.change(input, { target: { value: '这段内容应该保留' } });

  fireEvent.click(screen.getByRole('button', { name: '停止' }));

  expect(onStop).toHaveBeenCalledOnce();
  expect(onSend).not.toHaveBeenCalled();
  expect(input).toHaveValue('这段内容应该保留');
});

test('queues Enter submissions while a run is active', () => {
  const onSend = vi.fn();
  renderComposer({ isSending: true, onSend });
  const input = screen.getByRole('textbox', { name: '消息' });

  fireEvent.change(input, { target: { value: '排队的下一条消息' } });
  expect(screen.getByRole('button', { name: '排队发送' })).toBeInTheDocument();
  fireEvent.keyDown(input, { key: 'Enter' });

  expect(onSend).toHaveBeenCalledWith('排队的下一条消息', [], 'chat');
});

test('hides the queue button while the active run has no draft', () => {
  renderComposer({ isSending: true });

  expect(screen.queryByRole('button', { name: '排队发送' })).not.toBeInTheDocument();
});

test('steers the active run with Ctrl+Enter', () => {
  const onSteer = vi.fn();
  renderComposer({ isSending: true, onSteer });
  const input = screen.getByRole('textbox', { name: '消息' });

  fireEvent.change(input, { target: { value: '请把当前方向调整为这个' } });
  fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });

  expect(onSteer).toHaveBeenCalledWith('请把当前方向调整为这个');
});

test('renders queued messages with edit, move, guide, and remove actions', () => {
  const onRemoveQueuedMessage = vi.fn();
  const onSteerQueuedMessage = vi.fn();
  const onEditQueuedMessage = vi.fn();
  const onMoveQueuedMessage = vi.fn();
  const queuedMessages = [
    { id: 'queued-1', content: '已经在队列中' },
    { id: 'queued-2', content: '第二条消息' },
  ];
  render(
    <I18nProvider>
      <QueuedMessages
        queuedMessages={queuedMessages}
        onEditQueuedMessage={onEditQueuedMessage}
        onMoveQueuedMessage={onMoveQueuedMessage}
        onRemoveQueuedMessage={onRemoveQueuedMessage}
        onSteerQueuedMessage={onSteerQueuedMessage}
      />
    </I18nProvider>,
  );

  expect(screen.getByText('已经在队列中')).toBeInTheDocument();
  fireEvent.click(screen.getAllByRole('button', { name: '修改排队消息' })[0]);
  expect(onEditQueuedMessage).toHaveBeenCalledWith(queuedMessages[0]);

  fireEvent.click(screen.getAllByRole('button', { name: '下移排队消息' })[0]);
  expect(onMoveQueuedMessage).toHaveBeenCalledWith('queued-1', 'down');
  fireEvent.click(screen.getAllByRole('button', { name: '立即引导' })[0]);
  fireEvent.click(screen.getAllByRole('button', { name: '移除排队消息' })[0]);

  expect(onSteerQueuedMessage).toHaveBeenCalledWith('queued-1');
  expect(onRemoveQueuedMessage).toHaveBeenCalledWith('queued-1');
});
