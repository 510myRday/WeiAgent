import { useRef, useEffect, useCallback, useMemo, useState } from 'react';
import {
  ArrowLeft,
  LoaderCircle,
  Pin,
  Plus,
  Trash2,
} from 'lucide-react';
import type {
  Agent,
  AppSettings,
  Conversation,
  ImageAttachment,
  ModelProvider,
  ThinkingLevel,
  UiMessageItem,
  UpdateAgentPayload,
} from '../../types';
import { approveToolCall, readAgentToolsConfig, type AgentToolsConfig } from '../../api';
import { useI18n } from '../../i18n';
import { useConversations, usePageVisibility } from '../../hooks';
import { AgentListItem, agentDisplayName } from '../../components/AgentListItem';
import { Button, IconButton, ListItem, PaneHeader, SearchInput, useFeedback } from '../../components/ui';
import { ChatComposer } from './ChatComposer';
import { ChatAgentSelect } from './ChatAgentSelect';
import { MessageAvatar, MessageList, type MessageListHandle } from './MessageList';
import { MessageTimeline, type MessageTimelineItem } from './MessageTimeline';
import { userErrorMessage } from '../../utils/userError';

interface ChatPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  activeAgentId: string;
  onAgentChange: (id: string) => void;
  onAgentEdit: (id: string, payload: UpdateAgentPayload) => Promise<Agent>;
  onOpenMain: () => void;
  onBackToList: () => void;
  onCreateAgent: () => void;
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
const EMPTY_MESSAGES: UiMessageItem[] = [];

