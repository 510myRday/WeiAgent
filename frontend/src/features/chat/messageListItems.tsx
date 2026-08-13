import {
  memo,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type SyntheticEvent,
} from 'react';
import {
  Bot,
  Download,
  LoaderCircle,
  Maximize2,
  Paperclip,
  Pencil,
  RotateCcw,
  Scan,
  Send,
  X,
} from 'lucide-react';

import type {
  Agent,
  AgentRunPhase,
  AppSettings,
  ImageAttachment,
  ToolCallState,
  UiMessage,
  UiMessageItem,
} from '../../types';
import { AvatarBadge } from '../../components/AvatarPicker';
import { agentDisplayName } from '../../components/AgentListItem';
import { Dialog, useFeedback } from '../../components/ui';
import { useI18n } from '../../i18n';
import { MarkdownContent } from './MarkdownContent';
import { StreamingMarkdown } from './StreamingMarkdown';
import {
  MAX_IMAGE_COUNT,
  attachmentImageUrl,
  removeAttachment,
  toggleAttachmentMask,
} from './imageAttachments';
import { useImageAttachments } from './useImageAttachments';
import { ReasoningDisclosure } from './ReasoningDisclosure';
import { ToolDisclosure } from './ToolDisclosure';
import { classifyRetryReason } from './retryReason';
import { userErrorMessage } from '../../utils/userError';

const MAX_MESSAGE_IMAGE_EDGE = 360;

type MessageImageDisplaySize = {
  width: number;
  height: number;
};

type MessageActions = {
  onRestart: (messageId: string, content: string, attachments?: ImageAttachment[]) => void;
  onEditImage: (attachment: ImageAttachment) => void;
  onApproveToolCall: (
    approvalId: string,
    agentId: string,
    decision: 'approve' | 'reject' | 'always_approve',
  ) => Promise<void>;
};

export const MessageListItem = memo(function MessageListItem({
  item,
  previous,
  activeAgent,
  settings,
  isSending,
  supportsImages,
  supportsImageMasks,
  onRestart,
  onEditImage,
  onApproveToolCall,
  formatTimelineTime,
}: {
  item: UiMessageItem;
  previous?: UiMessageItem;
  activeAgent?: Agent;
  settings: AppSettings;
  isSending: boolean;
  supportsImages: boolean;
  supportsImageMasks: boolean;
  onRestart: MessageActions['onRestart'];
  onEditImage: MessageActions['onEditImage'];
  onApproveToolCall: MessageActions['onApproveToolCall'];
  formatTimelineTime: (value: string) => string;
}) {
  const { t } = useI18n();
  return (
    <MessageRow
      message={item}
      showTimeSeparator={shouldShowTimeSeparator(item, previous)}
      timeSeparatorLabel={formatTimelineTime(item.created_at)}
      participantName={item.role === 'user' ? t('chat.you') : activeAgent ? agentDisplayName(activeAgent, t) : 'Assistant'}
      agent={activeAgent}
      settings={settings}
      failedLabel={t('chat.failed')}
      actionsDisabled={isSending}
      supportsImages={supportsImages}
      supportsImageMasks={supportsImageMasks}
      onRestart={onRestart}
      onEditImage={onEditImage}
      onApproveToolCall={onApproveToolCall}
    />
  );
});

export function shouldShowAgentWaitingBubble(
  messages: UiMessageItem[],
  isSending: boolean,
  runPhase: AgentRunPhase,
  retryStatus: { attempt: number; maxAttempts: number; reason: string } | null,
): boolean {
  if (!isSending) return false;
  if (retryStatus) return true;
  if (runPhase === 'waiting_for_approval') return false;

  const last = messages[messages.length - 1];
  if (!last) return true;
  if (last.role === 'assistant') {
    const tools = (last.blocks ?? [])
      .filter((block): block is Extract<NonNullable<UiMessage['blocks']>[number], { type: 'tool' }> => block.type === 'tool')
      .map((block) => block.tool);
    if (tools.some((tool) => tool.status === 'pending_approval')) return false;
    if (last.status === 'streaming') return false;
    if (tools.some((tool) => tool.status === 'running')) return true;
    return runPhase === 'executing_tool';
  }
  return true;
}

