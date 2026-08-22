import { useRef, useEffect, useCallback, useMemo, useState } from 'react';
import {
  ArrowLeft,
  Plus,
  Settings2,
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
import { approveToolCall, readAgentMcpConfig, readTaskPlans } from '../../api';
import type { McpServer, TaskPlan } from '../../toolTypes';
import { useI18n } from '../../i18n';
import { useConversations, usePageVisibility } from '../../hooks';
import { AUTO_SAVE_DELAY_MS } from '../../hooks/useSerializedAutoSave';
import { AgentListItem, agentDisplayName } from '../../components/AgentListItem';
import { Button, IconButton, PaneHeader, SearchInput, useFeedback } from '../../components/ui';
import { ChatComposer, type ChatComposerHandle } from './ChatComposer';
import { ChatAgentSelect } from './ChatAgentSelect';
import { ConversationRow } from './ConversationRow';
import { TaskPlanPanel } from './TaskPlanPanel';
import { getTaskPlanSyncKey } from './taskPlanSync';
import { MessageAvatar, MessageList, type MessageListHandle } from './MessageList';
import { MessageTimeline, type MessageTimelineItem } from './MessageTimeline';
import { materializeImageAttachment } from './imageAttachments';
import { userErrorMessage } from '../../utils/userError';
import { compareNames, sortByName } from '../../utils/sort';
import { createChatDateFormatters, formatChatDateTime, formatChatTimelineTime } from './chatDateFormat';
import { sortAgentsByRecentConversation } from '../../utils/agentSorting';

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
  openConversationId?: string;
  onConversationOpened?: () => void;
  latestConversationUpdatedAtByAgent: Record<string, string>;
  onAgentActivity: (agentId: string) => void;
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
  openConversationId,
  onConversationOpened,
  latestConversationUpdatedAtByAgent,
  onAgentActivity,
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
    unreadConversationIdsByAgent,
    refreshUnreadState,
    markConversationRead,
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
    activeAgentId,
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
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const isPageVisible = usePageVisibility();

  const conversationSearchRef = useRef<HTMLInputElement>(null);
  const messageListRef = useRef<MessageListHandle>(null);
  const composerRef = useRef<ChatComposerHandle>(null);
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
  const sortedAgents = useMemo(
    () => sortAgentsByRecentConversation(agents, latestConversationUpdatedAtByAgent),
    [agents, latestConversationUpdatedAtByAgent],
  );
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
  const taskPlanSyncKey = useMemo(() => getTaskPlanSyncKey(messages), [messages]);
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
    let cancelled = false;
    setMcpServers([]);
    if (!activeAgentId) return undefined;

    void readAgentMcpConfig(activeAgentId)
      .then((settings) => {
        if (!cancelled) setMcpServers(settings.effective);
      })
      .catch(() => {
        if (!cancelled) setMcpServers([]);
      });

    return () => {
      cancelled = true;
    };
  }, [activeAgentId]);

  useEffect(() => {
    if (!activeConversationId || !isPageVisible || !isSending) return;
    const refresh = () => void refreshTaskPlan(true);
    const timer = window.setInterval(refresh, 1200);
    return () => window.clearInterval(timer);
  }, [activeConversationId, isPageVisible, isSending, refreshTaskPlan]);

  useEffect(() => {
    if (!activeConversationId || messages.length === 0) return;
    void refreshTaskPlan(true);
  }, [activeConversationId, messages.length, refreshTaskPlan, taskPlanSyncKey]);

  const filteredConversations = useMemo(() => {
    const query = conversationFilter.trim().toLowerCase();
    return conversations.filter((conversation) =>
      conversation.title.toLowerCase().includes(query),
    );
  }, [conversations, conversationFilter]);
  const dateFormatters = useMemo(() => createChatDateFormatters(locale), [locale]);
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
    if (!isPageVisible) return;
    const agentIds = agents.map((agent) => agent.id);
    void refreshUnreadState(agentIds);
    if (activeAgentId) void refreshRunningConversations(activeAgentId);
    const timer = window.setInterval(() => {
      void refreshUnreadState(agentIds);
      if (activeAgentId) void refreshRunningConversations(activeAgentId);
    }, 3000);
    return () => window.clearInterval(timer);
  }, [activeAgentId, agents, isPageVisible, refreshRunningConversations, refreshUnreadState]);

  useEffect(() => {
    if (!openConversationId || !activeAgentId) return;
    if (!conversations.some((conversation) => conversation.id === openConversationId)) return;
    markConversationRead(activeAgentId, openConversationId);
    selectConversation(openConversationId);
    onConversationOpened?.();
    onOpenMain();
  }, [activeAgentId, conversations, markConversationRead, onConversationOpened, onOpenMain, openConversationId, selectConversation]);

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
    markConversationRead(activeAgentId, conversationId);
    if (conversationId !== activeConversationId) selectConversation(conversationId);
    onOpenMain();
  }, [activeAgentId, activeConversationId, detachActiveRun, isSending, markConversationRead, onOpenMain, selectConversation]);

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
    }, AUTO_SAVE_DELAY_MS);
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
    }, AUTO_SAVE_DELAY_MS);
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
    onAgentActivity(activeAgent.id);
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
  }, [activeAgent, activeConversation, conversationActionError, createNewConversation, detachActiveRun, isSending, onAgentActivity, onOpenMain, toast]);

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
    onAgentActivity(activeAgent.id);
  }, [
    activeAgent,
    isModelSavePending,
    isSavingImageOptions,
    isSavingModel,
    isSending,
    onAgentActivity,
    send,
    supportsImageGeneration,
    supportsThinking,
    thinkingLevel,
  ]);

  const handleUnsupportedImage = useCallback(() => {
    toast.warning(t('chat.imageUnsupportedModel'));
  }, [t, toast]);

  const handleReferenceImage = useCallback(async (attachment: ImageAttachment) => {
    try {
      const materialized = await materializeImageAttachment(attachment);
      composerRef.current?.addImageAttachment(materialized);
    } catch (cause) {
      toast.error(userErrorMessage(cause, {
        summary: t('chat.imageLoadFailed'),
        recovery: t('error.retryConnection'),
      }));
    }
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

  const formatDate = useCallback((value: string) => formatChatDateTime(value, dateFormatters), [dateFormatters]);
  const formatTimelineTime = useCallback((value: string) => formatChatTimelineTime(value, dateFormatters), [dateFormatters]);

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
              {sortedAgents.map((agent) => (
                <AgentListItem
                  key={agent.id}
                  agent={agent}
                  unreadCount={unreadConversationIdsByAgent[agent.id]?.length ?? 0}
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
                  isUnread={unreadConversationIdsByAgent[activeAgent.id]?.includes(conversation.id) ?? false}
                  unreadLabel={t('conversation.unread')}
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
          onEditImage={handleReferenceImage}
          onApproveToolCall={handleApproveToolCall}
          mcpServers={mcpServers}
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
            ref={composerRef}
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

