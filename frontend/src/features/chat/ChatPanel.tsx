import { useRef, useEffect, useCallback, useMemo, useState, memo } from 'react';
import {
  ArrowLeft,
  CheckCircle2,
  ChevronDown,
  Circle,
  CircleDot,
  LoaderCircle,
  ListChecks,
  Pin,
  Plus,
  Settings2,
  Trash2,
} from 'lucide-react';
import type {
  Agent,
  AppSettings,
  Conversation,
  ImageModelConfig,
  ImageAttachment,
  GenerationMode,
  ModelProvider,
  ModelProviderPayload,
  ThinkingLevel,
  UiMessageItem,
  UpdateAgentPayload,
} from '../../types';
import { approveToolCall, readTaskPlans, type TaskPlan } from '../../api';
import { useI18n } from '../../i18n';
import { useConversations, usePageVisibility } from '../../hooks';
import { AgentListItem, agentDisplayName } from '../../components/AgentListItem';
import { Button, IconButton, ListItem, PaneHeader, SearchInput, useFeedback } from '../../components/ui';
import { ChatComposer } from './ChatComposer';
import { ChatAgentSelect } from './ChatAgentSelect';
import { MessageAvatar, MessageList, type MessageListHandle } from './MessageList';
import { MessageTimeline, type MessageTimelineItem } from './MessageTimeline';
import { userErrorMessage } from '../../utils/userError';
import { compareNames, sortByName } from '../../utils/sort';

interface ChatPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  activeAgentId: string;
  onAgentChange: (id: string) => void;
  onAgentEdit: (id: string, payload: UpdateAgentPayload) => Promise<Agent>;
  onProviderEdit: (
    id: string,
    payload: Partial<ModelProviderPayload>,
  ) => Promise<ModelProvider>;
  onOpenAgentSettings: (id: string) => void;
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

type PendingImageOptionsSave = {
  requestId: number;
  providerId: string;
  modelId: string;
  options: ComposerImageOptions;
};

type ComposerImageOptions = Pick<ImageModelConfig, 'size' | 'quality' | 'n'>;

const MODEL_AUTO_SAVE_DELAY_MS = 600;
const IMAGE_OPTIONS_AUTO_SAVE_DELAY_MS = 600;
const EMPTY_MESSAGES: UiMessageItem[] = [];

