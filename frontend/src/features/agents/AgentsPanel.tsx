import { useState, useEffect, useRef, FormEvent } from 'react';
import { ArrowLeft, Plus, SlidersHorizontal, Trash2 } from 'lucide-react';

import type { Agent, AgentWorkspace, ModelProvider, CreateAgentPayload } from '../../types';
import { readAgentWorkspace, updateAgentWorkspace } from '../../api';
import { useI18n } from '../../i18n';
import { defaultAvatarColor } from '../../avatarPresets';
import { AvatarBadge } from '../../components/AvatarPicker';
import { AgentListItem, agentDisplayDescription, agentDisplayName } from '../../components/AgentListItem';
import { PanelShell } from '../../components/layout';
import { AutoSaveIndicator, AvatarPopover, Button, FormField, IconButton, PaneHeader, useFeedback } from '../../components/ui';
import { TerminalToolSettings } from '../settings/TerminalToolSettings';

interface AgentsPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  activeAgentId: string;
  isLoadingAgents: boolean;
  onAgentChange: (id: string) => void;
  onAgentAdd: (payload: CreateAgentPayload) => Promise<Agent>;
  onAgentEdit: (id: string, payload: Partial<CreateAgentPayload>) => Promise<Agent>;
  onAgentRemove: (id: string) => Promise<void>;
  onOpenProviderSettings: () => void;
  onOpenMain: () => void;
  onBackToList: () => void;
}

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  description: '',
  model_provider_id: '',
  model_id: '',
  avatar_icon: 'bot',
  avatar_color: defaultAvatarColor('bot'),
};

const defaultWorkspaceForm: Pick<AgentWorkspace, 'personality' | 'memory' | 'instructions'> = {
  personality: '',
  memory: '',
  instructions: '',
};

type WorkspaceField = keyof typeof defaultWorkspaceForm;
type AgentAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
type PendingAgentSave = {
  agentId: string;
  serialized: string;
  payload: CreateAgentPayload;
  workspace: typeof defaultWorkspaceForm;
};

