import { useState, useEffect, useRef, useCallback, FormEvent, type KeyboardEvent } from 'react';
import { ArrowLeft, Brain, FileText, IdCard, Plus, SlidersHorizontal, Trash2, Wrench } from 'lucide-react';

import type {
  Agent,
  AgentProfile,
  AgentWorkspace,
  ModelProvider,
  CreateAgentPayload,
  UpdateAgentWorkspacePayload,
} from '../../types';
import { readAgentWorkspace } from '../../api';
import { useBeforeUnloadGuard } from '../../hooks';
import { useI18n } from '../../i18n';
import { defaultAvatarColor } from '../../avatarPresets';
import { AvatarBadge } from '../../components/AvatarPicker';
import { AgentListItem, agentDisplayDescription, agentDisplayName } from '../../components/AgentListItem';
import { PanelShell } from '../../components/layout';
import { AutoSaveIndicator, AvatarPopover, Button, FormField, IconButton, PaneHeader, useFeedback } from '../../components/ui';
import { TerminalToolSettings } from '../settings/TerminalToolSettings';
import { userErrorMessage } from '../../utils/userError';
import { compareNames, sortByName } from '../../utils/sort';

interface AgentsPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  activeAgentId: string;
  isLoadingAgents: boolean;
  onAgentChange: (id: string) => void;
  onAgentAdd: (payload: CreateAgentPayload) => Promise<Agent>;
  onAgentProfileSave: (
    id: string,
    payload: Partial<CreateAgentPayload>,
    workspace: UpdateAgentWorkspacePayload,
  ) => Promise<AgentProfile>;
  onAgentRemove: (id: string) => Promise<void>;
  onOpenProviderSettings: () => void;
  onOpenMain: () => void;
  onBackToList: () => void;
  onOpenIpProxies?: () => void;
}

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  description: '',
  model_provider_id: '',
  model_id: '',
  avatar_icon: 'bot',
  avatar_color: defaultAvatarColor('bot'),
};

const EMPTY_PROVIDER_ID = '00000000-0000-0000-0000-000000000000';

const defaultWorkspaceForm: Pick<AgentWorkspace, 'personality' | 'memory' | 'instructions'> = {
  personality: '',
  memory: '',
  instructions: '',
};

type AgentAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
type AgentConfigSection = 'basic' | 'prompt' | 'memory' | 'tools';
type PendingAgentSave = {
  agentId: string;
  serialized: string;
  payload: CreateAgentPayload;
  workspace: typeof defaultWorkspaceForm;
};

const agentConfigSections: {
  key: AgentConfigSection;
  icon: typeof IdCard;
  requiresAgent?: boolean;
}[] = [
  { key: 'basic', icon: IdCard },
  { key: 'prompt', icon: FileText, requiresAgent: true },
  { key: 'memory', icon: Brain, requiresAgent: true },
  { key: 'tools', icon: Wrench, requiresAgent: true },
];
const AGENT_AUTO_SAVE_DELAY_MS = 600;
const AGENT_AUTO_SAVE_RETRY_DELAY_MS = 1200;
const AGENT_AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;

