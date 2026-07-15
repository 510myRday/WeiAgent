import { useRef, useEffect, useLayoutEffect, useCallback, useMemo, useState, isValidElement } from 'react';
import type { FormEvent, KeyboardEvent, PointerEvent as ReactPointerEvent, ReactNode, RefObject } from 'react';
import {
  ArrowLeft,
  Bot,
  Check,
  ChevronDown,
  Clock3,
  Clipboard,
  MessageSquare,
  Square,
  Plus,
  Send,
  Trash2,
} from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import rehypeHighlight from 'rehype-highlight';
import rehypeKatex from 'rehype-katex';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';

import type {
  Agent,
  AgentRunPhase,
  AppSettings,
  Conversation,
  ModelProvider,
  ToolCallMessage,
  UiMessage,
  UiMessageItem,
  UpdateAgentPayload,
} from '../../types';
import { ToolCallCard } from '../../components/ToolCallCard';
import { approveToolCall, readAgentToolsConfig, type AgentToolsConfig } from '../../api';
import { useI18n } from '../../i18n';
import { useConversations } from '../../hooks';
import { AvatarBadge } from '../../components/AvatarPicker';
import { AgentListItem, agentDisplayName } from '../../components/AgentListItem';
import { Button, EmptyState, IconButton, ListItem, PaneHeader, SearchInput, useFeedback } from '../../components/ui';

interface ChatPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  activeAgentId: string;
  onAgentChange: (id: string) => void;
  onAgentEdit: (id: string, payload: UpdateAgentPayload) => Promise<Agent>;
  onOpenMain: () => void;
  onBackToList: () => void;
  settings: AppSettings;
}

type ModelOption = {
  value: string;
  providerId: string;
  modelId: string;
  label: string;
};

type PendingModelSave = {
  requestId: number;
  agentId: string;
  selection: ModelOption;
  previousValue: string;
};

const MODEL_AUTO_SAVE_DELAY_MS = 600;