export function ChatPanel({
  agents,
  providers,
  activeAgentId,
  onAgentChange,
  onAgentEdit,
  onOpenMain,
  onBackToList,
  onCreateAgent,
  settings,
}: ChatPanelProps) {
  const { t, locale } = useI18n();
  const { confirm, toast } = useFeedback();
  const {
    conversations,
    activeConversationId,
    selectConversation,
    messages,
    conversationUsage,
    hasOlderMessages,
    isLoadingOlderMessages,
    isLoadingConversations,
    conversationsError,
    isLoadingChat,
    isSending,
    runPhase,
    retryStatus,
    runningConversationIds,
    loadConversationsForAgent,
    refreshRunningConversations,
    loadMessages,
    loadOlderMessages,
    resumeActiveRun,
    detachActiveRun,
    cancelActiveSend,
    clearChat,
    send,
    createNewConversation,
    removeConversation,
    setConversationPinned,
  } = useConversations({
    newConversationTitle: t('conversation.newTitle'),
    runCancelledMessage: t('chat.runCancelled'),
    runInterruptedMessage: t('chat.runInterrupted'),
    onError: toast.error,
  });

  const [isStreamMode, setIsStreamMode] = useState(true);
  const [thinkingLevel, setThinkingLevel] = useState<ThinkingLevel>('medium');
  const [modelTargetAgentId, setModelTargetAgentId] = useState('');
  const [selectedModelValue, setSelectedModelValue] = useState('');
  const [isModelSavePending, setIsModelSavePending] = useState(false);
  const [isSavingModel, setIsSavingModel] = useState(false);
  const [conversationFilter, setConversationFilter] = useState('');
  const [pinningConversationIds, setPinningConversationIds] = useState<Set<string>>(() => new Set());
  const [activeTimelineMessageIndex, setActiveTimelineMessageIndex] = useState<number | null>(null);
  const [terminalPermissionMode, setTerminalPermissionMode] = useState<AgentToolsConfig['terminal']['permission_mode'] | null>(null);
  const isPageVisible = usePageVisibility();

  const conversationSearchRef = useRef<HTMLInputElement>(null);
  const messageListRef = useRef<MessageListHandle>(null);
  const modelSaveTimerRef = useRef<number | undefined>(undefined);
  const modelSaveRequestRef = useRef(0);
  const pendingModelSaveRef = useRef<PendingModelSave | null>(null);

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const conversationAgentIdKey = useMemo(() => {
    const agentIds = new Set<string>();
    if (activeAgentId) agentIds.add(activeAgentId);
    for (const item of messages) {
      if (item.agent_id) agentIds.add(item.agent_id);
    }
    return [...agentIds].join('\n');
  }, [activeAgentId, messages]);
  const conversationAgents = useMemo(() => {
    return conversationAgentIdKey
      .split('\n')
      .filter(Boolean)
      .map((agentId) => agents.find((agent) => agent.id === agentId))
      .filter((agent): agent is Agent => Boolean(agent));
  }, [agents, conversationAgentIdKey]);
  const modelTargetAgent = conversationAgents.find((agent) => agent.id === modelTargetAgentId)
    ?? activeAgent;
  const activeProvider = providers.find(
    (provider) => provider.id === modelTargetAgent?.model_provider_id,
  );
  const activeModelConfig = activeProvider?.model_configs.find(
    (config) => config.model_id === modelTargetAgent?.model_id,
  );
  const supportsImages = activeModelConfig?.multimodal === true;
  const supportsThinking = activeModelConfig?.thinking === true;
  const modelOptions = useMemo<ModelOption[]>(() => providers.flatMap((provider) =>
    provider.models.map((model) => ({
      value: JSON.stringify([provider.id, model]),
      providerId: provider.id,
      modelId: model,
      label: `${provider.name} | ${model}`,
    })),
  ), [providers]);
  const activeModelValue = modelTargetAgent
    ? JSON.stringify([modelTargetAgent.model_provider_id, modelTargetAgent.model_id])
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
  const visibleMessages = activeAgent ? messages : EMPTY_MESSAGES;
  const activeConversation = activeAgent
    ? conversations.find((conversation) => conversation.id === activeConversationId)
    : undefined;
  const filteredConversations = conversations.filter((conversation) =>
    conversation.title.toLowerCase().includes(conversationFilter.trim().toLowerCase()),
  );
  const dateFormatters = useMemo(() => {
    const formatLocale = locale === 'zh' ? 'zh-CN' : 'en-US';
    return {
      time: new Intl.DateTimeFormat(formatLocale, { hour: '2-digit', minute: '2-digit' }),
      dateTime: new Intl.DateTimeFormat(formatLocale, {
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      }),
      timelineDate: new Intl.DateTimeFormat(formatLocale, { month: 'long', day: 'numeric' }),
      timelineDateWithYear: new Intl.DateTimeFormat(formatLocale, {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
      }),
      markerDate: new Intl.DateTimeFormat(formatLocale, { month: '2-digit', day: '2-digit' }),
      markerTime: new Intl.DateTimeFormat(formatLocale, {
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      }),
    };
  }, [locale]);
  const timelineItems = useMemo<MessageTimelineItem[]>(() => {
    const result: MessageTimelineItem[] = [];
    let previousTimelineTimestamp: number | undefined;
    visibleMessages.forEach((item, index) => {
      const date = new Date(item.created_at);
      const timestamp = date.getTime();
      if (!Number.isFinite(timestamp)) return;
      if (previousTimelineTimestamp !== undefined && timestamp - previousTimelineTimestamp < 60_000) return;
      previousTimelineTimestamp = timestamp;
      result.push({
        messageIndex: index,
        dateLabel: dateFormatters.markerDate.format(date),
        timeLabel: dateFormatters.markerTime.format(date),
        preview: item.content.trim().replace(/\s+/g, ' ').slice(0, 48),
      });
    });
    return result;
  }, [activeConversationId, dateFormatters, visibleMessages.length]);
  const timelineMessageIndexes = useMemo(
    () => timelineItems.map((item) => item.messageIndex),
    [timelineItems],
  );

  useEffect(() => {
    setModelTargetAgentId((current) => (
      conversationAgents.some((agent) => agent.id === current)
        ? current
        : conversationAgents[0]?.id ?? ''
    ));
  }, [activeConversationId, conversationAgents]);

  useEffect(() => {
    const pending = pendingModelSaveRef.current;
    if (pending) {
      pendingModelSaveRef.current = null;
      void onAgentEdit(pending.agentId, {
        model_provider_id: pending.selection.providerId,
        model_id: pending.selection.modelId,
      }).catch((cause) => {
        toast.error(conversationActionError(cause));
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
    // Thinking-capable models default to medium; others stay off (no thinking params).
    setThinkingLevel(supportsThinking ? 'medium' : 'off');
  }, [activeModelValue, modelTargetAgentId, supportsThinking]);

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
    detachActiveRun();
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
  }, [activeAgentId, clearChat, detachActiveRun, loadConversationsForAgent]);

  useEffect(() => {
    if (!activeAgentId || !isPageVisible) return;
    void refreshRunningConversations(activeAgentId);
    const timer = window.setInterval(() => {
      void refreshRunningConversations(activeAgentId);
    }, 3000);
    return () => window.clearInterval(timer);
  }, [activeAgentId, isPageVisible, refreshRunningConversations]);

  useEffect(() => {
    if (!activeConversationId) return;
    let cancelled = false;
    void loadMessages(activeConversationId)
      .then((loaded) => {
        if (!cancelled && loaded) {
          return resumeActiveRun(activeConversationId, activeAgentId);
        }
        return undefined;
      })
      .catch((cause) => {
        if (!cancelled) {
          toast.error(conversationActionError(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeAgentId, activeConversationId, loadMessages, resumeActiveRun]);

  function handleAgentSelect(agentId: string) {
    onAgentChange(agentId);
  }

  function handleBackToAgents() {
    detachActiveRun();
    clearChat();
    setConversationFilter('');
    setTerminalPermissionMode(null);
    onAgentChange('');
  }

  function handleConversationSelect(conversationId: string) {
    if (isSending) detachActiveRun();
    if (conversationId !== activeConversationId) selectConversation(conversationId);
    onOpenMain();
  }

  const handleRestartMessage = useCallback((
    messageId: string,
    content: string,
    attachments: ImageAttachment[] = [],
  ) => {
    if (
      !activeAgent
      || isSending
      || !messageId
      || (!content.trim() && attachments.length === 0)
    ) return;
    void send(
      content.trim(),
      activeAgent.id,
      isStreamMode,
      attachments,
      supportsThinking ? thinkingLevel : 'off',
      messageId,
    );
  }, [activeAgent, isSending, isStreamMode, send, supportsThinking, thinkingLevel]);

  async function handleModelChange(value: string) {
    if (!modelTargetAgent || isSavingModel || isSending) return;
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
      agentId: modelTargetAgent.id,
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
      toast.error(conversationActionError(cause));
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
        toast.error(conversationActionError(cause));
      });
  }

  function conversationActionError(cause: unknown) {
    return userErrorMessage(cause, {
      summary: t('error.conversationActionFailed'),
      recovery: t('error.retryConnection'),
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
      toast.error(conversationActionError(cause));
    }
  }

  async function handlePinConversation(conversation: Conversation) {
    setPinningConversationIds((current) => new Set(current).add(conversation.id));
    try {
      await setConversationPinned(conversation.id, !conversation.is_pinned);
      toast.success(t(conversation.is_pinned
        ? 'notification.conversationUnpinned'
        : 'notification.conversationPinned'));
    } catch (cause) {
      toast.error(conversationActionError(cause));
    } finally {
      setPinningConversationIds((current) => {
        const next = new Set(current);
        next.delete(conversation.id);
        return next;
      });
    }
  }

  const handleSendMessage = useCallback((
    content: string,
    streamMode: boolean,
    attachments: ImageAttachment[],
  ) => {
    if (!activeAgent || isSending || isModelSavePending || isSavingModel) return;
    void send(
      content,
      activeAgent.id,
      streamMode,
      attachments,
      supportsThinking ? thinkingLevel : 'off',
    );
  }, [
    activeAgent,
    isModelSavePending,
    isSavingModel,
    isSending,
    send,
    supportsThinking,
    thinkingLevel,
  ]);

  const handleLoadOlderMessages = useCallback(async () => {
    if (!activeConversationId || isLoadingOlderMessages) return false;
    return loadOlderMessages(activeConversationId);
  }, [activeConversationId, isLoadingOlderMessages, loadOlderMessages]);

  const handleApproveToolCall = useCallback(async (
    approvalId: string,
    agentId: string,
    decision: 'approve' | 'reject' | 'always_approve',
  ) => {
    await approveToolCall(approvalId, agentId, decision);
  }, []);

  const handleTimelineSelect = useCallback((messageIndex: number) => {
    messageListRef.current?.scrollToMessage(messageIndex);
    setActiveTimelineMessageIndex(messageIndex);
  }, []);

  useEffect(() => {
    const handleShortcut = (event: globalThis.KeyboardEvent) => {
      if (event.repeat || document.querySelector('[role="dialog"]')) return;
      const commandKey = event.ctrlKey || event.metaKey;
      if (commandKey && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        conversationSearchRef.current?.focus();
        return;
      }
      if (commandKey && event.key.toLowerCase() === 'n' && activeAgent && !isSending) {
        event.preventDefault();
        handleNewConversation();
        return;
      }
      if (!commandKey && !event.altKey && event.key === 'Escape' && isSending) {
        event.preventDefault();
        cancelActiveSend();
      }
    };
    window.addEventListener('keydown', handleShortcut);
    return () => window.removeEventListener('keydown', handleShortcut);
  }, [activeAgent?.id, activeConversation?.id, activeConversation?.message_count, cancelActiveSend, isSending]);

  const formatTime = useCallback((value: string) => {
    return dateFormatters.time.format(new Date(value));
  }, [dateFormatters]);

  const formatDate = useCallback((value: string) => {
    return dateFormatters.dateTime.format(new Date(value));
  }, [dateFormatters]);

  const formatTimelineTime = useCallback((value: string) => {
    const date = new Date(value);
    const now = new Date();
    const isSameYear = date.getFullYear() === now.getFullYear();
    const isSameDay = date.toDateString() === now.toDateString();

    if (isSameDay) return formatTime(value);

    const dateLabel = (isSameYear
      ? dateFormatters.timelineDate
      : dateFormatters.timelineDateWithYear).format(date);
    return `${dateLabel} ${dateFormatters.time.format(date)}`;
  }, [dateFormatters, formatTime]);

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
              {agents.length === 0 ? (
                <div className="list-empty-action">
                  <p className="muted centered">{t('agentConfig.noAgents')}</p>
                  <Button variant="primary" type="button" onClick={onCreateAgent} icon={<Plus size={16} />}>
                    {t('agentConfig.create')}
                  </Button>
                </div>
              ) : null}
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
                <IconButton variant="ghost" type="button" onClick={handleBackToAgents} label={t('chat.selectAgent')}>
                  <ArrowLeft size={17} />
                </IconButton>
              )}
              actions={(
                <IconButton variant="ghost" type="button" onClick={handleNewConversation} disabled={isSending} label={t('conversation.create')}>
                  <Plus size={17} />
                </IconButton>
              )}
            />
            <SearchInput ref={conversationSearchRef} value={conversationFilter} onChange={setConversationFilter} placeholder={t('conversation.search')} />
            <div className="list-stack conversation-list">
              {isLoadingConversations && conversations.length === 0 ? (
                <p className="muted centered conversation-list-state" role="status">{t('conversation.loading')}</p>
              ) : null}
              {conversationsError ? (
                <div className="conversation-list-state conversation-list-error" role="alert">
                  <strong>{t('conversation.loadFailed')}</strong>
                  <span>{conversationsError}</span>
                  <Button variant="secondary" type="button" onClick={() => void loadConversationsForAgent(activeAgent.id)}>
                    {t('common.retry')}
                  </Button>
                </div>
              ) : null}
              {!isLoadingConversations && !conversationsError && conversations.length === 0 ? (
                <div className="conversation-list-state list-empty-action">
                  <p className="muted centered">{t('conversation.empty')}</p>
                  <Button variant="secondary" type="button" onClick={handleNewConversation} icon={<Plus size={15} />}>
                    {t('conversation.create')}
                  </Button>
                </div>
              ) : null}
              {!isLoadingConversations
                && !conversationsError
                && conversations.length > 0
                && filteredConversations.length === 0 ? (
                  <p className="muted centered conversation-list-state">{t('conversation.noSearchResults')}</p>
                ) : null}
              {filteredConversations.map((conversation) => (
                <ConversationRow
                  key={conversation.id}
                  conversation={conversation}
                  active={conversation.id === activeConversationId}
                  isRunning={runningConversationIds.has(conversation.id)}
                  formatDate={formatDate}
                  runStatusLabel={t('conversation.runStatus')}
                  deleteLabel={t('conversation.delete')}
                  pinLabel={conversation.is_pinned ? t('conversation.unpin') : t('conversation.pin')}
                  isPinning={pinningConversationIds.has(conversation.id)}
                  onSelect={() => handleConversationSelect(conversation.id)}
                  onTogglePin={() => handlePinConversation(conversation)}
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
            <div className="chat-header-controls">
              <ChatAgentSelect
                agents={conversationAgents}
                value={modelTargetAgent?.id ?? ''}
                disabled={isSavingModel}
                onChange={setModelTargetAgentId}
              />
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
            </div>
          ) : null}
        </header>
        <MessageList
          ref={messageListRef}
          conversationId={activeConversationId}
          messages={visibleMessages}
          activeAgent={activeAgent}
          settings={settings}
          isLoading={isLoadingChat}
          hasOlderMessages={hasOlderMessages}
          isLoadingOlderMessages={isLoadingOlderMessages}
          isSending={isSending}
          runPhase={runPhase}
          retryStatus={retryStatus}
          terminalPermissionMode={terminalPermissionMode}
          timelineMessageIndexes={timelineMessageIndexes}
          onLoadOlder={handleLoadOlderMessages}
          onRestart={handleRestartMessage}
          onApproveToolCall={handleApproveToolCall}
          onActiveTimelineMessageChange={setActiveTimelineMessageIndex}
          formatTimelineTime={formatTimelineTime}
          onCreateAgent={onCreateAgent}
        />

        {timelineItems.length > 1 ? (
          <MessageTimeline
            items={timelineItems}
            activeMessageIndex={activeTimelineMessageIndex}
            label={t('chat.timeline')}
            onSelect={handleTimelineSelect}
          />
        ) : null}

        <ChatComposer
          disabled={isComposerDisabled}
          placeholder={composerPlaceholder}
          draftKey={`${activeAgent?.id ?? 'none'}:${activeConversationId || 'new'}`}
          isSending={isSending}
          isSavingModel={isSavingModel}
          isModelSavePending={isModelSavePending}
          isStreamMode={isStreamMode}
          onStreamModeChange={setIsStreamMode}
          supportsImages={supportsImages}
          supportsThinking={supportsThinking}
          thinkingLevel={thinkingLevel}
          onThinkingLevelChange={setThinkingLevel}
          usage={conversationUsage}
          contextSize={activeModelConfig?.context_size ?? 128000}
          onUnsupportedImage={() => toast.warning(t('chat.imageUnsupportedModel'))}
          onSend={handleSendMessage}
          onStop={cancelActiveSend}
        />
      </section>
    </>
  );
}
function ConversationRow({
  conversation,
  active,
  isRunning,
  formatDate,
  runStatusLabel,
  deleteLabel,
  pinLabel,
  isPinning,
  onSelect,
  onTogglePin,
  onDelete,
}: {
  conversation: Conversation;
  active: boolean;
  isRunning: boolean;
  formatDate: (value: string) => string;
  runStatusLabel: string;
  deleteLabel: string;
  pinLabel: string;
  isPinning: boolean;
  onSelect: () => void;
  onTogglePin: () => void;
  onDelete: () => void;
}) {
  return (
    <div className={[
      'conversation-row',
      active ? 'is-active' : '',
      conversation.is_pinned ? 'is-pinned' : '',
    ].filter(Boolean).join(' ')}>
      <ListItem
        className="conversation-row-main"
        type="button"
        title={conversation.title}
        subtitle={(
          <span className={isRunning ? 'conversation-run-status' : undefined}>
            {isRunning ? <LoaderCircle size={12} aria-hidden="true" /> : null}
            <span>{isRunning ? runStatusLabel : formatDate(conversation.updated_at)}</span>
          </span>
        )}
        onClick={onSelect}
      />
      <IconButton
        className="conversation-action conversation-pin-action"
        variant="ghost"
        type="button"
        active={conversation.is_pinned}
        disabled={isPinning}
        aria-pressed={conversation.is_pinned}
        onClick={onTogglePin}
        label={pinLabel}
      >
        <Pin size={14} fill={conversation.is_pinned ? 'currentColor' : 'none'} />
      </IconButton>
      <IconButton
        className="conversation-action conversation-delete-action"
        variant="ghost"
        type="button"
        onClick={onDelete}
        label={deleteLabel}
      >
        <Trash2 size={14} />
      </IconButton>
    </div>
  );
}
