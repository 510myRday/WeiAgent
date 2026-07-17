import { memo, useEffect, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { Send, Square } from 'lucide-react';

import { Button } from '../../components/ui';
import { useI18n } from '../../i18n';
import { readChatDraft, writeChatDraft } from './chatDrafts';

interface ChatComposerProps {
  disabled: boolean;
  placeholder: string;
  isSending: boolean;
  isSavingModel: boolean;
  isModelSavePending: boolean;
  draftKey: string;
  isStreamMode: boolean;
  onStreamModeChange: (isStreamMode: boolean) => void;
  onSend: (content: string, isStreamMode: boolean) => void;
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
  onStreamModeChange,
  onSend,
  onStop,
}: ChatComposerProps) {
  const { t } = useI18n();
  const [draft, setDraft] = useState('');
  const formRef = useRef<HTMLFormElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    setDraft(readChatDraft(draftKey));
  }, [draftKey]);

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

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (isSending || isModelSavePending || isSavingModel) return;
    const content = draft.trim();
    if (!content || disabled) return;
    setDraft('');
    writeChatDraft(draftKey, '');
    onSend(content, isStreamMode);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (disabled || isSending) return;
    formRef.current?.requestSubmit();
  }

  return (
    <form className="composer" ref={formRef} onSubmit={handleSubmit}>
      <div className="composer-box">
        <textarea
          ref={inputRef}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          aria-label={t('chat.messageLabel')}
          disabled={disabled}
          rows={1}
        />
        <div className="composer-toolbar">
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
          <div className="composer-actions">
            <Button
              className={`send-button${isSending ? ' is-stopping' : ''}`}
              variant={isSending ? 'danger' : 'primary'}
              type={isSending ? 'button' : 'submit'}
              onClick={isSending ? onStop : undefined}
              disabled={disabled || (!isSending && (isModelSavePending || isSavingModel || !draft.trim()))}
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