export function ChatPanel({
  agents,
  providers,
  activeAgentId,
  onAgentChange,
  onAgentEdit,
  onOpenMain,
  onBackToList,
  settings,
}: ChatPanelProps) {
  const { t, locale } = useI18n();
  const { confirm, toast } = useFeedback();
  const {
    conversations,
    activeConversationId,
    setActiveConversationId,
    messages,
    isLoadingChat,
    isSending,
    runPhase,
    loadConversationsForAgent,
    loadMessages,
    cancelActiveSend,
    clearChat,
    send,
    createNewConversation,
    removeConversation,
  } = useConversations({
    newConversationTitle: t('conversation.newTitle'),
    runCancelledMessage: t('chat.runCancelled'),
    runInterruptedMessage: t('chat.runInterrupted'),
    onError: toast.error,
    timeoutSeconds: settings.request_timeout_seconds,
  });

  const [draft, setDraft] = useState('');
  const [isStreamMode, setIsStreamMode] = useState(true);
  const [selectedModelValue, setSelectedModelValue] = useState('');
  const [isModelSavePending, setIsModelSavePending] = useState(false);
  const [isSavingModel, setIsSavingModel] = useState(false);
  const [conversationFilter, setConversationFilter] = useState('');
  const [isAwayFromBottom, setIsAwayFromBottom] = useState(false);
  const [activeTimelineAnchor, setActiveTimelineAnchor] = useState('');
  const [terminalPermissionMode, setTerminalPermissionMode] = useState<AgentToolsConfig['terminal']['permission_mode'] | null>(null);

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);
  const composerFormRef = useRef<HTMLFormElement>(null);
  const composerInputRef = useRef<HTMLTextAreaElement>(null);
  const timelineWheelRef = useRef<HTMLDivElement>(null);
  const messagesScrollFrameRef = useRef<number | undefined>(undefined);
  const modelSaveTimerRef = useRef<number | undefined>(undefined);
  const modelSaveRequestRef = useRef(0);
  const pendingModelSaveRef = useRef<PendingModelSave | null>(null);

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const modelOptions = useMemo<ModelOption[]>(() => providers.flatMap((provider) =>
    provider.models.map((model) => ({
      value: JSON.stringify([provider.id, model]),
      providerId: provider.id,
      modelId: model,
      label: `${provider.name} | ${model}`,
    })),
  ), [providers]);
  const activeModelValue = activeAgent
    ? JSON.stringify([activeAgent.model_provider_id, activeAgent.model_id])
    : '';
  const hasProvider = Boolean(activeAgent?.model_provider_id);
  const hasModel = Boolean(activeAgent?.model_id);
  const isComposerDisabled = !activeAgent || !hasProvider || !hasModel;
  const composerPlaceholder = !activeAgent
    ? t('chat.placeholderDisabled')
    : !hasProvider
      ? t('chat.placeholderNoProvider')
      : !hasModel
        ? t('chat.placeholderNoModel')
        : t('chat.placeholder');
  const activeConversation = conversations.find((conversation) => conversation.id === activeConversationId);
  const filteredConversations = conversations.filter((conversation) =>
    conversation.title.toLowerCase().includes(conversationFilter.trim().toLowerCase()),
  );
  const timelineItems = useMemo<MessageTimelineItem[]>(() => {
    const result: MessageTimelineItem[] = [];
    let previousTimelineTimestamp: number | undefined;
    messages.forEach((item, index) => {
      if (!('role' in item)) return;
      const date = new Date(item.created_at);
      const timestamp = date.getTime();
      if (!Number.isFinite(timestamp)) return;
      if (previousTimelineTimestamp !== undefined && timestamp - previousTimelineTimestamp < 60_000) return;
      previousTimelineTimestamp = timestamp;
      result.push({
        anchorId: `message-anchor-${index}`,
        dateLabel: new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', { month: '2-digit', day: '2-digit' }).format(date),
        timeLabel: new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', { hour: '2-digit', minute: '2-digit', hour12: false }).format(date),
        preview: item.content.trim().replace(/\s+/g, ' ').slice(0, 48),
      });
    });
    return result;
  }, [locale, messages]);

  useEffect(() => {
    const pending = pendingModelSaveRef.current;
    if (pending) {
      pendingModelSaveRef.current = null;
      void onAgentEdit(pending.agentId, {
        model_provider_id: pending.selection.providerId,
        model_id: pending.selection.modelId,
      }).catch((cause) => {
        toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
      });
    }
    modelSaveRequestRef.current += 1;
    if (modelSaveTimerRef.current !== undefined) {
      window.clearTimeout(modelSaveTimerRef.current);
      modelSaveTimerRef.current = undefined;
    }
    setSelectedModelValue(activeModelValue);
    setIsModelSavePending(false);
    setIsSavingModel(false);
  }, [activeAgentId, activeModelValue]);

  useEffect(() => () => {
    const pending = pendingModelSaveRef.current;
    pendingModelSaveRef.current = null;
    modelSaveRequestRef.current += 1;
    if (modelSaveTimerRef.current !== undefined) {
      window.clearTimeout(modelSaveTimerRef.current);
    }
    if (pending) {
      void onAgentEdit(pending.agentId, {
        model_provider_id: pending.selection.providerId,
        model_id: pending.selection.modelId,
      }).catch(() => undefined);
    }
  }, []);

  useEffect(() => {
    cancelActiveSend();
    clearChat();
    setConversationFilter('');
    setTerminalPermissionMode(null);
    if (!activeAgentId) return;
    void loadConversationsForAgent(activeAgentId);
    let cancelled = false;
    void readAgentToolsConfig(activeAgentId)
      .then((settings) => {
        if (!cancelled) setTerminalPermissionMode(settings.effective.terminal.permission_mode);
      })
      .catch(() => {
        if (!cancelled) setTerminalPermissionMode(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activeAgentId, cancelActiveSend, clearChat, loadConversationsForAgent]);

  useEffect(() => {
    if (!activeConversationId) return;
    void loadMessages(activeConversationId);
  }, [activeConversationId, loadMessages]);

  const isMessagesNearBottom = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  }, []);

  const updateScrollButtonVisibility = useCallback(() => {
    const el = messagesContainerRef.current;
    if (!el) {
      setIsAwayFromBottom(false);
      return;
    }
    const canScroll = el.scrollHeight > el.clientHeight + 8;
    setIsAwayFromBottom(canScroll && !isMessagesNearBottom());
  }, [isMessagesNearBottom]);

  const updateActiveTimelineItem = useCallback(() => {
    const container = messagesContainerRef.current;
    if (!container) return;
    const timelineAnchorIds = new Set(timelineItems.map((item) => item.anchorId));
    const anchors = Array.from(container.querySelectorAll<HTMLElement>('[data-message-anchor]'))
      .filter((anchor) => timelineAnchorIds.has(anchor.dataset.messageAnchor ?? ''));
    if (anchors.length === 0) {
      setActiveTimelineAnchor('');
      return;
    }
    const containerTop = container.getBoundingClientRect().top;
    const focusLine = containerTop + Math.min(container.clientHeight * 0.3, 220);
    let closest = anchors[0];
    let closestDistance = Math.abs(closest.getBoundingClientRect().top - focusLine);
    for (const anchor of anchors.slice(1)) {
      const distance = Math.abs(anchor.getBoundingClientRect().top - focusLine);
      if (distance < closestDistance) {
        closest = anchor;
        closestDistance = distance;
      }
    }
    setActiveTimelineAnchor(closest.dataset.messageAnchor ?? '');
  }, [timelineItems]);

  const handleMessagesScroll = useCallback(() => {
    updateScrollButtonVisibility();
    if (messagesScrollFrameRef.current !== undefined) return;
    messagesScrollFrameRef.current = window.requestAnimationFrame(() => {
      messagesScrollFrameRef.current = undefined;
      updateActiveTimelineItem();
    });
  }, [updateActiveTimelineItem, updateScrollButtonVisibility]);

  const scrollToBottom = useCallback(
    (force = false) => {
      const el = messagesContainerRef.current;
      if (!el) return;
      if (!force && !isMessagesNearBottom()) return;
      el.scrollTop = el.scrollHeight;
      setIsAwayFromBottom(false);
    },
    [isMessagesNearBottom],
  );

  useLayoutEffect(() => {
    if (!isLoadingChat && messages.length > 0) {
      scrollToBottom(true);
    }
  }, [isLoadingChat, messages.length, scrollToBottom]);

  useLayoutEffect(() => {
    scrollToBottom();
  }, [messages, isSending, scrollToBottom]);

  useLayoutEffect(() => {
    const input = composerInputRef.current;
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = `${Math.min(Math.max(input.scrollHeight, 48), 168)}px`;
  }, [draft]);

  useEffect(() => {
    const frameId = window.requestAnimationFrame(() => {
      updateScrollButtonVisibility();
      updateActiveTimelineItem();
    });
    return () => window.cancelAnimationFrame(frameId);
  }, [messages, updateActiveTimelineItem, updateScrollButtonVisibility]);

  useEffect(() => () => {
    if (messagesScrollFrameRef.current !== undefined) {
      window.cancelAnimationFrame(messagesScrollFrameRef.current);
    }
  }, []);

  useEffect(() => {
    const wheel = timelineWheelRef.current;
    if (!wheel || !activeTimelineAnchor) return;
    const activeOption = wheel.querySelector<HTMLElement>(`[data-timeline-target="${activeTimelineAnchor}"]`);
    if (!activeOption) return;
    wheel.scrollTo({
      top: activeOption.offsetTop - (wheel.clientHeight / 2) + (activeOption.offsetHeight / 2),
      behavior: 'smooth',
    });
  }, [activeTimelineAnchor]);

  function scrollToTimelineMessage(anchorId: string) {
    const container = messagesContainerRef.current;
    const target = container?.querySelector<HTMLElement>(`[data-message-anchor="${anchorId}"]`);
    if (!container || !target) return;
    container.scrollTo({
      top: Math.max(0, target.offsetTop - Math.min(container.clientHeight * 0.28, 220)),
      behavior: 'smooth',
    });
    setActiveTimelineAnchor(anchorId);
  }

  function handleAgentSelect(agentId: string) {
    onAgentChange(agentId);
  }

  function handleConversationSelect(conversationId: string) {
    if (isSending) cancelActiveSend();
    setActiveConversationId(conversationId);
    onOpenMain();
  }

  function handleSend(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (isSending) return;
    if (isModelSavePending || isSavingModel) return;
    const content = draft.trim();
    if (!activeAgent || !content) return;
    setDraft('');
    void send(content, activeAgent.id, isStreamMode);
    scrollToBottom(true);
  }

  function handleComposerKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    if (!activeAgent || isSending) return;
    composerFormRef.current?.requestSubmit();
  }

  async function handleModelChange(value: string) {
    if (!activeAgent || isSavingModel || isSending) return;
    const selection = modelOptions.find((option) => option.value === value);
    if (!selection) return;

    if (modelSaveTimerRef.current !== undefined) {
      window.clearTimeout(modelSaveTimerRef.current);
      modelSaveTimerRef.current = undefined;
    }

    if (value === activeModelValue) {
      pendingModelSaveRef.current = null;
      modelSaveRequestRef.current += 1;
      setSelectedModelValue(activeModelValue);
      setIsModelSavePending(false);
      return;
    }

    const requestId = modelSaveRequestRef.current + 1;
    modelSaveRequestRef.current = requestId;
    pendingModelSaveRef.current = {
      requestId,
      agentId: activeAgent.id,
      selection,
      previousValue: activeModelValue,
    };
    setSelectedModelValue(value);
    setIsModelSavePending(true);
    modelSaveTimerRef.current = window.setTimeout(() => {
      modelSaveTimerRef.current = undefined;
      void persistPendingModelSelection(requestId);
    }, MODEL_AUTO_SAVE_DELAY_MS);
  }

  async function persistPendingModelSelection(requestId: number) {
    const pending = pendingModelSaveRef.current;
    if (!pending || pending.requestId !== requestId || modelSaveRequestRef.current !== requestId) return;
    pendingModelSaveRef.current = null;
    setIsModelSavePending(false);
    setIsSavingModel(true);
    try {
      await onAgentEdit(pending.agentId, {
        model_provider_id: pending.selection.providerId,
        model_id: pending.selection.modelId,
      });
    } catch (cause) {
      if (modelSaveRequestRef.current === requestId) {
        setSelectedModelValue(pending.previousValue);
        setIsModelSavePending(false);
      }
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      if (modelSaveRequestRef.current === requestId) {
        setIsSavingModel(false);
      }
    }
  }

  function handleNewConversation() {
    if (!activeAgent) return;
    if (isSending) return;
    if (activeConversation && activeConversation.message_count === 0) {
      onOpenMain();
      return;
    }
    void createNewConversation(activeAgent.id)
      .then(() => onOpenMain())
      .catch((cause) => {
        toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
      });
  }

  async function handleDeleteConversation(conversationId: string) {
    if (!activeAgent) return;
    const confirmed = await confirm({
      title: t('conversation.delete'),
      message: t('conversation.deleteConfirm'),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;
    if (isSending && conversationId === activeConversationId) cancelActiveSend();
    try {
      await removeConversation(conversationId, activeAgent.id);
      toast.success(t('notification.conversationDeleted'));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    }
  }

  function formatTime(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  function formatDate(value: string) {
    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(value));
  }

  function formatTimelineTime(value: string) {
    const date = new Date(value);
    const now = new Date();
    const isSameYear = date.getFullYear() === now.getFullYear();
    const isSameDay = date.toDateString() === now.toDateString();

    if (isSameDay) return formatTime(value);

    return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en-US', {
      month: 'long',
      day: 'numeric',
      ...(isSameYear ? {} : { year: 'numeric' as const }),
      hour: '2-digit',
      minute: '2-digit',
    }).format(date);
  }

  function shouldShowTimeSeparator(message: UiMessage, previous?: UiMessage) {
    if (!previous) return true;
    const currentTime = new Date(message.created_at).getTime();
    const previousTime = new Date(previous.created_at).getTime();
    if (!Number.isFinite(currentTime) || !Number.isFinite(previousTime)) return false;
    return currentTime - previousTime >= 5 * 60 * 1000;
  }

  return (
    <>
      <section className="context-pane chat-list-pane" aria-label={t('chat.selectAgent')}>
        {!activeAgent ? (
          <>
            <PaneHeader
              className="mobile-pane-header"
              title={t('nav.chat')}
            />
            <div className="list-stack">
              {agents.length === 0 ? <p className="muted centered">{t('agentConfig.noAgents')}</p> : null}
              {agents.map((agent) => (
                <AgentListItem
                  key={agent.id}
                  agent={agent}
                  onClick={() => handleAgentSelect(agent.id)}
                />
              ))}
            </div>
          </>
        ) : (
          <>
            <PaneHeader
              className="conversation-pane-header"
              title={agentDisplayName(activeAgent, t)}
              leading={(
                <IconButton variant="ghost" type="button" onClick={() => onAgentChange('')} label={t('chat.selectAgent')}>
                  <ArrowLeft size={17} />
                </IconButton>
              )}
              actions={(
                <IconButton variant="ghost" type="button" onClick={handleNewConversation} disabled={isSending} label={t('conversation.create')}>
                  <Plus size={17} />
                </IconButton>
              )}
            />
            <SearchInput value={conversationFilter} onChange={setConversationFilter} placeholder={t('conversation.search')} />
            <div className="list-stack conversation-list">
              {conversations.length === 0 ? <p className="muted centered">{t('conversation.empty')}</p> : null}
              {filteredConversations.map((conversation) => (
                <ConversationRow
                  key={conversation.id}
                  conversation={conversation}
                  active={conversation.id === activeConversationId}
                  formatDate={formatDate}
                  deleteLabel={t('conversation.delete')}
                  onSelect={() => handleConversationSelect(conversation.id)}
                  onDelete={() => handleDeleteConversation(conversation.id)}
                />
              ))}
            </div>
          </>
        )}
      </section>

      <section className="main-pane chat-area" aria-label={t('chat.ariaLabel')}>
        <header className="chat-main-header">
          <Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />} aria-label={t('common.back')} title={t('common.back')}>
            {t('common.back')}
          </Button>
          <div className="chat-main-identity">
            <MessageAvatar agent={activeAgent} settings={settings} isUser={false} />
            <div>
              <strong>{activeAgent ? agentDisplayName(activeAgent, t) : t('chat.noAgent')}</strong>
              <span>{activeConversation?.title ?? (activeAgent ? t('conversation.newTitle') : t('chat.noAgentHint'))}</span>
            </div>
          </div>
          {activeAgent ? (
            <select
              className="chat-model-select"
              value={selectedModelValue}
              aria-label={t('chat.selectModel')}
              aria-busy={isModelSavePending || isSavingModel}
              disabled={modelOptions.length === 0 || isSavingModel || isSending}
              title={t('chat.selectModel')}
              onChange={(event) => void handleModelChange(event.target.value)}
            >
              <option value="" disabled>{t('chat.selectModel')}</option>
              {modelOptions.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </select>
          ) : null}
        </header>
        <div className="messages" aria-live="polite" ref={messagesContainerRef} onScroll={handleMessagesScroll}>
          {isLoadingChat ? <p className="muted centered">{t('chat.loading')}</p> : null}
          {!isLoadingChat && messages.length === 0 ? (
            <EmptyState
              icon={activeAgent ? <MessageSquare size={34} /> : <Bot size={34} />}
              title={activeAgent ? t('chat.emptyTitle') : t('chat.noAgent')}
              description={activeAgent ? t('chat.emptyHint') : t('chat.noAgentHint')}
            />
          ) : null}
          {messages.map((item, index) => {
            if ('type' in item && item.type === 'tool_call') {
              const toolMsg = item as ToolCallMessage;
              return (
                <div key={toolMsg.call_id} className="message-row assistant message-row--tool">
                  {!false && <MessageAvatar agent={activeAgent} settings={settings} isUser={false} />}
                  <div className="message-body">
                    <div className="message-meta"><strong>{activeAgent ? agentDisplayName(activeAgent, t) : 'Assistant'}</strong></div>
                    <div className="message-bubble message-bubble--tool">
                      <ToolCallCard
                        agentId={activeAgent?.id ?? ''}
                        callId={toolMsg.call_id}
                        name={toolMsg.name}
                        arguments={toolMsg.arguments}
                        status={toolMsg.status}
                        result={toolMsg.result}
                        permissionMode={terminalPermissionMode}
                        onApprove={async (decision) => {
                          const approvalId = toolMsg.arguments.approval_id;
                          if (!approvalId) return;
                          try {
                            const approvalAgentId = toolMsg.arguments.agent_id || activeAgent?.id;
                            if (!approvalAgentId) return;
                            await approveToolCall(approvalId, approvalAgentId, decision);
                          } catch (e) {
                            toast.error(e instanceof Error ? e.message : t('error.unknown'));
                            throw e;
                          }
                        }}
                      />
                    </div>
                  </div>
                </div>
              );
            }
            const msg = item as UiMessage;
            const prev = messages[index - 1];
            const prevUi = prev && 'role' in prev ? prev as UiMessage : undefined;
            return (
              <MessageRow
                key={msg.clientId}
                anchorId={`message-anchor-${index}`}
                message={msg}
                showTimeSeparator={shouldShowTimeSeparator(msg, prevUi)}
                timeSeparatorLabel={formatTimelineTime(msg.created_at)}
                participantName={msg.role === 'user' ? t('chat.you') : activeAgent ? agentDisplayName(activeAgent, t) : 'Assistant'}
                agent={activeAgent}
                settings={settings}
                failedLabel={t('chat.failed')}
              />
            );
          })}
          {shouldShowAgentWaitingBubble(messages, isSending, runPhase) ? (
            <article className="message-row assistant is-streaming is-waiting" aria-live="polite">
              <MessageAvatar agent={activeAgent} settings={settings} isUser={false} />
              <div className="message-body">
                <div className="message-meta">
                  <strong>{activeAgent ? agentDisplayName(activeAgent, t) : 'Assistant'}</strong>
                </div>
                <div className="message-bubble">
                  <TypingIndicator label={t('chat.sending')} />
                </div>
              </div>
            </article>
          ) : null}
          <div ref={messagesEndRef} />
        </div>

        {timelineItems.length > 1 ? (
          <MessageTimeNavigator
            items={timelineItems}
            activeAnchor={activeTimelineAnchor}
            wheelRef={timelineWheelRef}
            label={t('chat.timeline')}
            onSelect={scrollToTimelineMessage}
          />
        ) : null}

        {isAwayFromBottom ? (
          <button
            className="scroll-to-bottom-button"
            type="button"
            onClick={() => scrollToBottom(true)}
            aria-label={t('chat.scrollToBottom')}
            title={t('chat.scrollToBottom')}
          >
            <ChevronDown size={22} strokeWidth={2.4} />
          </button>
        ) : null}

        <form className="composer" ref={composerFormRef} onSubmit={handleSend}>
          <div className="composer-box">
            <textarea
              ref={composerInputRef}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={handleComposerKeyDown}
              placeholder={composerPlaceholder}
              aria-label={t('chat.messageLabel')}
              disabled={isComposerDisabled}
              rows={1}
            />
            <div className="composer-toolbar">
              <button
                type="button"
                className={`stream-switch${isStreamMode ? ' is-active' : ''}`}
                role="switch"
                aria-checked={isStreamMode}
                aria-label={t('chat.stream')}
                onClick={() => setIsStreamMode((current) => !current)}
              >
                <span className="stream-switch-track" aria-hidden="true"><span /></span>
                <span>{t('chat.stream')}</span>
              </button>
              <div className="composer-actions">
                <Button
                  className={`send-button${isSending ? ' is-stopping' : ''}`}
                  variant={isSending ? 'danger' : 'primary'}
                  type={isSending ? 'button' : 'submit'}
                  onClick={isSending ? cancelActiveSend : undefined}
                  disabled={isComposerDisabled || (!isSending && (isModelSavePending || isSavingModel || !draft.trim()))}
                  icon={isSending ? <Square className="stop-icon" size={16} /> : <Send size={17} />}
                >
                  {isSending ? t('chat.stop') : t('chat.send')}
                </Button>
              </div>
            </div>
          </div>
        </form>
      </section>
    </>
  );
}