export function WaitingBubble({
  agent,
  settings,
  retryStatus,
  runPhase,
}: {
  agent?: Agent;
  settings: AppSettings;
  retryStatus: { attempt: number; maxAttempts: number; reason: string } | null;
  runPhase: AgentRunPhase;
}) {
  const { t } = useI18n();
  // Compaction owns the waiting bubble for the whole summary request,
  // including any retries inside that request.
  const isCompacting = runPhase === 'compacting_context';
  const isRetrying = Boolean(retryStatus) && !isCompacting;
  const retryKind = retryStatus ? classifyRetryReason(retryStatus.reason) : null;
  const label = isCompacting
    ? t('chat.compactingContext')
    : isRetrying && retryStatus
      ? t(`chat.retryingByReason.${retryKind}`)
      : t('chat.sending');
  return (
    <article
      className={`message-row assistant is-streaming is-waiting${isRetrying ? ' is-retrying' : ''}`}
      aria-live="polite"
    >
      <MessageAvatar agent={agent} settings={settings} isUser={false} />
      <div className="message-body">
        <div className="message-meta">
          <strong>{agent ? agentDisplayName(agent, t) : 'Assistant'}</strong>
        </div>
        <div className="message-bubble">
          {isRetrying && retryStatus ? (
            <div className="message-retry-status" role="status" aria-label={label}>
              <span className="message-retry-spinner" aria-hidden="true">
                <LoaderCircle size={14} />
              </span>
              <div className="message-retry-copy">
                <div className="message-retry-heading">
                  <span className="message-retry-title">{label}</span>
                  <span className="message-retry-progress">
                    {retryStatus.attempt}/{retryStatus.maxAttempts}
                  </span>
                </div>
                <span className="message-retry-reason" title={retryStatus.reason}>
                  {retryStatus.reason}
                </span>
              </div>
            </div>
          ) : (
            <TypingIndicator label={label} />
          )}
        </div>
      </div>
    </article>
  );
}

