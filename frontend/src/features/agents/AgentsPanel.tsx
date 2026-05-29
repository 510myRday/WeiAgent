import { useState, useEffect, useRef, FormEvent } from 'react';
import { Plus, Save, ShieldCheck, SlidersHorizontal, Trash2 } from 'lucide-react';

import type { Agent, AgentWorkspace, ModelProvider, CreateAgentPayload } from '../../types';
import { readAgentWorkspace, updateAgentWorkspace } from '../../api';
import { useI18n } from '../../i18n';
import { defaultAvatarColor } from '../../avatarPresets';
import { AvatarBadge } from '../../components/AvatarPicker';
import { PanelShell } from '../../components/layout';
import { AlertBanner, AvatarPopover, Button, FormField, IconButton, ListItem, PaneHeader } from '../../components/ui';

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
}

const defaultAgentForm: CreateAgentPayload = {
  name: '',
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

const workspaceTabs: WorkspaceField[] = ['personality', 'memory', 'instructions'];

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
}: AgentsPanelProps) {
  const { t } = useI18n();
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [workspaceForm, setWorkspaceForm] = useState(defaultWorkspaceForm);
  const [activeWorkspaceField, setActiveWorkspaceField] = useState<WorkspaceField>('personality');
  const [isLoadingWorkspace, setIsLoadingWorkspace] = useState(false);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [error, setError] = useState('');
  const agentFormDirtyRef = useRef(false);
  agentFormDirtyRef.current = agentFormDirty;

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const selectedAgentProvider = providers.find((provider) => provider.id === agentForm.model_provider_id);
  const activeWorkspaceLabel = t(`agentConfig.${activeWorkspaceField}`);
  const displayAgentName = (agent: Agent) => agent.is_builtin ? t('agentConfig.builtinMasterName') : agent.name;

  useEffect(() => {
    if (!activeAgent) {
      if (agentFormDirtyRef.current) return;
      setAgentForm(defaultAgentForm);
      setWorkspaceForm(defaultWorkspaceForm);
      return;
    }
    setAgentForm({
      name: displayAgentName(activeAgent),
      model_provider_id: activeAgent.model_provider_id,
      model_id: activeAgent.model_id,
      avatar_icon: activeAgent.avatar_icon,
      avatar_color: activeAgent.avatar_color,
    });
    setAgentFormDirty(false);
  }, [activeAgent, t]);

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
        setWorkspaceForm({
          personality: workspace.personality,
          memory: workspace.memory,
          instructions: workspace.instructions,
        });
      })
      .catch((cause) => setError(cause instanceof Error ? cause.message : 'Error'))
      .finally(() => {
        if (!cancelled) setIsLoadingWorkspace(false);
      });
    return () => {
      cancelled = true;
    };
  }, [activeAgent]);

  function switchAgent(id: string) {
    if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return;
    setError('');
    onAgentChange(id);
    onOpenMain();
  }

  function agentModelLabel(agent: Agent) {
    const provider = providers.find((p) => p.id === agent.model_provider_id);
    return provider ? `${provider.name} / ${agent.model_id}` : agent.model_id;
  }

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const payload = {
      name: activeAgent?.is_builtin ? activeAgent.name : agentForm.name.trim(),
      model_provider_id: agentForm.model_provider_id,
      model_id: agentForm.model_id.trim(),
      avatar_icon: agentForm.avatar_icon,
      avatar_color: agentForm.avatar_color,
    };
    if ((!payload.name && !activeAgent?.is_builtin) || !payload.model_provider_id || !payload.model_id) {
      setError(t('error.validation'));
      return;
    }
    if (agents.some((agent) =>
      agent.id !== activeAgent?.id
      && agent.avatar_icon === payload.avatar_icon
      && agent.avatar_color.toLowerCase() === payload.avatar_color.toLowerCase()
    )) {
      setError(t('error.avatarDuplicate'));
      return;
    }
    setIsSubmittingAgent(true);
    setError('');
    try {
      if (activeAgent) {
        await onAgentEdit(activeAgent.id, payload);
        await updateAgentWorkspace(activeAgent.id, workspaceForm);
      } else {
        await onAgentAdd(payload);
      }
      setAgentFormDirty(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    } finally {
      setIsSubmittingAgent(false);
    }
  }

  async function handleDelete() {
    if (!activeAgent || activeAgent.is_builtin) return;
    if (!window.confirm(t('deleteConfirm.message').replace('{name}', displayAgentName(activeAgent)))) return;
    try {
      await onAgentRemove(activeAgent.id);
      setAgentFormDirty(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
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
            leading={<Button className="back-button" type="button" onClick={onOpenMain}>{t('common.back')}</Button>}
            actions={(
              <IconButton
                variant="ghost"
                type="button"
                onClick={() => { onAgentChange(''); setAgentFormDirty(false); onOpenMain(); }}
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
              <ListItem
                key={agent.id}
                type="button"
                active={agent.id === activeAgentId}
                title={(
                  <span className="agent-list-title">
                    <span>{displayAgentName(agent)}</span>
                    {agent.is_builtin ? <span className="mini-status-badge"><ShieldCheck size={12} />{t('agentConfig.builtinBadge')}</span> : null}
                  </span>
                )}
                subtitle={agentModelLabel(agent)}
                leading={<AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} />}
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
            eyebrow={t('agentConfig.title')}
            title={activeAgent ? displayAgentName(activeAgent) : t('agentConfig.newTitle')}
            description={activeAgent ? agentModelLabel(activeAgent) : t('agentConfig.selectAgent')}
            actions={(
              <>
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

          {error ? <AlertBanner message={error} onDismiss={() => setError('')} dismissLabel={t('error.dismiss')} /> : null}

          <form className="form-card entity-form" onSubmit={handleSubmit}>
            <div className="form-grid two-column">
              <FormField label={t('agentConfig.name')} htmlFor="agent-name" required>
                <input
                  id="agent-name"
                  value={agentForm.name}
                  onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                  placeholder={t('agentConfig.placeholder_name')}
                  disabled={isSubmittingAgent || activeAgent?.is_builtin}
                />
              </FormField>
              <FormField label={t('agentConfig.avatar')}>
                <AvatarPopover
                  icon={agentForm.avatar_icon}
                  color={agentForm.avatar_color}
                  title={t('agentConfig.avatarPreview')}
                  pickerTitle={t('agentConfig.avatarChange')}
                  onChange={(next) => {
                    setAgentForm({ ...agentForm, avatar_icon: next.icon, avatar_color: next.color });
                    setAgentFormDirty(true);
                  }}
                  trigger={() => (
                    <button type="button" className="avatar-trigger">
                      <AvatarBadge icon={agentForm.avatar_icon} color={agentForm.avatar_color} size={24} />
                      <span>{t('agentConfig.avatarChange')}</span>
                    </button>
                  )}
                />
              </FormField>
              <FormField label={t('agentConfig.provider')} htmlFor="agent-provider" required>
                <div className="field-with-action">
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
                  <IconButton variant="ghost" type="button" onClick={onOpenProviderSettings} label={t('agentConfig.openProviderSettings')}>
                    <SlidersHorizontal size={17} />
                  </IconButton>
                </div>
              </FormField>
              <FormField label={t('agentConfig.model')} htmlFor="agent-model" required>
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
            {activeAgent ? (
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
            ) : null}
            <div className="form-actions">
              <Button variant="primary" type="submit" disabled={isSubmittingAgent} icon={<Save size={17} />}>
                {activeAgent ? t('agentConfig.save') : t('agentConfig.create')}
              </Button>
            </div>
          </form>
        </div>
      )}
    />
  );
}