function shouldShowAgentWaitingBubble(
  messages: UiMessageItem[],
  isSending: boolean,
  runPhase: AgentRunPhase,
): boolean {
  if (!isSending) return false;
  if (runPhase === 'waiting_for_approval') return false;

  const last = messages[messages.length - 1];
  if (!last) return true;

  // Tool agent-loop: keep a typing bubble under tool cards, except while waiting for user approval.
  if ('type' in last && last.type === 'tool_call') {
    return last.status !== 'pending_approval';
  }

  if ('role' in last && last.role === 'assistant') {
    // Empty streaming assistant already renders dots inside the bubble.
    // Streaming text already communicates progress.
    // Finalized assistant content should not flash a trailing waiting bubble before `done`.
    return last.status !== 'streaming' && runPhase === 'executing_tool';
  }

  // e.g. last item is a user message and the optimistic assistant bubble is missing.
  return true;
}

type MessageTimelineItem = {
  anchorId: string;
  dateLabel: string;
  timeLabel: string;
  preview: string;
};

function MessageTimeNavigator({
  items,
  activeAnchor,
  wheelRef,
  label,
  onSelect,
}: {
  items: MessageTimelineItem[];
  activeAnchor: string;
  wheelRef: RefObject<HTMLDivElement | null>;
  label: string;
  onSelect: (anchorId: string) => void;
}) {
  const dragStateRef = useRef<{
    pointerId: number;
    startY: number;
    startScrollTop: number;
    moved: boolean;
  } | null>(null);
  const suppressClickRef = useRef(false);
  const [isDragging, setIsDragging] = useState(false);

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.button !== 0 || !wheelRef.current) return;
    dragStateRef.current = {
      pointerId: event.pointerId,
      startY: event.clientY,
      startScrollTop: wheelRef.current.scrollTop,
      moved: false,
    };
    wheelRef.current.setPointerCapture(event.pointerId);
    setIsDragging(true);
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    const dragState = dragStateRef.current;
    const wheel = wheelRef.current;
    if (!dragState || !wheel || dragState.pointerId !== event.pointerId) return;
    const delta = event.clientY - dragState.startY;
    if (Math.abs(delta) > 3) dragState.moved = true;
    if (dragState.moved) event.preventDefault();
    wheel.scrollTop = dragState.startScrollTop - delta;
  }

  function finishPointerDrag(event: ReactPointerEvent<HTMLDivElement>) {
    const dragState = dragStateRef.current;
    const wheel = wheelRef.current;
    if (!dragState || !wheel || dragState.pointerId !== event.pointerId) return;
    if (wheel.hasPointerCapture(event.pointerId)) wheel.releasePointerCapture(event.pointerId);
    dragStateRef.current = null;
    setIsDragging(false);
    if (!dragState.moved) return;

    suppressClickRef.current = true;
    window.setTimeout(() => { suppressClickRef.current = false; }, 0);
    const wheelCenter = wheel.getBoundingClientRect().top + (wheel.clientHeight / 2);
    const options = Array.from(wheel.querySelectorAll<HTMLElement>('[data-timeline-target]'));
    const closest = options.reduce<HTMLElement | null>((best, option) => {
      if (!best) return option;
      const optionCenter = option.getBoundingClientRect().top + (option.offsetHeight / 2);
      const bestCenter = best.getBoundingClientRect().top + (best.offsetHeight / 2);
      return Math.abs(optionCenter - wheelCenter) < Math.abs(bestCenter - wheelCenter) ? option : best;
    }, null);
    const anchorId = closest?.dataset.timelineTarget;
    if (anchorId) onSelect(anchorId);
  }

  return (
    <aside className={`message-time-navigator${isDragging ? ' is-dragging' : ''}`} aria-label={label}>
      <div className="message-time-navigator__marker" aria-hidden="true">
        <Clock3 size={13} />
        <span />
      </div>
      <div
        className="message-time-wheel"
        ref={wheelRef}
        role="listbox"
        aria-label={label}
        tabIndex={0}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={finishPointerDrag}
        onPointerCancel={finishPointerDrag}
      >
        {items.map((item) => (
          <button
            key={item.anchorId}
            className={`message-time-option${activeAnchor === item.anchorId ? ' is-active' : ''}`}
            type="button"
            role="option"
            aria-selected={activeAnchor === item.anchorId}
            data-timeline-target={item.anchorId}
            title={item.preview || `${item.dateLabel} ${item.timeLabel}`}
            onClick={() => {
              if (suppressClickRef.current) {
                suppressClickRef.current = false;
                return;
              }
              onSelect(item.anchorId);
            }}
          >
            <span>{item.dateLabel}</span>
            <strong>{item.timeLabel}</strong>
          </button>
        ))}
      </div>
    </aside>
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

function MessageRow({
  anchorId,
  message,
  showTimeSeparator,
  timeSeparatorLabel,
  participantName,
  agent,
  settings,
  failedLabel,
}: {
  anchorId: string;
  message: UiMessage;
  showTimeSeparator: boolean;
  timeSeparatorLabel: string;
  participantName: string;
  agent?: Agent;
  settings: AppSettings;
  failedLabel: string;
}) {
  const isUser = message.role === 'user';

  return (
    <>
      {showTimeSeparator ? (
        <div className="message-time-separator">
          <time dateTime={message.created_at}>{timeSeparatorLabel}</time>
        </div>
      ) : null}
      <article
        className={['message-row', message.role, message.status === 'streaming' ? 'is-streaming' : ''].filter(Boolean).join(' ')}
        data-message-anchor={anchorId}
      >
        {!isUser ? <MessageAvatar agent={agent} settings={settings} isUser={false} /> : null}
        <div className="message-body">
          <div className="message-meta">
            <strong>{participantName}</strong>
          </div>
          <div className="message-bubble">
            <MessageContent message={message} />
            {message.status === 'failed' ? <span className="message-status error">{failedLabel}</span> : null}
          </div>
        </div>
        {isUser ? <MessageAvatar agent={agent} settings={settings} isUser /> : null}
      </article>
    </>
  );
}

function MessageAvatar({ agent, settings, isUser }: { agent?: Agent; settings: AppSettings; isUser: boolean }) {
  if (isUser) {
    return <AvatarBadge icon={settings.avatar_icon} color={settings.avatar_color} size={23} className="chat-avatar" />;
  }

  if (agent) {
    return <AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} size={23} className="chat-avatar" />;
  }

  return <span className="chat-avatar avatar-muted"><Bot size={23} /></span>;
}