export function ChatPanel({
  agents,
  providers,
  activeAgentId,
  onAgentChange,
  onAgentEdit,
  onProviderEdit,
  onOpenAgentSettings,
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
    imageOnlyConversationTitle: t('conversation.imagePromptTitle'),
    imageAttachmentTitleNote: t('conversation.imageAttachmentTitleNote'),
    runCancelledMessage: t('chat.runCancelled'),
    runInterruptedMessage: t('chat.runInterrupted'),
    onError: toast.error,
  });

  const [thinkingLevel, setThinkingLevel] = useState<ThinkingLevel>('medium');
  const [generationMode, setGenerationMode] = useState<GenerationMode>('chat');
  const [modelTargetAgentId, setModelTargetAgentId] = useState('');
  const [selectedModelValue, setSelectedModelValue] = useState('');
  const [isModelSavePending, setIsModelSavePending] = useState(false);
  const [isSavingModel, setIsSavingModel] = useState(false);
  const [imageOptionsDraft, setImageOptionsDraft] = useState<ComposerImageOptions>({});
  const [isSavingImageOptions, setIsSavingImageOptions] = useState(false);
  const [conversationFilter, setConversationFilter] = useState('');
  const [pinningConversationIds, setPinningConversationIds] = useState<Set<string>>(() => new Set());
  const [activeTimelineMessageIndex, setActiveTimelineMessageIndex] = useState<number | null>(null);
  const [taskPlans, setTaskPlans] = useState<TaskPlan[]>([]);
  const [isTaskPlanOpen, setIsTaskPlanOpen] = useState(false);
  const [isTaskPlanLoading, setIsTaskPlanLoading] = useState(false);
  const isPageVisible = usePageVisibility();

  const conversationSearchRef = useRef<HTMLInputElement>(null);
  const messageListRef = useRef<MessageListHandle>(null);
  const modelSaveTimerRef = useRef<number | undefined>(undefined);
  const modelSaveRequestRef = useRef(0);
  const pendingModelSaveRef = useRef<PendingModelSave | null>(null);
  const imageOptionsDraftRef = useRef<ComposerImageOptions>({});
  const imageOptionsSaveTimerRef = useRef<number | undefined>(undefined);
  const imageOptionsSaveRequestRef = useRef(0);
  const pendingImageOptionsSaveRef = useRef<PendingImageOptionsSave | null>(null);
  const imageOptionsSaveInFlightRef = useRef(false);
  const autoModelSelectionKeyRef = useRef<string | null>(null);
  const onAgentEditRef = useRef(onAgentEdit);
  const taskPlanRequestRef = useRef(0);

  onAgentEditRef.current = onAgentEdit;
  imageOptionsDraftRef.current = imageOptionsDraft;

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
  const pendingModelSelection = useMemo(() => {
    if (!selectedModelValue) return null;
    try {
      const parsed: unknown = JSON.parse(selectedModelValue);
      if (!Array.isArray(parsed)) return null;
      const [providerId, modelId] = parsed;
      if (typeof providerId !== 'string' || typeof modelId !== 'string') return null;
      return { providerId, modelId };
    } catch {
      return null;
    }
  }, [selectedModelValue]);
  const targetProviderId = pendingModelSelection?.providerId
    ?? modelTargetAgent?.model_provider_id;
  const targetModelId = pendingModelSelection?.modelId ?? modelTargetAgent?.model_id;
  const activeProvider = providers.find(
    (provider) => provider.id === targetProviderId,
  );
  const activeModelConfig = activeProvider?.model_configs.find(
    (config) => config.model_id === targetModelId,
  );
  const supportsImages = activeModelConfig?.multimodal === true;
  const supportsImageGeneration = activeProvider?.provider_type === 'openai_images';
  const supportsThinking = activeModelConfig?.thinking === true;
  const effectiveGenerationMode: GenerationMode = supportsImageGeneration ? 'image' : generationMode;
  const imageOptionSize = activeModelConfig?.image_options?.size;
  const imageOptionQuality = activeModelConfig?.image_options?.quality;
  const imageOptionCount = activeModelConfig?.image_options?.n;
  useEffect(() => {
    const pending = pendingImageOptionsSaveRef.current;
    if (
      pending
      && pending.providerId === activeProvider?.id
      && pending.modelId === targetModelId
    ) return;
    if (pending) {
      pendingImageOptionsSaveRef.current = null;
      imageOptionsSaveRequestRef.current += 1;
      if (imageOptionsSaveTimerRef.current !== undefined) {
        window.clearTimeout(imageOptionsSaveTimerRef.current);
        imageOptionsSaveTimerRef.current = undefined;
      }
    }
    const next = {
      size: imageOptionSize,
      quality: imageOptionQuality,
      n: imageOptionCount,
    };
    imageOptionsDraftRef.current = next;
    setImageOptionsDraft({
      ...next,
    });
    setIsSavingImageOptions(false);
  }, [
    imageOptionCount,
    imageOptionQuality,
    imageOptionSize,
    activeProvider?.id,
    targetModelId,
  ]);
  useEffect(() => () => {
    if (imageOptionsSaveTimerRef.current !== undefined) {
      window.clearTimeout(imageOptionsSaveTimerRef.current);
    }
  }, []);
  useEffect(() => {
    if (supportsImageGeneration) {
      setGenerationMode('image');
    } else {
      setGenerationMode('chat');
    }
  }, [supportsImageGeneration]);
  const modelOptions = useMemo<ModelOption[]>(() => sortByName(providers).flatMap((provider) =>
    [...provider.models].sort(compareNames).map((model) => ({
      value: JSON.stringify([provider.id, model]),
      providerId: provider.id,
      modelId: model,
      label: `${provider.name} | ${model}`,
    })),
  ), [providers]);
  const soleModelOption = modelOptions.length === 1 ? modelOptions[0] : undefined;
  const activeModelValue = modelTargetAgent
    ? JSON.stringify([modelTargetAgent.model_provider_id, modelTargetAgent.model_id])
    : '';
  const hasProvider = Boolean(targetProviderId);
  const hasModel = Boolean(targetModelId);
  const isComposerDisabled = !activeAgent || !hasProvider || !hasModel;
  const composerPlaceholder = !activeAgent
    ? t('chat.placeholderDisabled')
    : !hasProvider
      ? t('chat.placeholderNoProvider')
      : !hasModel
        ? t('chat.placeholderNoModel')
        : effectiveGenerationMode === 'image'
          ? t('chat.imagePromptPlaceholder')
          : t('chat.placeholder');
  const composerInputLabel = effectiveGenerationMode === 'image'
    ? t('chat.imagePromptLabel')
    : t('chat.messageLabel');
  const visibleMessages = activeAgent ? messages : EMPTY_MESSAGES;
  const activeConversation = activeAgent
    ? conversations.find((conversation) => conversation.id === activeConversationId)
    : undefined;

  const conversationActionError = useCallback((cause: unknown) => {
    return userErrorMessage(cause, {
      summary: t('error.conversationActionFailed'),
      recovery: t('error.retryConnection'),
    });
  }, [t]);

  const refreshTaskPlan = useCallback(async (silent = false) => {
    const requestId = ++taskPlanRequestRef.current;
    if (!activeConversationId) {
      setTaskPlans([]);
      setIsTaskPlanLoading(false);
      return;
    }
    if (!silent) setIsTaskPlanLoading(true);
    try {
      const plans = await readTaskPlans(activeConversationId);
      if (requestId === taskPlanRequestRef.current) setTaskPlans(plans);
    } catch (error) {
      if (!silent && requestId === taskPlanRequestRef.current) {
        toast.error(userErrorMessage(error, {
          summary: t('chat.taskPlanLoading'),
          recovery: t('error.retryConnection'),
        }));
      }
    } finally {
      if (requestId === taskPlanRequestRef.current) setIsTaskPlanLoading(false);
    }
  }, [activeConversationId, t, toast]);

  useEffect(() => {
    setTaskPlans([]);
    setIsTaskPlanOpen(false);
    void refreshTaskPlan();
  }, [activeConversationId, refreshTaskPlan]);

  useEffect(() => {
    if (!activeConversationId || !isPageVisible || !isSending) return;
    const refresh = () => void refreshTaskPlan(true);
    const timer = window.setInterval(refresh, 1200);
    return () => window.clearInterval(timer);
  }, [activeConversationId, isPageVisible, isSending, refreshTaskPlan]);

  useEffect(() => {
    if (!activeConversationId || messages.length === 0) return;
    void refreshTaskPlan(true);
  }, [activeConversationId, messages.length, refreshTaskPlan]);

  const filteredConversations = useMemo(() => {
    const query = conversationFilter.trim().toLowerCase();
    return conversations.filter((conversation) =>
      conversation.title.toLowerCase().includes(query),
    );
  }, [conversations, conversationFilter]);
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
  const timelineItemsRef = useRef<MessageTimelineItem[]>([]);
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
    // Keep the previous array identity when nothing changed so MessageTimeline
    // does not re-render on every stream flush.
    const previous = timelineItemsRef.current;
    const unchanged = previous.length === result.length && previous.every((item, index) => {
      const next = result[index];
      return item.messageIndex === next.messageIndex
        && item.dateLabel === next.dateLabel
        && item.timeLabel === next.timeLabel
        && item.preview === next.preview;
    });
    if (unchanged) return previous;
    timelineItemsRef.current = result;
    return result;
  }, [dateFormatters, visibleMessages]);
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
  }, [activeModelValue, conversationActionError, modelTargetAgentId, onAgentEdit, toast]);

  useEffect(() => {
    // Keep thinking state in sync without resetting a model selection that is
    // still waiting for auto-save.
    setThinkingLevel(supportsThinking ? 'medium' : 'off');
  }, [supportsThinking]);

  useEffect(() => {
    if (
      !soleModelOption
      || !modelTargetAgent
      || isSavingModel
      || isSavingImageOptions
      || isSending
      || activeModelValue === soleModelOption.value
      || selectedModelValue === soleModelOption.value
    ) return;

    const selectionKey = `${modelTargetAgent.id}:${soleModelOption.value}`;
    if (autoModelSelectionKeyRef.current === selectionKey) return;
    autoModelSelectionKeyRef.current = selectionKey;

    const requestId = modelSaveRequestRef.current + 1;
    modelSaveRequestRef.current = requestId;
    pendingModelSaveRef.current = null;
    setSelectedModelValue(soleModelOption.value);
    setIsModelSavePending(true);
    setIsSavingModel(true);
    void onAgentEdit(modelTargetAgent.id, {
      model_provider_id: soleModelOption.providerId,
      model_id: soleModelOption.modelId,
    }).catch((cause) => {
      if (modelSaveRequestRef.current === requestId) {
        setSelectedModelValue(activeModelValue);
        setIsModelSavePending(false);
      }
      toast.error(conversationActionError(cause));
    }).finally(() => {
      if (modelSaveRequestRef.current === requestId) {
        setIsSavingModel(false);
      }
    });
  }, [
    activeModelValue,
    conversationActionError,
    isSavingImageOptions,
    isSavingModel,
    isSending,
    modelTargetAgent,
    modelTargetAgent?.id,
    onAgentEdit,
    selectedModelValue,
    soleModelOption,
    soleModelOption?.value,
    toast,
  ]);

  useEffect(() => () => {
    const pending = pendingModelSaveRef.current;
    pendingModelSaveRef.current = null;
    modelSaveRequestRef.current += 1;
    if (modelSaveTimerRef.current !== undefined) {
      window.clearTimeout(modelSaveTimerRef.current);
    }
    if (pending) {
      void onAgentEditRef.current(pending.agentId, {
        model_provider_id: pending.selection.providerId,
        model_id: pending.selection.modelId,
      }).catch(() => undefined);
    }
  }, []);

  useEffect(() => {
    detachActiveRun();
    clearChat();
    setConversationFilter('');
    if (!activeAgentId) return;
    void loadConversationsForAgent(activeAgentId);
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
  }, [activeAgentId, activeConversationId, conversationActionError, loadMessages, resumeActiveRun, toast]);

  function handleAgentSelect(agentId: string) {
    onAgentChange(agentId);
  }

  function handleBackToAgents() {
    detachActiveRun();
    clearChat();
    setConversationFilter('');
    onAgentChange('');
  }

  const handleConversationSelect = useCallback((conversationId: string) => {
    if (isSending) detachActiveRun();
    if (conversationId !== activeConversationId) selectConversation(conversationId);
    onOpenMain();
  }, [activeConversationId, detachActiveRun, isSending, onOpenMain, selectConversation]);

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
      !supportsImageGeneration,
      attachments,
      supportsThinking ? thinkingLevel : 'off',
      effectiveGenerationMode,
      messageId,
    );
  }, [
    activeAgent,
    effectiveGenerationMode,
    isSending,
    send,
    supportsImageGeneration,
    supportsThinking,
    thinkingLevel,
  ]);

  async function handleModelChange(value: string) {
    if (!modelTargetAgent || isSavingModel || isSavingImageOptions || isSending) return;
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

  function handleImageOptionsChange(patch: Partial<ComposerImageOptions>) {
    if (
      !activeProvider
      || !activeModelConfig
      || !supportsImageGeneration
      || isSending
    ) return;
    const next = { ...imageOptionsDraftRef.current, ...patch };
    imageOptionsDraftRef.current = next;
    setImageOptionsDraft(next);
    setIsSavingImageOptions(true);
    const requestId = imageOptionsSaveRequestRef.current + 1;
    imageOptionsSaveRequestRef.current = requestId;
    pendingImageOptionsSaveRef.current = {
      requestId,
      providerId: activeProvider.id,
      modelId: activeModelConfig.model_id,
      options: next,
    };
    if (imageOptionsSaveTimerRef.current !== undefined) {
      window.clearTimeout(imageOptionsSaveTimerRef.current);
    }
    imageOptionsSaveTimerRef.current = window.setTimeout(() => {
      imageOptionsSaveTimerRef.current = undefined;
      void flushPendingImageOptionsSave();
    }, IMAGE_OPTIONS_AUTO_SAVE_DELAY_MS);
  }

  async function flushPendingImageOptionsSave() {
    if (imageOptionsSaveInFlightRef.current) return;
    const pending = pendingImageOptionsSaveRef.current;
    if (!pending) return;
    pendingImageOptionsSaveRef.current = null;
    imageOptionsSaveInFlightRef.current = true;
    try {
      const provider = providers.find((item) => item.id === pending.providerId);
      const modelConfig = provider?.model_configs.find((config) => config.model_id === pending.modelId);
      if (!provider || !modelConfig) return;
      await onProviderEdit(provider.id, {
        model_configs: provider.model_configs.map((config) => (
          config.model_id === pending.modelId
            ? {
              ...config,
              image_options: {
                ...config.image_options,
                size: pending.options.size,
                quality: pending.options.quality,
                n: pending.options.n,
              },
            }
            : config
        )),
      });
    } catch (cause) {
      if (pending.requestId === imageOptionsSaveRequestRef.current) {
        const restored = {
          size: imageOptionSize,
          quality: imageOptionQuality,
          n: imageOptionCount,
        };
        imageOptionsDraftRef.current = restored;
        setImageOptionsDraft(restored);
        toast.error(userErrorMessage(cause, {
          summary: t('chat.imageOptionsSaveFailed'),
          recovery: t('error.retryConnection'),
        }));
      }
    } finally {
      imageOptionsSaveInFlightRef.current = false;
      if (pendingImageOptionsSaveRef.current) {
        imageOptionsSaveTimerRef.current = window.setTimeout(() => {
          imageOptionsSaveTimerRef.current = undefined;
          void flushPendingImageOptionsSave();
        }, 0);
      } else if (pending.requestId === imageOptionsSaveRequestRef.current) {
        setIsSavingImageOptions(false);
      }
    }
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

  const handleNewConversation = useCallback(() => {
    if (!activeAgent) return;
    const isCurrentConversationEmpty = Boolean(
      activeConversation && activeConversation.message_count === 0,
    );
    if (isSending) detachActiveRun();
    if (!isSending && isCurrentConversationEmpty) {
      onOpenMain();
      return;
    }
    void createNewConversation(activeAgent.id)
      .then(() => onOpenMain())
      .catch((cause) => {
        toast.error(conversationActionError(cause));
      });
  }, [activeAgent, activeConversation, conversationActionError, createNewConversation, detachActiveRun, isSending, onOpenMain, toast]);

  const handleDeleteConversation = useCallback(async (conversationId: string) => {
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
  }, [
    activeAgent,
    activeConversationId,
    cancelActiveSend,
    confirm,
    conversationActionError,
    isSending,
    removeConversation,
    t,
    toast,
  ]);

  const handlePinConversation = useCallback(async (conversation: Conversation) => {
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
  }, [conversationActionError, setConversationPinned, t, toast]);

  const handleSendMessage = useCallback((
    content: string,
    attachments: ImageAttachment[],
    imageMode: GenerationMode,
  ) => {
    if (
      !activeAgent
      || isSending
      || isModelSavePending
      || isSavingModel
      || isSavingImageOptions
    ) return;
    void send(
      content,
      activeAgent.id,
      !supportsImageGeneration,
      attachments,
      supportsThinking ? thinkingLevel : 'off',
      supportsImageGeneration ? 'image' : imageMode,
    );
  }, [
    activeAgent,
    isModelSavePending,
    isSavingImageOptions,
    isSavingModel,
    isSending,
    send,
    supportsImageGeneration,
    supportsThinking,
    thinkingLevel,
  ]);

  const handleUnsupportedImage = useCallback(() => {
    toast.warning(t('chat.imageUnsupportedModel'));
  }, [t, toast]);

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
      if (commandKey && event.key.toLowerCase() === 'n' && activeAgent) {
        event.preventDefault();
        handleNewConversation();
        return;
      }
    };
    window.addEventListener('keydown', handleShortcut);
    return () => window.removeEventListener('keydown', handleShortcut);
  }, [activeAgent, activeAgent?.id, activeConversation?.id, activeConversation?.message_count, handleNewConversation, isSending]);

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
                <IconButton variant="ghost" type="button" onClick={handleNewConversation} label={t('conversation.create')}>
                  <Plus size={17} />
                </IconButton>
              )}
            />
            <SearchInput ref={conversationSearchRef} value={conversationFilter} onChange={setConversationFilter} placeholder={t('conversation.search')} label={t('conversation.search')} />
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
                  onSelect={handleConversationSelect}
                  onTogglePin={handlePinConversation}
                  onDelete={handleDeleteConversation}
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
                disabled={
                  modelOptions.length === 0
                  || isSavingModel
                  || isSavingImageOptions
                  || isSending
                }
                title={t('chat.selectModel')}
                onChange={(event) => void handleModelChange(event.target.value)}
              >
                <option value="" disabled>{t('chat.selectModel')}</option>
                {modelOptions.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
              <IconButton
                className="chat-agent-settings-button"
                variant="ghost"
                type="button"
                label={t('chat.configureAgent')}
                disabled={!modelTargetAgent || isSavingModel}
                onClick={() => {
                  if (modelTargetAgent) onOpenAgentSettings(modelTargetAgent.id);
                }}
              >
                <Settings2 size={16} />
              </IconButton>
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
          supportsImages={supportsImages || supportsImageGeneration}
          supportsImageMasks={supportsImageGeneration}
          runPhase={runPhase}
          retryStatus={retryStatus}
          timelineMessageIndexes={timelineMessageIndexes}
          onLoadOlder={handleLoadOlderMessages}
          onRestart={handleRestartMessage}
          onApproveToolCall={handleApproveToolCall}
          onActiveTimelineMessageChange={setActiveTimelineMessageIndex}
          formatTimelineTime={formatTimelineTime}
        />

        {timelineItems.length > 1 ? (
          <MessageTimeline
            items={timelineItems}
            activeMessageIndex={activeTimelineMessageIndex}
            label={t('chat.timeline')}
            onSelect={handleTimelineSelect}
          />
        ) : null}

        <div className={`chat-composer-stack${activeConversationId && !supportsImageGeneration ? ' has-task-plan' : ''}`}>
          {activeConversationId && !supportsImageGeneration ? (
            <TaskPlanPanel
              plans={taskPlans}
              isLoading={isTaskPlanLoading}
              isOpen={isTaskPlanOpen}
              onToggle={() => setIsTaskPlanOpen((current) => !current)}
              t={t}
            />
          ) : null}
          <ChatComposer
            disabled={isComposerDisabled}
            placeholder={composerPlaceholder}
            inputLabel={composerInputLabel}
            draftKey={`${activeAgent?.id ?? 'none'}:${activeConversationId || 'new'}`}
            isSending={isSending}
            isSavingModel={isSavingModel}
            isModelSavePending={isModelSavePending || isSavingImageOptions}
            supportsImages={supportsImages}
            supportsImageGeneration={supportsImageGeneration}
            generationMode={effectiveGenerationMode}
            imageOptions={imageOptionsDraft}
            imageSizeControlKey={targetModelId ?? ''}
            isSavingImageOptions={isSavingImageOptions}
            onGenerationModeChange={setGenerationMode}
            onImageOptionsChange={handleImageOptionsChange}
            supportsThinking={supportsThinking}
            thinkingLevel={thinkingLevel}
            onThinkingLevelChange={setThinkingLevel}
            usage={conversationUsage}
            contextSize={activeModelConfig?.context_size ?? 128000}
            onUnsupportedImage={handleUnsupportedImage}
            onSend={handleSendMessage}
            onStop={cancelActiveSend}
          />
        </div>
      </section>
    </>
  );
}

