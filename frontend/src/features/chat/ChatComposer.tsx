import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from 'react';
import {
  Brain,
  Check,
  Paperclip,
  Send,
  Square,
  X,
} from 'lucide-react';

import type {
  ConversationUsage,
  ImageAttachment,
  ThinkingLevel,
} from '../../types';
import { Button } from '../../components/ui';
import { useI18n } from '../../i18n';
import { readChatDraft, writeChatDraft } from './chatDrafts';
import { attachmentImageUrl } from './imageAttachments';
import { useImageAttachments } from './useImageAttachments';

const THINKING_LEVELS: ThinkingLevel[] = ['off', 'low', 'medium', 'high', 'x_high'];

interface ChatComposerProps {
  disabled: boolean;
  placeholder: string;
  isSending: boolean;
  isSavingModel: boolean;
  isModelSavePending: boolean;
  draftKey: string;
  isStreamMode: boolean;
  supportsImages: boolean;
  supportsThinking: boolean;
  thinkingLevel: ThinkingLevel;
  usage: ConversationUsage;
  /** Model context window size in tokens. */
  contextSize: number;
  onStreamModeChange: (isStreamMode: boolean) => void;
  onThinkingLevelChange: (level: ThinkingLevel) => void;
  onUnsupportedImage: () => void;
  onSend: (
    content: string,
    isStreamMode: boolean,
    attachments: ImageAttachment[],
  ) => void;
  onStop: () => void;
}

export const ChatComposer = memo(function ChatComposer({
  disabled,
  placeholder,
  isSending,
  isSavingModel,
  isModelSavePending,
  draftKey,
  isStreamMode,
  supportsImages,
  supportsThinking,
  thinkingLevel,
  usage,
  contextSize,
  onStreamModeChange,
  onThinkingLevelChange,
  onUnsupportedImage,
  onSend,
  onStop,
}: ChatComposerProps) {
  const { t, locale } = useI18n();
  const [draft, setDraft] = useState('');
  const [thinkingOpen, setThinkingOpen] = useState(false);
  const formRef = useRef<HTMLFormElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const thinkingRef = useRef<HTMLDivElement>(null);
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
    supportsImages,
    interactionDisabled: disabled || isSending,
    onUnsupportedImage,
  });

  useEffect(() => {
    setDraft(readChatDraft(draftKey));
    setAttachments([]);
    resetDragState();
    setThinkingOpen(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftKey]);

  useEffect(() => {
    if (!supportsImages) setAttachments([]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [supportsImages]);

  useEffect(() => {
    const timer = window.setTimeout(() => writeChatDraft(draftKey, draft), 200);
    return () => window.clearTimeout(timer);
  }, [draft, draftKey]);

  useEffect(() => {
    if (!thinkingOpen) return;
    const close = (event: MouseEvent) => {
      if (!thinkingRef.current?.contains(event.target as Node)) setThinkingOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [thinkingOpen]);

  useLayoutEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = `${Math.min(Math.max(input.scrollHeight, 48), 168)}px`;
  }, [draft]);

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (isSending || isModelSavePending || isSavingModel) return;
    const content = draft.trim();
    if ((!content && attachments.length === 0) || disabled) return;
    const submittedAttachments = attachments;
    setDraft('');
    setAttachments([]);
    writeChatDraft(draftKey, '');
    onSend(content, isStreamMode, submittedAttachments);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (disabled || isSending) return;
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
              <div className="composer-image-preview" key={attachment.id}>
                <img src={attachmentImageUrl(attachment)} alt={attachment.name} />
                <button
                  type="button"
                  onClick={() => setAttachments((current) => (
                    current.filter((item) => item.id !== attachment.id)
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
          aria-label={t('chat.messageLabel')}
          disabled={disabled}
          rows={1}
        />
        <div className="composer-toolbar">
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
            <button
              type="button"
              className={`stream-switch${isStreamMode ? ' is-active' : ''}`}
              role="switch"
              aria-checked={isStreamMode}
              aria-label={t('chat.stream')}
              onClick={() => onStreamModeChange(!isStreamMode)}
            >
              <span className="stream-switch-track" aria-hidden="true"><span /></span>
              <span>{t('chat.stream')}</span>
            </button>
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
            <Button
              className={`send-button${isSending ? ' is-stopping' : ''}`}
              variant={isSending ? 'danger' : 'primary'}
              type={isSending ? 'button' : 'submit'}
              onClick={isSending ? onStop : undefined}
              disabled={disabled || (!isSending && (isModelSavePending || isSavingModel || !canSend))}
              icon={isSending ? <Square className="stop-icon" size={16} /> : <Send size={17} />}
            >
              {isSending ? t('chat.stop') : t('chat.send')}
            </Button>
          </div>
        </div>
      </div>
    </form>
  );
});

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