function MessageContent({ message }: { message: UiMessage }) {
  const { t } = useI18n();

  if (message.status === 'streaming' && !message.content) {
    return <TypingIndicator label={t('chat.sending')} />;
  }

  if (message.status === 'streaming') {
    return (
      <div className="message-markdown streaming-content">
        <MarkdownContent content={message.content} />
      </div>
    );
  }

  return (
    <div className="message-markdown">
      <MarkdownContent content={message.content} />
    </div>
  );
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const text = extractText(children);
  const language = extractLanguage(children);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="code-block-wrap">
      <span className="code-language-label">{language}</span>
      <button className="code-copy-button" type="button" onClick={handleCopy} disabled={!text} aria-label={t('chat.copyCode')}>
        {copied ? <Check size={14} /> : <Clipboard size={14} />}
        <span>{copied ? t('chat.copied') : t('chat.copy')}</span>
      </button>
      <pre>{children}</pre>
    </div>
  );
}

function extractLanguage(node: ReactNode): string {
  const className = findCodeClassName(node);
  const raw = className?.match(/language-([\w-]+)/)?.[1] ?? 'text';
  return formatLanguage(raw);
}

function findCodeClassName(node: ReactNode): string | undefined {
  if (Array.isArray(node)) {
    return node.map(findCodeClassName).find(Boolean);
  }
  if (!isValidElement<{ className?: string; children?: ReactNode }>(node)) {
    return undefined;
  }
  return node.props.className ?? findCodeClassName(node.props.children);
}