export function AgentsPanel({
  agents,
  providers,
  activeAgentId,
  isLoadingAgents,
  onAgentChange,
  onAgentAdd,
  onAgentProfileSave,
  onAgentRemove,
  onOpenProviderSettings,
  onOpenMain,
  onBackToList,
  onOpenIpProxies,
}: AgentsPanelProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [workspaceForm, setWorkspaceForm] = useState(defaultWorkspaceForm);
  const [activeSection, setActiveSection] = useState<AgentConfigSection>('basic');
  const [isLoadingWorkspace, setIsLoadingWorkspace] = useState(false);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [autoSaveStatus, setAutoSaveStatus] = useState<AgentAutoSaveStatus>('idle');
  const activeAgentIdRef = useRef(activeAgentId);
  const loadedAgentIdRef = useRef('');
  const loadedWorkspaceAgentIdRef = useRef('');
  const latestSerializedRef = useRef('');
  const persistedSerializedRef = useRef('');
  const pendingSaveRef = useRef<PendingAgentSave | null>(null);
  const saveInFlightRef = useRef(false);
  const savePromiseRef = useRef<Promise<boolean> | null>(null);
  const flushAgentSaveQueueRef = useRef<() => Promise<boolean>>(() => Promise.resolve(true));
  const saveRetryCountRef = useRef(0);
  const saveTimerRef = useRef<number | undefined>(undefined);
  const retryTimerRef = useRef<number | undefined>(undefined);
  const validationMessageRef = useRef('');
  const isMountedRef = useRef(true);
  activeAgentIdRef.current = activeAgentId;

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
      if (saveTimerRef.current !== undefined) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = undefined;
      }
      void flushAgentSaveQueueRef.current();
    };
  }, []);

  useBeforeUnloadGuard(
    agentFormDirty
    || autoSaveStatus === 'pending'
    || autoSaveStatus === 'saving'
    || autoSaveStatus === 'error',
  );

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const activeAgentRef = useRef<Agent | undefined>(activeAgent);
  activeAgentRef.current = activeAgent;
  const selectedAgentProvider = providers.find((provider) => provider.id === agentForm.model_provider_id);
  const visibleSections = agentConfigSections.filter((section) => activeAgent || !section.requiresAgent);
  const displayedAgentName = activeAgent?.is_builtin
    ? agentDisplayName(activeAgent, t)
    : agentForm.name;
  const displayedAgentDescription = activeAgent?.is_builtin
    ? agentDisplayDescription(activeAgent, t)
    : agentForm.description;

  const agentActionError = useCallback((cause: unknown) => {
    return userErrorMessage(cause, {
      summary: t('error.agentActionFailed'),
      recovery: t('error.retryConnection'),
    });
  }, [t]);

  useEffect(() => {
    if (!activeAgent) {
      loadedAgentIdRef.current = '';
      loadedWorkspaceAgentIdRef.current = '';
      setAgentForm(defaultAgentForm);
      setWorkspaceForm(defaultWorkspaceForm);
      setAgentFormDirty(false);
      setAutoSaveStatus('idle');
      setActiveSection('basic');
      return;
    }
    if (loadedAgentIdRef.current === activeAgent.id) return;
    loadedAgentIdRef.current = activeAgent.id;
    loadedWorkspaceAgentIdRef.current = '';
    setAgentForm({
      name: activeAgent.name,
      description: activeAgent.description,
      model_provider_id: normalizeModelProviderId(activeAgent.model_provider_id),
      model_id: activeAgent.model_id,
      avatar_icon: activeAgent.avatar_icon,
      avatar_color: activeAgent.avatar_color,
    });
    setWorkspaceForm(defaultWorkspaceForm);
    setAgentFormDirty(false);
    setAutoSaveStatus('idle');
    setActiveSection('basic');
  }, [activeAgent]);

  useEffect(() => {
    let cancelled = false;
    const workspaceAgent = activeAgentRef.current;
    const workspaceAgentId = workspaceAgent?.id ?? '';
    if (!workspaceAgentId || !workspaceAgent) {
      setWorkspaceForm(defaultWorkspaceForm);
      return;
    }
    setIsLoadingWorkspace(true);
    readAgentWorkspace(workspaceAgentId)
      .then((workspace) => {
        if (cancelled) return;
        const nextWorkspace = {
          personality: workspace.personality,
          memory: workspace.memory,
          instructions: workspace.instructions,
        };
        const initialPayload = agentPayloadFromAgent(workspaceAgent);
        const serialized = serializeAgentSnapshot(initialPayload, nextWorkspace);
        loadedWorkspaceAgentIdRef.current = workspaceAgentId;
        persistedSerializedRef.current = serialized;
        latestSerializedRef.current = serialized;
        pendingSaveRef.current = null;
        saveRetryCountRef.current = 0;
        setWorkspaceForm(nextWorkspace);
        setAgentFormDirty(false);
        setAutoSaveStatus('idle');
      })
      .catch((cause) => toast.error(agentActionError(cause)))
      .finally(() => {
        if (!cancelled) setIsLoadingWorkspace(false);
      });
    return () => {
      cancelled = true;
    };
  }, [activeAgent?.id, agentActionError, toast]);

  useEffect(() => {
    if (
      !activeAgent
      || !agentFormDirty
      || isLoadingWorkspace
      || loadedWorkspaceAgentIdRef.current !== activeAgent.id
    ) {
      return;
    }

    const payload = agentPayloadFromForm(activeAgent, agentForm);
    if (
      (!payload.name && !activeAgent.is_builtin)
      || (!payload.description && !activeAgent.is_builtin)
      || !payload.model_provider_id
      || !payload.model_id
    ) {
      setAutoSaveStatus('pending');
      return;
    }
    if (agents.some((agent) =>
      agent.id !== activeAgent.id
      && agent.avatar_icon === payload.avatar_icon
      && agent.avatar_color.toLowerCase() === payload.avatar_color.toLowerCase()
    )) {
      setAutoSaveStatus('error');
      if (validationMessageRef.current !== 'avatarDuplicate') {
        validationMessageRef.current = 'avatarDuplicate';
        toast.warning(t('error.avatarDuplicate'));
      }
      return;
    }
    validationMessageRef.current = '';

    const serialized = serializeAgentSnapshot(payload, workspaceForm);
    if (latestSerializedRef.current !== serialized) {
      saveRetryCountRef.current = 0;
    }
    latestSerializedRef.current = serialized;
    if (persistedSerializedRef.current === serialized) {
      pendingSaveRef.current = null;
      setAgentFormDirty(false);
      setAutoSaveStatus('saved');
      return;
    }

    pendingSaveRef.current = {
      agentId: activeAgent.id,
      serialized,
      payload,
      workspace: { ...workspaceForm },
    };
    setAutoSaveStatus('pending');
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
    }
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = undefined;
      void flushAgentSaveQueueRef.current();
    }, AGENT_AUTO_SAVE_DELAY_MS);
  }, [activeAgent, agentForm, agentFormDirty, agents, isLoadingWorkspace, t, toast, workspaceForm]);

  async function flushAgentSaveQueue(): Promise<boolean> {
    if (savePromiseRef.current) return savePromiseRef.current;
    if (!pendingSaveRef.current) return true;
    const pending = pendingSaveRef.current;
    pendingSaveRef.current = null;
    const operation = (async () => {
      saveInFlightRef.current = true;
      if (isMountedRef.current && activeAgentIdRef.current === pending.agentId) {
        setAutoSaveStatus('saving');
      }

      let retryDelay = 0;
      let saved = false;
      try {
        await onAgentProfileSave(pending.agentId, pending.payload, pending.workspace);
        saved = true;
        persistedSerializedRef.current = pending.serialized;
        saveRetryCountRef.current = 0;
        if (
          isMountedRef.current
          && activeAgentIdRef.current === pending.agentId
          && latestSerializedRef.current === pending.serialized
        ) {
          const queuedSave = pendingSaveRef.current as PendingAgentSave | null;
          if (queuedSave?.serialized === pending.serialized) {
            pendingSaveRef.current = null;
          }
          if (saveTimerRef.current !== undefined) {
            window.clearTimeout(saveTimerRef.current);
            saveTimerRef.current = undefined;
          }
          setAgentFormDirty(false);
          setAutoSaveStatus('saved');
        }
      } catch (cause) {
        const attempt = saveRetryCountRef.current + 1;
        saveRetryCountRef.current = attempt;
        if (latestSerializedRef.current === pending.serialized) {
          pendingSaveRef.current = pending;
          retryDelay = Math.min(
            AGENT_AUTO_SAVE_MAX_RETRY_DELAY_MS,
            AGENT_AUTO_SAVE_RETRY_DELAY_MS * (2 ** Math.min(attempt - 1, 5)),
          );
        }
        if (isMountedRef.current && activeAgentIdRef.current === pending.agentId) {
          setAutoSaveStatus('error');
          if (attempt === 1) {
            toast.error(agentActionError(cause));
          }
        }
      } finally {
        saveInFlightRef.current = false;
        if (pendingSaveRef.current) {
          if (retryTimerRef.current !== undefined) {
            window.clearTimeout(retryTimerRef.current);
          }
          retryTimerRef.current = window.setTimeout(() => {
            retryTimerRef.current = undefined;
            void flushAgentSaveQueueRef.current();
          }, retryDelay);
        }
      }
      return saved;
    })();
    savePromiseRef.current = operation;
    try {
      return await operation;
    } finally {
      if (savePromiseRef.current === operation) savePromiseRef.current = null;
    }
  }

  flushAgentSaveQueueRef.current = flushAgentSaveQueue;

  async function flushAgentSaveNow(): Promise<boolean> {
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
    }
    if (retryTimerRef.current !== undefined) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = undefined;
    }
    let saved = await flushAgentSaveQueue();
    while (saved && pendingSaveRef.current) {
      if (retryTimerRef.current !== undefined) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = undefined;
      }
      saved = await flushAgentSaveQueue();
    }
    return saved;
  }

  async function switchAgent(id: string) {
    if (!await flushAgentSaveNow()) return;
    onAgentChange(id);
    onOpenMain();
  }

  async function startCreatingAgent() {
    if (!await flushAgentSaveNow()) return;
    onAgentChange('');
    setAgentFormDirty(false);
    onOpenMain();
  }

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (activeAgent) {
      await flushAgentSaveNow();
      return;
    }
    const payload = {
      name: agentForm.name.trim(),
      description: agentForm.description.trim(),
      model_provider_id: agentForm.model_provider_id,
      model_id: agentForm.model_id.trim(),
      avatar_icon: agentForm.avatar_icon,
      avatar_color: agentForm.avatar_color,
    };
    if (!payload.name || !payload.description || !payload.model_provider_id || !payload.model_id) {
      setActiveSection('basic');
      toast.warning(t('error.validation'));
      return;
    }
    if (agents.some((agent) =>
      agent.avatar_icon === payload.avatar_icon
      && agent.avatar_color.toLowerCase() === payload.avatar_color.toLowerCase()
    )) {
      toast.warning(t('error.avatarDuplicate'));
      return;
    }
    setIsSubmittingAgent(true);
    try {
      await onAgentAdd(payload);
      toast.success(t('notification.agentCreated'));
      setAgentFormDirty(false);
    } catch (cause) {
      toast.error(agentActionError(cause));
    } finally {
      setIsSubmittingAgent(false);
    }
  }

  async function handleDelete() {
    if (!activeAgent || activeAgent.is_builtin) return;
    const confirmed = await confirm({
      title: t('deleteConfirm.title'),
      message: t('deleteConfirm.message', { name: agentDisplayName(activeAgent, t) }),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;
    pendingSaveRef.current = null;
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
    }
    try {
      await onAgentRemove(activeAgent.id);
      setAgentFormDirty(false);
      toast.success(t('notification.agentDeleted'));
    } catch (cause) {
      toast.error(agentActionError(cause));
    }
  }

  return (
    <PanelShell
      className="agents-panel"
      sidebar={(
        <div className="context-pane agents-panel-sidebar">
          <PaneHeader
            className="mobile-pane-header"
            title={t('nav.agents')}
            actions={(
              <IconButton
                variant="ghost"
                type="button"
                onClick={() => void startCreatingAgent()}
                label={t('agentConfig.create')}
              >
                <Plus size={18} />
              </IconButton>
            )}
          />
          <div className="list-stack">
            {isLoadingAgents && agents.length === 0 ? <p className="muted">{t('chat.loading')}</p> : null}
            {!isLoadingAgents && agents.length === 0 ? (
              <div className="list-empty-action">
                <p className="muted centered">{t('agentConfig.noAgents')}</p>
                <Button variant="primary" type="button" onClick={() => void startCreatingAgent()} icon={<Plus size={16} />}>
                  {t('agentConfig.create')}
                </Button>
              </div>
            ) : null}
            {agents.map((agent) => (
              <AgentListItem
                key={agent.id}
                agent={agent}
                active={agent.id === activeAgentId}
                onClick={() => void switchAgent(agent.id)}
              />
            ))}
          </div>
        </div>
      )}
      main={(
        <div className="main-pane config-main">
          <PaneHeader
            className="config-page-header"
            leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
            eyebrow={t('agentConfig.title')}
            title={activeAgent ? agentDisplayName(activeAgent, t) : t('agentConfig.newTitle')}
            description={activeAgent ? agentDisplayDescription(activeAgent, t) : t('agentConfig.selectAgent')}
            actions={(
              <>
                {activeAgent ? <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={() => void flushAgentSaveNow()} /> : null}
                {activeAgent?.is_builtin ? <span className="model-note-badge">{t('agentConfig.coreAgentModelHint')}</span> : null}
                {!activeAgent ? <span className="model-note-badge">{t('agentConfig.workspaceCreateHint')}</span> : null}
                {activeAgent && !activeAgent.is_builtin ? (
                  <IconButton variant="danger" type="button" onClick={handleDelete} label={t('agentConfig.delete')}>
                    <Trash2 size={17} />
                  </IconButton>
                ) : null}
              </>
            )}
          />

          <AgentConfigTabs
            sections={visibleSections}
            activeSection={activeSection}
            label={t('agentConfig.sectionsLabel')}
            getLabel={(section) => t(`agentConfig.section.${section}`)}
            onChange={setActiveSection}
          />

          <form className="entity-form agent-config-form" onSubmit={handleSubmit} noValidate>
            {activeSection === 'basic' ? (
              <section
                id="agent-config-panel-basic"
                className="agent-section-panel agent-basic-section"
                role="tabpanel"
                aria-labelledby="agent-config-tab-basic"
              >
                <div className="form-card">
                  <div className="agent-basics-layout">
                    <div className="agent-avatar-field">
                      <FormField label={t('agentConfig.avatar')}>
                        <AvatarPopover
                          className="agent-avatar-popover"
                          icon={agentForm.avatar_icon}
                          color={agentForm.avatar_color}
                          title={t('agentConfig.avatarPreview')}
                          pickerTitle={t('agentConfig.avatarChange')}
                          onChange={(next) => {
                            setAgentForm({ ...agentForm, avatar_icon: next.icon, avatar_color: next.color });
                            setAgentFormDirty(true);
                          }}
                          trigger={(open) => (
                            <button
                              type="button"
                              className="agent-avatar-trigger"
                              aria-expanded={open}
                              aria-label={t('agentConfig.avatarChange')}
                            >
                              <AvatarBadge
                                icon={agentForm.avatar_icon}
                                color={agentForm.avatar_color}
                                size={48}
                                className="agent-avatar-preview"
                              />
                              <span className="agent-avatar-change">{t('agentConfig.avatarChange')}</span>
                            </button>
                          )}
                        />
                      </FormField>
                    </div>

                    <div className="agent-config-details agent-basic-fields">
                      <FormField label={t('agentConfig.name')} htmlFor="agent-name" required>
                        <input
                          id="agent-name"
                          value={displayedAgentName}
                          onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                          placeholder={t('agentConfig.placeholder_name')}
                          disabled={isSubmittingAgent || activeAgent?.is_builtin}
                          required
                        />
                      </FormField>
                      <FormField label={t('agentConfig.description')} htmlFor="agent-description" required>
                        <textarea
                          id="agent-description"
                          value={displayedAgentDescription}
                          onChange={(e) => { setAgentForm({ ...agentForm, description: e.target.value }); setAgentFormDirty(true); }}
                          placeholder={t('agentConfig.placeholder_description')}
                          disabled={isSubmittingAgent || activeAgent?.is_builtin}
                          rows={7}
                          required
                        />
                      </FormField>
                    </div>
                  </div>
                </div>

                <div className="form-card agent-model-card">
                  <PaneHeader compact title={t('agentConfig.section.model')} description={t('agentConfig.modelHint')} />
                  {providers.length === 0 ? (
                    <div className="agent-inline-notice">
                      <p>{t('agentConfig.noProviders')}</p>
                      <Button variant="primary" type="button" onClick={onOpenProviderSettings} icon={<SlidersHorizontal size={16} />}>
                        {t('agentConfig.configureProvider')}
                      </Button>
                    </div>
                  ) : (
                    <div className="agent-model-row">
                      <FormField label={t('agentConfig.provider')} htmlFor="agent-provider" required>
                        <select
                          id="agent-provider"
                          value={agentForm.model_provider_id}
                          onChange={(e) => {
                            const provider = providers.find((p) => p.id === e.target.value);
                            setAgentForm({ ...agentForm, model_provider_id: e.target.value, model_id: provider?.models[0] ?? '' });
                            setAgentFormDirty(true);
                          }}
                          disabled={isSubmittingAgent}
                          required
                        >
                          <option value="">{t('agentConfig.placeholder_provider')}</option>
                          {sortByName(providers).map((provider) => <option key={provider.id} value={provider.id}>{provider.name}</option>)}
                        </select>
                      </FormField>
                      <FormField
                        label={t('agentConfig.model')}
                        htmlFor="agent-model"
                        required
                        action={(
                          <IconButton variant="ghost" type="button" onClick={onOpenProviderSettings} label={t('agentConfig.openProviderSettings')}>
                            <SlidersHorizontal size={16} />
                          </IconButton>
                        )}
                      >
                        <select
                          id="agent-model"
                          value={agentForm.model_id}
                          onChange={(e) => { setAgentForm({ ...agentForm, model_id: e.target.value }); setAgentFormDirty(true); }}
                          disabled={isSubmittingAgent || !selectedAgentProvider}
                          required
                        >
                          <option value="">{t('agentConfig.placeholder_model')}</option>
                          {[...(selectedAgentProvider?.models ?? [])].sort(compareNames).map((model) => <option key={model} value={model}>{model}</option>)}
                        </select>
                      </FormField>
                    </div>
                  )}
                </div>
              </section>
            ) : null}

            {activeAgent && activeSection === 'prompt' ? (
              <section
                id="agent-config-panel-prompt"
                className="form-card agent-section-panel"
                role="tabpanel"
                aria-labelledby="agent-config-tab-prompt"
              >
                <PaneHeader compact title={t('agentConfig.section.prompt')} description={isLoadingWorkspace ? t('chat.loading') : t('agentConfig.promptHint')} />
                <div className="form-grid two-column agent-prompt-grid">
                  <FormField label={t('agentConfig.personality')} htmlFor="agent-personality">
                    <textarea
                      id="agent-personality"
                      value={workspaceForm.personality}
                      onChange={(e) => { setWorkspaceForm({ ...workspaceForm, personality: e.target.value }); setAgentFormDirty(true); }}
                      rows={12}
                      disabled={isSubmittingAgent || isLoadingWorkspace}
                    />
                  </FormField>
                  <FormField label={t('agentConfig.instructions')} htmlFor="agent-instructions">
                    <textarea
                      id="agent-instructions"
                      value={workspaceForm.instructions}
                      onChange={(e) => { setWorkspaceForm({ ...workspaceForm, instructions: e.target.value }); setAgentFormDirty(true); }}
                      rows={12}
                      disabled={isSubmittingAgent || isLoadingWorkspace}
                    />
                  </FormField>
                </div>
              </section>
            ) : null}

            {activeAgent && activeSection === 'memory' ? (
              <section
                id="agent-config-panel-memory"
                className="form-card agent-section-panel"
                role="tabpanel"
                aria-labelledby="agent-config-tab-memory"
              >
                <PaneHeader compact title={t('agentConfig.section.memory')} description={isLoadingWorkspace ? t('chat.loading') : t('agentConfig.memoryHint')} />
                <FormField label={t('agentConfig.memory')} htmlFor="agent-memory">
                  <textarea
                    id="agent-memory"
                    value={workspaceForm.memory}
                    onChange={(e) => { setWorkspaceForm({ ...workspaceForm, memory: e.target.value }); setAgentFormDirty(true); }}
                    rows={16}
                    disabled={isSubmittingAgent || isLoadingWorkspace}
                  />
                </FormField>
              </section>
            ) : null}

            {activeAgent && activeSection === 'tools' ? (
              <section
                id="agent-config-panel-tools"
                className="agent-tools-section"
                role="tabpanel"
                aria-labelledby="agent-config-tab-tools"
              >
                <TerminalToolSettings scope="agent" agentId={activeAgent.id} onOpenIpProxies={onOpenIpProxies} />
              </section>
            ) : null}

            {!activeAgent ? (
              <div className="form-actions agent-create-actions">
                <Button variant="primary" type="submit" disabled={isSubmittingAgent} icon={<Plus size={17} />}>
                  {t('agentConfig.create')}
                </Button>
              </div>
            ) : null}
          </form>
        </div>
      )}
    />
  );
}

