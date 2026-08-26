import {
  forwardRef,
  memo,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from 'react';
import {
  Brain,
  Check,
  ChevronDown,
  ChevronUp,
  Image as ImageIcon,
  ListPlus,
  LoaderCircle,
  Paperclip,
  Pencil,
  Scan,
  Send,
  Square,
  X,
} from 'lucide-react';

import type {
  ConversationUsage,
  GenerationMode,
  ImageAttachment,
  ImageModelConfig,
  ThinkingLevel,
} from '../../types';
import { ImageSizeControl } from '../../components/ImageSizeControl';
import { Button, useDismissiblePopover } from '../../components/ui';
import { useI18n } from '../../i18n';
import { readChatDraft, writeChatDraft } from './chatDrafts';
import {
  MAX_IMAGE_COUNT,
  attachmentImageUrl,
  createAttachmentId,
  removeAttachment,
  toggleAttachmentMask,
} from './imageAttachments';
import { useImageAttachments } from './useImageAttachments';

const THINKING_LEVELS: ThinkingLevel[] = ['off', 'low', 'medium', 'high', 'x_high'];
const IMAGE_QUALITY_OPTIONS = ['auto', 'low', 'medium', 'high', 'standard', 'hd'];

type ComposerImageOptions = Pick<ImageModelConfig, 'size' | 'quality' | 'n'>;

export type QueuedChatMessage = {
  id: string;
  content: string;
  attachments?: readonly ImageAttachment[];
  generationMode?: GenerationMode;
};

export type QueuedMessageMoveDirection = 'up' | 'down';

interface QueuedMessagesProps {
  queuedMessages: readonly QueuedChatMessage[];
  onEditQueuedMessage: (message: QueuedChatMessage) => void;
  onMoveQueuedMessage: (id: string, direction: QueuedMessageMoveDirection) => void;
  onRemoveQueuedMessage: (id: string) => void;
  onSteerQueuedMessage: (id: string) => void;
}

interface ChatComposerProps {
  disabled: boolean;
  placeholder: string;
  inputLabel: string;
  isSending: boolean;
  isSavingModel: boolean;
  isModelSavePending: boolean;
  draftKey: string;
  supportsImages: boolean;
  supportsImageGeneration: boolean;
  generationMode: GenerationMode;
  imageOptions: ComposerImageOptions;
  imageSizeControlKey?: string;
  isSavingImageOptions: boolean;
  supportsThinking: boolean;
  thinkingLevel: ThinkingLevel;
  usage: ConversationUsage;
  /** Model context window size in tokens. */
  contextSize: number;
  onGenerationModeChange: (mode: GenerationMode) => void;
  onImageOptionsChange: (patch: Partial<ComposerImageOptions>) => void;
  onThinkingLevelChange: (level: ThinkingLevel) => void;
  onUnsupportedImage: () => void;
  onSend: (
    content: string,
    attachments: ImageAttachment[],
    generationMode: GenerationMode,
  ) => void;
  onSteer: (content: string) => void;
  onStop: () => void;
}

export type ChatComposerHandle = {
  addImageAttachment: (attachment: ImageAttachment) => void;
  restoreQueuedMessage: (message: QueuedChatMessage) => void;
};

export const ChatComposer = memo(forwardRef<ChatComposerHandle, ChatComposerProps>(function ChatComposer({
  disabled,
  placeholder,
  inputLabel,
  isSending,
  isSavingModel,
  isModelSavePending,
  draftKey,
  supportsImages,
  supportsImageGeneration,
  generationMode,
  imageOptions,
  imageSizeControlKey,
  isSavingImageOptions,
  supportsThinking,
  thinkingLevel,
  usage,
  contextSize,
  onGenerationModeChange,
  onImageOptionsChange,
  onThinkingLevelChange,
  onUnsupportedImage,
  onSend,
  onSteer,
  onStop,
}: ChatComposerProps, ref) {
  const { t, locale } = useI18n();
  const [draft, setDraft] = useState('');
  const [thinkingOpen, setThinkingOpen] = useState(false);
  const formRef = useRef<HTMLFormElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const thinkingRef = useDismissiblePopover<HTMLDivElement>(thinkingOpen, () => setThinkingOpen(false));
  const {
    attachments,
    setAttachments,
    isDraggingFiles,
    resetDragState,
    addFiles,
    handlePaste,
    handleDragEnter,
    handleDragOver,
    handleDragLeave,
    handleDrop,
  } = useImageAttachments({
    supportsImages: supportsImages || (supportsImageGeneration && generationMode === 'image'),
    interactionDisabled: disabled || isSending,
    onUnsupportedImage,
  });
  const canAttachImages = supportsImages || (supportsImageGeneration && generationMode === 'image');

  useImperativeHandle(ref, () => ({
    addImageAttachment(attachment) {
      if (!canAttachImages) {
        onUnsupportedImage();
        return;
      }
      setAttachments((current) => (
        current.length >= MAX_IMAGE_COUNT
          ? current
          : [...current, { ...attachment, id: createAttachmentId(), is_mask: false }]
      ));
      inputRef.current?.focus();
    },
    restoreQueuedMessage(message) {
      const content = message.content;
      setDraft(content);
      setAttachments((message.attachments ?? []).slice(0, MAX_IMAGE_COUNT).map((attachment) => ({
        ...attachment,
        id: createAttachmentId(),
      })));
      if (message.generationMode) onGenerationModeChange(message.generationMode);
      writeChatDraft(draftKey, content);
      inputRef.current?.focus();
    },
  }), [canAttachImages, draftKey, onGenerationModeChange, onUnsupportedImage, setAttachments]);

  useEffect(() => {
    setDraft(readChatDraft(draftKey));
    setAttachments([]);
    resetDragState();
    setThinkingOpen(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftKey]);

  useEffect(() => {
    if (!supportsImages && !(supportsImageGeneration && generationMode === 'image')) setAttachments([]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generationMode, supportsImageGeneration, supportsImages]);

  useEffect(() => {
    const timer = window.setTimeout(() => writeChatDraft(draftKey, draft), 200);
    return () => window.clearTimeout(timer);
  }, [draft, draftKey]);

  useLayoutEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = `${Math.min(Math.max(input.scrollHeight, 48), 168)}px`;
  }, [draft]);

  function submitMessage(mode: 'send' | 'steer') {
    if (isModelSavePending || isSavingModel) return;
    const content = draft.trim();
    if ((!content && attachments.length === 0) || disabled) return;
    const submittedAttachments = attachments;
    setDraft('');
    setAttachments([]);
    writeChatDraft(draftKey, '');
    if (mode === 'steer') {
      if (content) onSteer(content);
    } else {
      onSend(content, submittedAttachments, generationMode);
    }
  }

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    submitMessage('send');
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (disabled) return;
    if (isSending && (event.ctrlKey || event.metaKey)) {
      submitMessage('steer');
      return;
    }
    formRef.current?.requestSubmit();
  }

  const canSend = Boolean(draft.trim() || attachments.length > 0);
  return (
    <form className="composer" ref={formRef} onSubmit={handleSubmit}>
      <div
        className={`composer-box${isDraggingFiles ? ' is-dragging-files' : ''}`}
        onDragEnter={handleDragEnter}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
      >
        {attachments.length > 0 ? (
          <div className="composer-image-strip">
            {attachments.map((attachment) => (
              <div
                className={`composer-image-preview${attachment.is_mask ? ' is-mask' : ''}`}
                key={attachment.id}
              >
                <img src={attachmentImageUrl(attachment)} alt={attachment.name} referrerPolicy="no-referrer" />
                {generationMode === 'image' && attachment.media_type === 'image/png' ? (
                  <button
                    type="button"
                    className={`composer-image-mask-button${attachment.is_mask ? ' is-active' : ''}`}
                    onClick={() => setAttachments((current) => (
                      toggleAttachmentMask(current, attachment.id)
                    ))}
                    disabled={
                      !attachment.is_mask
                      && !attachments.some((item) => item.id !== attachment.id && !item.is_mask)
                    }
                    aria-pressed={Boolean(attachment.is_mask)}
                    aria-label={t(attachment.is_mask ? 'chat.maskImage' : 'chat.markAsMask')}
                    title={t(attachment.is_mask ? 'chat.maskImage' : 'chat.markAsMask')}
                  >
                    <Scan size={13} />
                  </button>
                ) : null}
                <button
                  type="button"
                  className="composer-image-remove-button"
                  onClick={() => setAttachments((current) => (
                    removeAttachment(current, attachment.id)
                  ))}
                  aria-label={t('chat.removeImage')}
                  title={t('chat.removeImage')}
                >
                  <X size={13} />
                </button>
              </div>
            ))}
          </div>
        ) : null}
        <textarea
          ref={inputRef}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          placeholder={placeholder}
          aria-label={inputLabel}
          disabled={disabled}
          rows={1}
        />
        <div className="composer-toolbar">
          <div className="composer-toolbar-main">
            <div className="composer-controls">
            <input
              ref={fileInputRef}
              className="composer-file-input"
              type="file"
              accept=".png,.jpg,.jpeg,.webp,.gif,image/png,image/jpeg,image/webp,image/gif"
              multiple
              onChange={(event) => {
                void addFiles(Array.from(event.target.files ?? []));
                event.target.value = '';
              }}
            />
            <button
              type="button"
              className="composer-icon-button composer-add-image"
              onClick={() => fileInputRef.current?.click()}
              disabled={disabled || isSending}
              aria-label={t('chat.addImage')}
              title={t('chat.addImage')}
            >
              <Paperclip size={18} />
            </button>
            {supportsImageGeneration ? (
              <button
                type="button"
                className={`thinking-button${generationMode === 'image' ? ' is-active' : ''}`}
                onClick={() => onGenerationModeChange(generationMode === 'image' ? 'chat' : 'image')}
                disabled={disabled || isSending || supportsImageGeneration}
                aria-pressed={generationMode === 'image'}
                aria-label={t('chat.imageMode')}
                title={t('chat.imageMode')}
              >
                <ImageIcon size={17} />
                <span>{t('chat.imageMode')}</span>
              </button>
            ) : null}
            {supportsThinking ? (
              <div className="thinking-control" ref={thinkingRef}>
                <button
                  type="button"
                  className={`thinking-button${thinkingLevel !== 'off' ? ' is-active' : ''}`}
                  onClick={() => setThinkingOpen((open) => !open)}
                  disabled={disabled || isSending}
                  aria-expanded={thinkingOpen}
                  aria-label={t('chat.thinking')}
                  title={t('chat.thinking')}
                >
                  <Brain size={17} />
                  <span>
                    {t('chat.thinkingLevelLabel', {
                      level: t(`chat.thinkingLevel.${thinkingLevel}`),
                    })}
                  </span>
                </button>
                {thinkingOpen ? (
                  <div className="thinking-menu" role="menu">
                    {THINKING_LEVELS.map((level) => (
                      <button
                        key={level}
                        type="button"
                        role="menuitemradio"
                        aria-checked={thinkingLevel === level}
                        className={thinkingLevel === level ? 'is-selected' : ''}
                        onClick={() => {
                          onThinkingLevelChange(level);
                          setThinkingOpen(false);
                        }}
                      >
                        <span>{t(`chat.thinkingLevel.${level}`)}</span>
                        {thinkingLevel === level ? <Check size={15} /> : null}
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            ) : null}
            {supportsImageGeneration ? (
              <div className="composer-image-options" aria-label={t('chat.imageSettings')}>
                <label className="composer-image-option composer-image-option--size">
                  <span>{t('provider.imageSize')}</span>
                  <ImageSizeControl
                    value={imageOptions.size}
                    resetKey={imageSizeControlKey}
                    selectAriaLabel={t('provider.imageSize')}
                    onChange={(size) => onImageOptionsChange({ size })}
                    disabled={disabled || isSending || isSavingImageOptions}
                  />
                </label>
                <label className="composer-image-option">
                  <span>{t('provider.imageQuality')}</span>
                  <select
                    value={imageOptions.quality ?? ''}
                    onChange={(event) => onImageOptionsChange({
                      quality: event.target.value || undefined,
                    })}
                    disabled={disabled || isSending || isSavingImageOptions}
                  >
                    <option value="">{t('provider.imageDefault')}</option>
                    {IMAGE_QUALITY_OPTIONS.map((quality) => (
                      <option key={quality} value={quality}>{quality}</option>
                    ))}
                  </select>
                </label>
                <label className="composer-image-option composer-image-option--count">
                  <span>{t('provider.imageCount')}</span>
                  <select
                    value={imageOptions.n ?? ''}
                    onChange={(event) => onImageOptionsChange({
                      n: event.target.value ? Number(event.target.value) : undefined,
                    })}
                    disabled={disabled || isSending || isSavingImageOptions}
                  >
                    <option value="">{t('provider.imageDefault')}</option>
                    {Array.from({ length: 10 }, (_, index) => index + 1).map((count) => (
                      <option key={count} value={count}>{count}</option>
                    ))}
                  </select>
                </label>
                {isSavingImageOptions ? (
                  <span
                    className="composer-image-options__saving"
                    title={t('tools.autoSave.saving')}
                    aria-label={t('tools.autoSave.saving')}
                  >
                    <LoaderCircle size={14} className="is-spinning" />
                  </span>
                ) : null}
              </div>
            ) : null}
            </div>
            <div className="composer-actions">
            <div className="conversation-usage">
              <ContextUsageRing
                used={usage.total_tokens}
                limit={contextSize}
                label={t('chat.contextUsage', {
                  used: formatTokenCount(usage.total_tokens, locale),
                  limit: formatTokenCount(Math.max(1, contextSize), locale),
                })}
                detailTitle={t('chat.contextUsageTitle')}
                detailUsed={t('chat.contextUsageUsed', {
                  percent: formatUsagePercent(usage.total_tokens, contextSize),
                  remaining: Math.max(
                    0,
                    100 - formatUsagePercent(usage.total_tokens, contextSize),
                  ),
                })}
                detailStats={t('chat.contextUsageStats', {
                  used: formatTokenCount(usage.total_tokens, locale),
                  limit: formatTokenCount(Math.max(1, contextSize), locale),
                })}
              />
            </div>
            {isSending && canSend ? (
              <button
                type="button"
                className="composer-queue-button"
                onClick={() => formRef.current?.requestSubmit()}
                disabled={disabled || !canSend}
                aria-label={t('chat.queueMessage')}
                title={t('chat.queueMessage')}
              >
                <ListPlus size={16} />
              </button>
            ) : null}
            <Button
              className={`send-button${isSending ? ' is-stopping' : ''}`}
              variant={isSending ? 'danger' : 'primary'}
              type="button"
              onClick={(event) => {
                // Keep this button out of the form's native activation path. Stopping a
                // run changes isSending synchronously, which can otherwise turn the
                // clicked button into a submit button before the click completes.
                event.preventDefault();
                if (isSending) {
                  onStop();
                } else {
                  formRef.current?.requestSubmit();
                }
              }}
              disabled={disabled || (!isSending && (isModelSavePending || isSavingModel || !canSend))}
              icon={isSending ? <Square className="stop-icon" size={16} /> : <Send size={17} />}
            >
              {isSending ? t('chat.stop') : t('chat.send')}
            </Button>
            </div>
          </div>
        </div>
      </div>
    </form>
  );
}));

export function QueuedMessages({
  queuedMessages,
  onEditQueuedMessage,
  onMoveQueuedMessage,
  onRemoveQueuedMessage,
  onSteerQueuedMessage,
}: QueuedMessagesProps) {
  const { t } = useI18n();

  return queuedMessages.length > 0 ? (
    <div className="composer-queue" aria-label={t('chat.queuedMessages')}>
      {queuedMessages.map((message, index) => (
        <div className="composer-queue-row" key={message.id}>
          <span className="composer-queue-index">{index + 1}</span>
          <span
            className="composer-queue-content"
            title={message.content || t('chat.queuedMessageImageOnly')}
          >
            {message.content || t('chat.queuedMessageImageOnly')}
          </span>
          {message.attachments && message.attachments.length > 0 ? (
            <span
              className="composer-queue-attachment"
              aria-label={t('chat.queuedMessageImages', { count: message.attachments.length })}
              title={t('chat.queuedMessageImages', { count: message.attachments.length })}
            >
              <ImageIcon size={14} />
              {message.attachments.length > 1 ? <span>{message.attachments.length}</span> : null}
            </span>
          ) : null}
          <button
            type="button"
            className="composer-queue-action"
            onClick={() => onEditQueuedMessage(message)}
            aria-label={t('chat.editQueuedMessage')}
            title={t('chat.editQueuedMessage')}
          >
            <Pencil size={14} />
          </button>
          <button
            type="button"
            className="composer-queue-action"
            onClick={() => onMoveQueuedMessage(message.id, 'up')}
            disabled={index === 0}
            aria-label={t('chat.moveQueuedMessageUp')}
            title={t('chat.moveQueuedMessageUp')}
          >
            <ChevronUp size={14} />
          </button>
          <button
            type="button"
            className="composer-queue-action"
            onClick={() => onMoveQueuedMessage(message.id, 'down')}
            disabled={index === queuedMessages.length - 1}
            aria-label={t('chat.moveQueuedMessageDown')}
            title={t('chat.moveQueuedMessageDown')}
          >
            <ChevronDown size={14} />
          </button>
          <button
            type="button"
            className="composer-queue-action"
            onClick={() => onSteerQueuedMessage(message.id)}
            aria-label={t('chat.steerQueuedMessage')}
            title={t('chat.steerQueuedMessage')}
          >
            <Send size={14} />
          </button>
          <button
            type="button"
            className="composer-queue-action"
            onClick={() => onRemoveQueuedMessage(message.id)}
            aria-label={t('chat.removeQueuedMessage')}
            title={t('chat.removeQueuedMessage')}
          >
            <X size={14} />
          </button>
          </div>
      ))}
    </div>
  ) : null;
}

function ContextUsageRing({
  used,
  limit,
  label,
  detailTitle,
  detailUsed,
  detailStats,
}: {
  used: number;
  limit: number;
  label: string;
  detailTitle: string;
  detailUsed: string;
  detailStats: string;
}) {
  const safeLimit = Math.max(1, Math.floor(limit) || 128000);
  const safeUsed = Math.max(0, used);
  const ratio = Math.min(1, safeUsed / safeLimit);
  const size = 16;
  const stroke = 2.25;
  const radius = (size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  const dashOffset = circumference * (1 - ratio);

  return (
    <span
      className={`context-usage-ring${ratio >= 1 ? ' is-full' : ''}`}
    >
      <span className="context-usage-ring__icon" role="img" aria-label={label}>
        <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
          <circle
            className="context-usage-ring__track"
            cx={size / 2}
            cy={size / 2}
            r={radius}
            fill="none"
            strokeWidth={stroke}
          />
          <circle
            className="context-usage-ring__progress"
            cx={size / 2}
            cy={size / 2}
            r={radius}
            fill="none"
            strokeWidth={stroke}
            strokeDasharray={circumference}
            strokeDashoffset={dashOffset}
            strokeLinecap="round"
            transform={`rotate(-90 ${size / 2} ${size / 2})`}
          />
        </svg>
      </span>
      <span className="context-usage-ring__tooltip" role="tooltip">
        <span className="context-usage-ring__tooltip-title">{detailTitle}</span>
        <span className="context-usage-ring__tooltip-line">{detailUsed}</span>
        <span className="context-usage-ring__tooltip-line">{detailStats}</span>
      </span>
    </span>
  );
}

function formatTokenCount(value: number, locale: string): string {
  return new Intl.NumberFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
    notation: value >= 10_000 ? 'compact' : 'standard',
    maximumFractionDigits: 1,
  }).format(value);
}

function formatUsagePercent(used: number, limit: number): number {
  const safeLimit = Math.max(1, Math.floor(limit) || 128000);
  const safeUsed = Math.max(0, used);
  return Math.min(100, Math.round((safeUsed / safeLimit) * 100));
}