function TaskPlanPanel({
  plans,
  isLoading,
  isOpen,
  onToggle,
  t,
}: {
  plans: TaskPlan[];
  isLoading: boolean;
  isOpen: boolean;
  onToggle: () => void;
  t: (path: string, params?: Record<string, string | number>) => string;
}) {
  const currentPlan = plans[0];
  const completed = currentPlan?.plan.filter((item) => item.status === 'completed').length ?? 0;
  const total = currentPlan?.plan.length ?? 0;

  return (
    <section className={`task-plan-panel${isOpen ? ' is-open' : ''}`} aria-label={t('tools.taskPlanning.name')}>
      <button
        type="button"
        className="task-plan-panel__summary"
        aria-expanded={isOpen}
        onClick={onToggle}
      >
        <span className="task-plan-panel__summary-icon" aria-hidden="true"><ListChecks size={16} /></span>
          <span className="task-plan-panel__summary-copy">
            <strong>{t('tools.taskPlanning.name')}</strong>
          {plans.length > 0 ? (
            <span
              className="task-plan-panel__count-badge"
              title={t('chat.taskPlanHistoryCount', { count: plans.length })}
              aria-label={t('chat.taskPlanHistoryCount', { count: plans.length })}
            >
              {plans.length}
            </span>
            ) : null}
          </span>
          {currentPlan ? (
            <span
              className="task-plan-panel__summary-progress"
              aria-label={t('chat.taskPlanCurrentProgress', { completed, total })}
            >
              <span className="task-plan-panel__progress-count">
                {t('chat.taskPlanCurrentProgress', { completed, total })}
              </span>
              <span className="task-plan-panel__progress-track" aria-hidden="true">
                {currentPlan.plan.map((item, index) => (
                  <span
                    key={`${currentPlan.planning_id}-progress-${index}`}
                    className={item.status === 'completed' ? 'is-completed' : undefined}
                  />
                ))}
              </span>
            </span>
          ) : null}
          <ChevronDown className="task-plan-panel__chevron" size={17} aria-hidden="true" />
      </button>
      {isOpen ? (
        <div className="task-plan-panel__details">
          {isLoading ? <p className="task-plan-panel__empty">{t('chat.taskPlanLoading')}</p> : null}
          {!isLoading && plans.length === 0 ? <p className="task-plan-panel__empty">{t('chat.taskPlanEmpty')}</p> : null}
          {!isLoading && plans.length > 0 ? (
            <div className="task-plan-panel__scroll">
              <div className="task-plan-history">
                {plans.map((plan, index) => (
                  <TaskPlanHistoryCard
                    key={plan.planning_id}
                    plan={plan}
                    planNumber={plans.length - index}
                    t={t}
                  />
                ))}
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function TaskPlanHistoryCard({
  plan,
  planNumber,
  t,
}: {
  plan: TaskPlan;
  planNumber: number;
  t: (path: string, params?: Record<string, string | number>) => string;
}) {
  return (
    <article className="task-plan-history__card">
      <div className="task-plan-history__title-row">
        <span className="task-plan-history__number" aria-label={`第 ${planNumber} 个任务`}>{planNumber}</span>
        <h3 className="task-plan-history__title">{plan.explanation || t('tools.taskPlanning.name')}</h3>
        <time className="task-plan-history__time" dateTime={plan.created_at}>
          {formatTaskPlanStartTime(plan.created_at)}
        </time>
      </div>
      <ol className="task-plan-panel__list">
        {plan.plan.map((item, index) => (
          <li key={`${plan.planning_id}-${index}-${item.step}`} className={`task-plan-item task-plan-item--${item.status}`}>
            <span className="task-plan-item__index">{index + 1}</span>
            <span
              className="task-plan-item__step"
              title={item.step}
              aria-label={t(`chat.taskPlanStatus${item.status === 'in_progress' ? 'InProgress' : item.status === 'completed' ? 'Completed' : 'Pending'}`)}
            >
              {item.step}
            </span>
            <span className="task-plan-item__status-icon" aria-hidden="true">
              {item.status === 'completed' ? <CheckCircle2 size={17} /> : item.status === 'in_progress' ? <CircleDot size={17} /> : <Circle size={17} />}
            </span>
          </li>
        ))}
      </ol>
    </article>
  );
}

function formatTaskPlanStartTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;

  const pad = (part: number) => String(part).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function ConversationRowImpl({
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
  onSelect: (conversationId: string) => void;
  onTogglePin: (conversation: Conversation) => void;
  onDelete: (conversationId: string) => void;
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
        onClick={() => onSelect(conversation.id)}
      />
      <IconButton
        className="conversation-action conversation-pin-action"
        variant="ghost"
        type="button"
        active={conversation.is_pinned}
        disabled={isPinning}
        aria-pressed={conversation.is_pinned}
        onClick={() => onTogglePin(conversation)}
        label={pinLabel}
      >
        <Pin size={14} fill={conversation.is_pinned ? 'currentColor' : 'none'} />
      </IconButton>
      <IconButton
        className="conversation-action conversation-delete-action"
        variant="ghost"
        type="button"
        onClick={() => onDelete(conversation.id)}
        label={deleteLabel}
      >
        <Trash2 size={14} />
      </IconButton>
    </div>
  );
}

const ConversationRow = memo(ConversationRowImpl);