const MessageRow = memo(function MessageRow({
  message,
  showTimeSeparator,
  timeSeparatorLabel,
  participantName,
  agent,
  settings,
  failedLabel,
  actionsDisabled,
  supportsImages,
  supportsImageMasks,
  onRestart,
  onEditImage,
  onApproveToolCall,
}: {
  message: UiMessage;
  showTimeSeparator: boolean;
  timeSeparatorLabel: string;
  participantName: string;
  agent?: Agent;
  settings: AppSettings;
  failedLabel: string;
  actionsDisabled: boolean;
  supportsImages: boolean;
  supportsImageMasks: boolean;
  onRestart: MessageActions['onRestart'];
  onEditImage: MessageActions['onEditImage'];
  onApproveToolCall: MessageActions['onApproveToolCall'];
}) {
  const { t } = useI18n();
  const isUser = message.role === 'user';
  const [isEditing, setIsEditing] = useState(false);
  const [editDraft, setEditDraft] = useState(message.content);
  const editInputRef = useRef<HTMLTextAreaElement>(null);
  const editFileInputRef = useRef<HTMLInputElement>(null);
  const isImageOnly = Boolean(
    message.attachments?.length
    && !message.content
    && !message.reasoning_content
    && !message.blocks?.length,
  );
  const {
    attachments: editAttachments,
    setAttachments: setEditAttachments,
    isDraggingFiles,
    resetDragState,
    addFiles: addEditFiles,
    handlePaste: handleEditPaste,
    handleDragEnter: handleEditDragEnter,
    handleDragOver: handleEditDragOver,
    handleDragLeave: handleEditDragLeave,
    handleDrop: handleEditDrop,
  } = useImageAttachments({
    supportsImages,
    interactionDisabled: actionsDisabled,
  });
  const canRestart = isUser && Boolean(message.id) && message.status !== 'streaming';

  useLayoutEffect(() => {
    const input = editInputRef.current;
    if (!isEditing || !input) return;
    input.style.height = 'auto';
    input.style.height = `${Math.min(Math.max(input.scrollHeight, 72), 240)}px`;
  }, [editDraft, isEditing]);

  useLayoutEffect(() => {
    if (!isEditing) return;
    const input = editInputRef.current;
    if (!input) return;
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }, [isEditing]);

  function beginEditing() {
    setEditDraft(message.content);
    setEditAttachments(message.attachments ?? []);
    setIsEditing(true);
  }

  function cancelEditing() {
    setEditDraft(message.content);
    setEditAttachments(message.attachments ?? []);
    resetDragState();
    setIsEditing(false);
  }

  function submitEdit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const content = editDraft.trim();
    if ((!content && editAttachments.length === 0) || actionsDisabled || !message.id) return;
    setIsEditing(false);
    onRestart(message.id, content, editAttachments);
  }

  function handleEditKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Escape') {
      event.preventDefault();
      cancelEditing();
      return;
    }
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  }

  return (
    <>
      {showTimeSeparator ? (
        <div className="message-time-separator">
          <time dateTime={message.created_at}>{timeSeparatorLabel}</time>
        </div>
      ) : null}
      <article
        className={[
          'message-row',
          message.role,
          message.status === 'streaming' ? 'is-streaming' : '',
          isImageOnly ? 'is-image-only' : '',
          isEditing ? 'is-editing' : '',
        ].filter(Boolean).join(' ')}
      >
        {!isUser ? <MessageAvatar agent={agent} settings={settings} isUser={false} /> : null}
        <div className="message-body">
          <div className="message-meta">
            <strong>{participantName}</strong>
          </div>
          {isEditing ? (
            <form className="message-edit-form" onSubmit={submitEdit}>
              <div className="message-bubble message-edit-bubble">
                <div
                  className={`message-edit-drop-zone${isDraggingFiles ? ' is-dragging-files' : ''}`}
                  onDragEnter={handleEditDragEnter}
                  onDragOver={handleEditDragOver}
                  onDragLeave={handleEditDragLeave}
                  onDrop={handleEditDrop}
                >
                  {editAttachments.length > 0 ? (
                    <div className="message-edit-image-strip">
                      {editAttachments.map((attachment) => (
                        <div
                          className={`message-edit-image-preview${attachment.is_mask ? ' is-mask' : ''}`}
                          key={attachment.id}
                        >
                          <img
                            src={attachmentImageUrl(attachment)}
                            alt={attachment.name}
                            referrerPolicy="no-referrer"
                          />
                          {supportsImageMasks && attachment.media_type === 'image/png' ? (
                            <button
                              type="button"
                              className={`message-edit-image-mask-button${attachment.is_mask ? ' is-active' : ''}`}
                              onClick={() => setEditAttachments((current) => (
                                toggleAttachmentMask(current, attachment.id)
                              ))}
                              disabled={
                                !attachment.is_mask
                                && !editAttachments.some((item) => (
                                  item.id !== attachment.id && !item.is_mask
                                ))
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
                            className="message-edit-image-remove-button"
                            onClick={() => setEditAttachments((current) => (
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
                    ref={editInputRef}
                    className="message-edit-textarea"
                    value={editDraft}
                    onChange={(event) => setEditDraft(event.target.value)}
                    onKeyDown={handleEditKeyDown}
                    onPaste={handleEditPaste}
                    aria-label={t('chat.editMessage')}
                    disabled={actionsDisabled}
                    rows={3}
                  />
                </div>
              </div>
              <div className="message-actions message-edit-actions">
                <input
                  ref={editFileInputRef}
                  className="composer-file-input"
                  type="file"
                  accept=".png,.jpg,.jpeg,.webp,.gif,image/png,image/jpeg,image/webp,image/gif"
                  multiple
                  onChange={(event) => {
                    void addEditFiles(Array.from(event.target.files ?? []));
                    event.target.value = '';
                  }}
                />
                <button
                  className="message-action-button message-edit-add-image"
                  type="button"
                  onClick={() => editFileInputRef.current?.click()}
                  disabled={actionsDisabled || !supportsImages || editAttachments.length >= MAX_IMAGE_COUNT}
                  aria-label={t('chat.addImage')}
                  title={t('chat.addImage')}
                >
                  <Paperclip size={15} />
                  <span>{t('chat.addImage')}</span>
                </button>
                <button
                  className="message-action-button"
                  type="button"
                  onClick={cancelEditing}
                  aria-label={t('chat.cancelEdit')}
                  title={t('chat.cancelEdit')}
                >
                  <X size={14} />
                  <span>{t('chat.cancelEdit')}</span>
                </button>
                <button
                  className="message-action-button is-primary"
                  type="submit"
                  disabled={actionsDisabled || (!editDraft.trim() && !message.attachments?.length)}
                  aria-label={t('chat.sendEdit')}
                  title={t('chat.sendEdit')}
                >
                  <Send size={14} />
                  <span>{t('chat.sendEdit')}</span>
                </button>
              </div>
            </form>
          ) : (
            <>
              <div className="message-bubble">
                <MessageContent
                  message={message}
                  agentId={agent?.id ?? ''}
                  onEditImage={onEditImage}
                  onApproveToolCall={onApproveToolCall}
                />
                {message.status === 'failed' ? <span className="message-status error">{failedLabel}</span> : null}
              </div>
              {canRestart ? (
                <div className="message-actions">
                  <button
                    className="message-action-button"
                    type="button"
                    onClick={beginEditing}
                    disabled={actionsDisabled}
                    aria-label={t('chat.editMessage')}
                    title={t('chat.editMessage')}
                  >
                    <Pencil size={15} />
                  </button>
                  <button
                    className="message-action-button"
                    type="button"
                    onClick={() => onRestart(message.id, message.content, message.attachments)}
                    disabled={actionsDisabled}
                    aria-label={t('chat.retryMessage')}
                    title={t('chat.retryMessage')}
                  >
                    <RotateCcw size={15} />
                  </button>
                </div>
              ) : null}
            </>
          )}
        </div>
        {isUser ? <MessageAvatar agent={agent} settings={settings} isUser /> : null}
      </article>
    </>
  );
});

export function MessageAvatar({
  agent,
  settings,
  isUser,
}: {
  agent?: Agent;
  settings: AppSettings;
  isUser: boolean;
}) {
  if (isUser) {
    return <AvatarBadge icon={settings.avatar_icon} color={settings.avatar_color} size={23} className="chat-avatar" />;
  }
  if (agent) {
    return <AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} size={23} className="chat-avatar" />;
  }
  return <span className="chat-avatar avatar-muted"><Bot size={23} /></span>;
}

function MessageContent({
  message,
  agentId,
  onEditImage,
  onApproveToolCall,
}: {
  message: UiMessage;
  agentId: string;
  onEditImage: MessageActions['onEditImage'];
  onApproveToolCall: MessageActions['onApproveToolCall'];
}) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const [previewAttachment, setPreviewAttachment] = useState<ImageAttachment | null>(null);
  const [imageDisplaySizes, setImageDisplaySizes] = useState<Record<string, MessageImageDisplaySize>>({});
  const blocks = message.blocks ?? [];
  const tools = blocks
    .filter((block): block is Extract<NonNullable<UiMessage['blocks']>[number], { type: 'tool' }> => block.type === 'tool')
    .map((block) => block.tool);
  const hasStructuredBody = tools.length > 0;
  const emptyStreaming = (
    message.status === 'streaming'
    && !message.content
    && !message.reasoning_content
    && tools.length === 0
  );
  if (emptyStreaming) {
    return <TypingIndicator label={t('chat.sending')} />;
  }

  function handleImageLoad(attachmentId: string, event: SyntheticEvent<HTMLImageElement>) {
    const image = event.currentTarget;
    if (!image.naturalWidth || !image.naturalHeight) return;

    const scale = Math.min(
      MAX_MESSAGE_IMAGE_EDGE / image.naturalWidth,
      MAX_MESSAGE_IMAGE_EDGE / image.naturalHeight,
      1,
    );
    const nextSize = {
      width: Math.round(image.naturalWidth * scale),
      height: Math.round(image.naturalHeight * scale),
    };

    setImageDisplaySizes((current) => {
      const previous = current[attachmentId];
      if (previous?.width === nextSize.width && previous.height === nextSize.height) return current;
      return { ...current, [attachmentId]: nextSize };
    });
  }

  const singleImageDisplaySize = message.attachments?.length === 1
    ? imageDisplaySizes[message.attachments[0].id]
    : undefined;

  async function handleApprove(tool: ToolCallState, decision: 'approve' | 'reject' | 'always_approve') {
    const approvalId = typeof tool.arguments.approval_id === 'string' ? tool.arguments.approval_id : '';
    const argumentAgentId = typeof tool.arguments.agent_id === 'string' ? tool.arguments.agent_id : '';
    const resolvedAgentId = argumentAgentId || agentId;
    if (!approvalId || !resolvedAgentId) return;
    try {
      await onApproveToolCall(approvalId, resolvedAgentId, decision);
    } catch (cause) {
      toast.error(userErrorMessage(cause, {
        summary: t('error.conversationActionFailed'),
        recovery: t('error.retryConnection'),
      }));
      throw cause;
    }
  }

  return (
    <>
      {message.attachments?.length ? (
        <div
          className={`message-image-grid${message.attachments.length === 1 ? ' is-single' : ''}${
            singleImageDisplaySize ? ' has-sized-single' : ''
          }`}
          style={singleImageDisplaySize ? { width: `${singleImageDisplaySize.width}px` } : undefined}
        >
          {message.attachments.map((attachment) => {
            const imageUrl = attachmentImageUrl(attachment);
            const displaySize = message.attachments?.length === 1
              ? imageDisplaySizes[attachment.id]
              : undefined;
            return (
              <div
                className="message-image-preview"
                key={attachment.id}
                style={displaySize ? {
                  width: `${displaySize.width}px`,
                  aspectRatio: `${displaySize.width} / ${displaySize.height}`,
                  minHeight: 0,
                } : undefined}
              >
                <button
                  className="message-image-preview__open"
                  type="button"
                  onClick={() => setPreviewAttachment(attachment)}
                  title={t('chat.openImage')}
                  aria-label={`${t('chat.openImage')}: ${attachment.name}`}
                >
                  <img
                    src={imageUrl}
                    alt={attachment.name}
                    loading="lazy"
                    referrerPolicy="no-referrer"
                    onLoad={(event) => handleImageLoad(attachment.id, event)}
                  />
                </button>
                <div className="message-image-preview__actions">
                  <a
                    className="message-image-action"
                    href={imageUrl}
                    download={attachment.name}
                    title={t('chat.downloadImage')}
                    aria-label={`${t('chat.downloadImage')}: ${attachment.name}`}
                  >
                    <Download size={14} aria-hidden="true" />
                  </a>
                  <button
                    className="message-image-action"
                    type="button"
                    onClick={() => setPreviewAttachment(attachment)}
                    title={t('chat.openImage')}
                    aria-label={`${t('chat.openImage')}: ${attachment.name}`}
                  >
                    <Maximize2 size={14} aria-hidden="true" />
                  </button>
                  <button
                    className="message-image-action"
                    type="button"
                    onClick={() => onEditImage(attachment)}
                    title={t('chat.editImage')}
                    aria-label={`${t('chat.editImage')}: ${attachment.name}`}
                  >
                    <Pencil size={14} aria-hidden="true" />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      ) : null}
      {message.reasoning_content ? (
        <ReasoningDisclosure
          content={message.reasoning_content}
          isStreaming={message.status === 'streaming' && !message.content && tools.length === 0}
        />
      ) : null}
      {hasStructuredBody ? (
        blocks.map((block, index) => {
          if (block.type === 'text') {
            if (!block.content) return null;
            const isStreamingTail = message.status === 'streaming' && index === blocks.length - 1;
            return (
              <div
                key={`text-${index}`}
                className={`message-markdown${isStreamingTail ? ' streaming-content' : ''}`}
              >
                {isStreamingTail ? (
                  <StreamingMarkdown content={block.content} />
                ) : (
                  <MarkdownContent content={block.content} />
                )}
              </div>
            );
          }
          return (
            <ToolDisclosure
              key={block.tool.call_id}
              agentId={agentId}
              tool={block.tool}
              onApprove={(decision) => handleApprove(block.tool, decision)}
            />
          );
        })
      ) : message.content ? (
        <div className={`message-markdown${message.status === 'streaming' ? ' streaming-content' : ''}`}>
          {message.status === 'streaming' ? (
            <StreamingMarkdown content={message.content} />
          ) : (
            <MarkdownContent content={message.content} />
          )}
        </div>
      ) : null}
      <Dialog
        open={Boolean(previewAttachment)}
        title={previewAttachment?.name || t('chat.imagePreview')}
        closeLabel={t('common.close')}
        onClose={() => setPreviewAttachment(null)}
        size="xl"
        className="image-preview-dialog"
        bodyClassName="image-preview-dialog__body"
      >
        {previewAttachment ? (
          <img
            className="image-preview-dialog__image"
            src={attachmentImageUrl(previewAttachment)}
            alt={previewAttachment.name}
            referrerPolicy="no-referrer"
          />
        ) : null}
      </Dialog>
    </>
  );
}

function TypingIndicator({ label }: { label: string }) {
  return (
    <div className="typing-indicator" role="status" aria-label={label}>
      <span />
      <span />
      <span />
    </div>
  );
}

function shouldShowTimeSeparator(message: UiMessage, previous?: UiMessage) {
  if (!previous) return true;
  const currentTime = new Date(message.created_at).getTime();
  const previousTime = new Date(previous.created_at).getTime();
  if (!Number.isFinite(currentTime) || !Number.isFinite(previousTime)) return false;
  return currentTime - previousTime >= 5 * 60 * 1000;
}