function AgentConfigTabs({
  sections,
  activeSection,
  label,
  getLabel,
  onChange,
}: {
  sections: typeof agentConfigSections;
  activeSection: AgentConfigSection;
  label: string;
  getLabel: (section: AgentConfigSection) => string;
  onChange: (section: AgentConfigSection) => void;
}) {
  function handleKeyDown(event: KeyboardEvent<HTMLButtonElement>, section: AgentConfigSection) {
    const currentIndex = sections.findIndex((item) => item.key === section);
    let nextIndex: number | null = null;
    if (event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % sections.length;
    if (event.key === 'ArrowLeft') nextIndex = (currentIndex - 1 + sections.length) % sections.length;
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = sections.length - 1;
    if (nextIndex === null) return;

    event.preventDefault();
    const nextSection = sections[nextIndex].key;
    onChange(nextSection);
    event.currentTarget.parentElement
      ?.querySelector<HTMLElement>(`#agent-config-tab-${nextSection}`)
      ?.focus();
  }

  return (
    <div className="agent-config-tabs" role="tablist" aria-label={label}>
      {sections.map(({ key, icon: Icon }) => {
        const active = activeSection === key;
        return (
          <button
            key={key}
            id={`agent-config-tab-${key}`}
            className={`agent-config-tab${active ? ' is-active' : ''}`}
            type="button"
            role="tab"
            tabIndex={active ? 0 : -1}
            aria-selected={active}
            aria-controls={`agent-config-panel-${key}`}
            onClick={() => onChange(key)}
            onKeyDown={(event) => handleKeyDown(event, key)}
          >
            <Icon size={16} aria-hidden="true" />
            <span>{getLabel(key)}</span>
          </button>
        );
      })}
    </div>
  );
}


function agentPayloadFromAgent(agent: Agent): CreateAgentPayload {
  return {
    name: agent.name,
    description: agent.description,
    model_provider_id: normalizeModelProviderId(agent.model_provider_id),
    model_id: agent.model_id,
    avatar_icon: agent.avatar_icon,
    avatar_color: agent.avatar_color,
  };
}

function normalizeModelProviderId(providerId: string): string {
  return providerId === EMPTY_PROVIDER_ID ? '' : providerId;
}

function agentPayloadFromForm(agent: Agent, form: CreateAgentPayload): CreateAgentPayload {
  return {
    name: agent.is_builtin ? agent.name : form.name.trim(),
    description: agent.is_builtin ? agent.description : form.description.trim(),
    model_provider_id: form.model_provider_id,
    model_id: form.model_id.trim(),
    avatar_icon: form.avatar_icon,
    avatar_color: form.avatar_color,
  };
}

function serializeAgentSnapshot(
  payload: CreateAgentPayload,
  workspace: typeof defaultWorkspaceForm,
): string {
  return JSON.stringify({ payload, workspace });
}