function formatLanguage(language: string): string {
  const normalized = language.toLowerCase();
  const labels: Record<string, string> = {
    js: 'JS',
    javascript: 'JS',
    jsx: 'JSX',
    ts: 'TS',
    typescript: 'TS',
    tsx: 'TSX',
    html: 'HTML',
    css: 'CSS',
    json: 'JSON',
    md: 'MD',
    markdown: 'MD',
    py: 'Python',
    python: 'Python',
    rs: 'Rust',
    rust: 'Rust',
    sh: 'Shell',
    bash: 'Shell',
    powershell: 'PowerShell',
    ps1: 'PowerShell',
    text: 'Text',
    txt: 'Text',
  };
  return labels[normalized] ?? normalized.toUpperCase();
}

function extractText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(extractText).join('');
  if (isValidElement<{ children?: ReactNode }>(node)) return extractText(node.props.children);
  return '';
}

function MarkdownContent({ content }: { content: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm, remarkMath]}
      rehypePlugins={[rehypeKatex, rehypeHighlight]}
      components={{ pre: CodeBlock }}
    >
      {normalizeMathMarkdown(content)}
    </ReactMarkdown>
  );
}

function normalizeMathMarkdown(content: string): string {
  return content.replace(/\\\[([\s\S]*?)\\\]/g, (_match, formula: string) => {
    return `$$\n${formula.trim()}\n$$`;
  }).replace(/\\\(([\s\S]*?)\\\)/g, (_match, formula: string) => `$${formula.trim()}$`);
}

function ConversationRow({
  conversation,
  active,
  formatDate,
  deleteLabel,
  onSelect,
  onDelete,
}: {
  conversation: Conversation;
  active: boolean;
  formatDate: (value: string) => string;
  deleteLabel: string;
  onSelect: () => void;
  onDelete: () => void;
}) {
  return (
    <div className={['conversation-row', active ? 'is-active' : ''].filter(Boolean).join(' ')}>
      <ListItem
        className="conversation-row-main"
        type="button"
        title={conversation.title}
        subtitle={formatDate(conversation.updated_at)}
        leading={<span className="avatar-badge avatar-muted"><MessageSquare size={17} /></span>}
        onClick={onSelect}
      />
      <span className="row-actions">
        <IconButton variant="danger" type="button" onClick={onDelete} label={deleteLabel}>
          <Trash2 size={15} />
        </IconButton>
      </span>
    </div>
  );
}