const workspaceTabs: WorkspaceField[] = ['personality', 'memory', 'instructions'];
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
  onAgentEdit,
  onAgentRemove,
  onOpenProviderSettings,
  onOpenMain,
  onBackToList,
}: AgentsPanelProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [workspaceForm, setWorkspaceForm] = useState(defaultWorkspaceForm);
  const [activeWorkspaceField, setActiveWorkspaceField] = useState<WorkspaceField>('personality');
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
  const saveRetryCountRef = useRef(0);
  const saveTimerRef = useRef<number | undefined>(undefined);
  const retryTimerRef = useRef<number | undefined>(undefined);
  const validationMessageRef = useRef('');
  const isMountedRef = useRef(true);
  activeAgentIdRef.current = activeAgentId;

  useEffect(() => () => {
    isMountedRef.current = false;
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
    }
    void flushAgentSaveQueue();
  }, []);

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const selectedAgentProvider = providers.find((provider) => provider.id === agentForm.model_provider_id);
  const activeWorkspaceLabel = t(`agentConfig.${activeWorkspaceField}`);

  useEffect(() => {
    if (!activeAgent) {
      loadedAgentIdRef.current = '';
      loadedWorkspaceAgentIdRef.current = '';
      setAgentForm(defaultAgentForm);
      setWorkspaceForm(defaultWorkspaceForm);
      setAgentFormDirty(false);
      setAutoSaveStatus('idle');
      return;
    }
    if (loadedAgentIdRef.current === activeAgent.id) return;
    loadedAgentIdRef.current = activeAgent.id;
    loadedWorkspaceAgentIdRef.current = '';
    setAgentForm({
      name: agentDisplayName(activeAgent, t),
      description: agentDisplayDescription(activeAgent, t),
      model_provider_id: activeAgent.model_provider_id,
      model_id: activeAgent.model_id,
      avatar_icon: activeAgent.avatar_icon,
      avatar_color: activeAgent.avatar_color,
    });
    setWorkspaceForm(defaultWorkspaceForm);
    setAgentFormDirty(false);
    setAutoSaveStatus('idle');
  }, [activeAgent?.id, t]);

  useEffect(() => {
    let cancelled = false;
    if (!activeAgent) {
      setWorkspaceForm(defaultWorkspaceForm);
      return;
    }
    setIsLoadingWorkspace(true);
    readAgentWorkspace(activeAgent.id)
      .then((workspace) => {
        if (cancelled) return;
        const nextWorkspace = {
          personality: workspace.personality,
          memory: workspace.memory,
          instructions: workspace.instructions,
        };
        const initialPayload = agentPayloadFromAgent(activeAgent);
        const serialized = serializeAgentSnapshot(initialPayload, nextWorkspace);
        loadedWorkspaceAgentIdRef.current = activeAgent.id;
        persistedSerializedRef.current = serialized;
        latestSerializedRef.current = serialized;
        pendingSaveRef.current = null;
        saveRetryCountRef.current = 0;
        setWorkspaceForm(nextWorkspace);
        setAgentFormDirty(false);
        setAutoSaveStatus('idle');
      })
      .catch((cause) => toast.error(cause instanceof Error ? cause.message : t('error.unknown')))
      .finally(() => {
        if (!cancelled) setIsLoadingWorkspace(false);
      });
    return () => {
      cancelled = true;
    };
  }, [activeAgent?.id, t, toast]);

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
      void flushAgentSaveQueue();
    }, AGENT_AUTO_SAVE_DELAY_MS);
  }, [activeAgent, agentForm, agentFormDirty, agents, isLoadingWorkspace, t, toast, workspaceForm]);

  async function flushAgentSaveQueue() {
    if (saveInFlightRef.current || !pendingSaveRef.current) return;
    const pending = pendingSaveRef.current;
    pendingSaveRef.current = null;
    saveInFlightRef.current = true;
    if (isMountedRef.current && activeAgentIdRef.current === pending.agentId) {
      setAutoSaveStatus('saving');
    }

    let retryDelay = 0;
    try {
      await onAgentEdit(pending.agentId, pending.payload);
      await updateAgentWorkspace(pending.agentId, pending.workspace);
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
          toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
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
          void flushAgentSaveQueue();
        }, retryDelay);
      }
    }
  }

  function flushAgentSaveNow() {
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
    }
    void flushAgentSaveQueue();
  }

  function switchAgent(id: string) {
    flushAgentSaveNow();
    onAgentChange(id);
    onOpenMain();
  }

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (activeAgent) {
      flushAgentSaveNow();
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
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
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
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
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
                onClick={() => { flushAgentSaveNow(); onAgentChange(''); setAgentFormDirty(false); onOpenMain(); }}
                label={t('agentConfig.create')}
              >
                <Plus size={17} />
              </IconButton>
            )}
          />
          <div className="list-stack">
            {isLoadingAgents && agents.length === 0 ? <p className="muted">{t('chat.loading')}</p> : null}
            {!isLoadingAgents && agents.length === 0 ? <p className="muted">{t('agentConfig.noAgents')}</p> : null}
            {agents.map((agent) => (
              <AgentListItem
                key={agent.id}
                agent={agent}
                active={agent.id === activeAgentId}
                onClick={() => switchAgent(agent.id)}
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
                {activeAgent ? <AutoSaveIndicator status={autoSaveStatus} t={t} /> : null}
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

          <form className="form-card entity-form" onSubmit={handleSubmit}>
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

              <div className="agent-config-details">
                <FormField label={t('agentConfig.name')} htmlFor="agent-name" required>
                  <input
                    id="agent-name"
                    value={agentForm.name}
                    onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                    placeholder={t('agentConfig.placeholder_name')}
                    disabled={isSubmittingAgent || activeAgent?.is_builtin}
                  />
                </FormField>

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
                    >
                      <option value="">{t('agentConfig.placeholder_provider')}</option>
                      {providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name}</option>)}
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
                    >
                      <option value="">{t('agentConfig.placeholder_model')}</option>
                      {selectedAgentProvider?.models.map((model) => <option key={model} value={model}>{model}</option>)}
                    </select>
                  </FormField>
                </div>

                <div className="agent-description-field">
                  <FormField label={t('agentConfig.description')} htmlFor="agent-description" required>
                    <textarea
                      id="agent-description"
                      value={agentForm.description}
                      onChange={(e) => { setAgentForm({ ...agentForm, description: e.target.value }); setAgentFormDirty(true); }}
                      placeholder={t('agentConfig.placeholder_description')}
                      disabled={isSubmittingAgent || activeAgent?.is_builtin}
                      rows={3}
                      required
                    />
                  </FormField>
                </div>
              </div>
            </div>
            {activeAgent ? (
              <>
                <div className="workspace-editor">
                  <PaneHeader compact title={t('agentConfig.workspace')} description={isLoadingWorkspace ? t('chat.loading') : t('agentConfig.workspaceHint')} />
                  <div className="workspace-tabs" role="tablist" aria-label={t('agentConfig.workspace')}>
                    {workspaceTabs.map((field) => (
                      <button
                        key={field}
                        type="button"
                        className={`workspace-tab${activeWorkspaceField === field ? ' active' : ''}`}
                        role="tab"
                        aria-selected={activeWorkspaceField === field}
                        aria-controls="agent-workspace-field"
                        onClick={() => setActiveWorkspaceField(field)}
                      >
                        {t(`agentConfig.${field}`)}
                      </button>
                    ))}
                  </div>
                  <FormField label={activeWorkspaceLabel} htmlFor="agent-workspace-field">
                    <textarea
                      id="agent-workspace-field"
                      role="tabpanel"
                      value={workspaceForm[activeWorkspaceField]}
                      onChange={(e) => { setWorkspaceForm({ ...workspaceForm, [activeWorkspaceField]: e.target.value }); setAgentFormDirty(true); }}
                      rows={12}
                      disabled={isSubmittingAgent || isLoadingWorkspace}
                    />
                  </FormField>
                </div>
                <div className="agent-tools-section">
                  <TerminalToolSettings scope="agent" agentId={activeAgent.id} />
                </div>
              </>
            ) : null}
            {!activeAgent ? (
              <div className="form-actions">
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


function agentPayloadFromAgent(agent: Agent): CreateAgentPayload {
  return {
    name: agent.name,
    description: agent.description,
    model_provider_id: agent.model_provider_id,
    model_id: agent.model_id,
    avatar_icon: agent.avatar_icon,
    avatar_color: agent.avatar_color,
  };
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
