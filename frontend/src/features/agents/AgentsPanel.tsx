import { useState, useEffect, useRef, FormEvent } from 'react';
import { Plus, Save, ShieldCheck, Trash2 } from 'lucide-react';

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
  onOpenMain: () => void;
}

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  system_prompt: 'You are a helpful AI assistant.',
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

export function AgentsPanel({
  agents,
  providers,
  activeAgentId,
  isLoadingAgents,
  onAgentChange,
  onAgentAdd,
  onAgentEdit,
  onAgentRemove,
  onOpenMain,
}: AgentsPanelProps) {
  const { t } = useI18n();
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [workspaceForm, setWorkspaceForm] = useState(defaultWorkspaceForm);
  const [isLoadingWorkspace, setIsLoadingWorkspace] = useState(false);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [error, setError] = useState('');
  const agentFormDirtyRef = useRef(false);
  agentFormDirtyRef.current = agentFormDirty;

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const selectedAgentProvider = providers.find((provider) => provider.id === agentForm.model_provider_id);

  useEffect(() => {
    if (!activeAgent) {
      if (agentFormDirtyRef.current) return;
      const provider = providers[0];
      setAgentForm({
        ...defaultAgentForm,
        model_provider_id: provider?.id ?? '',
        model_id: provider?.models[0] ?? '',
      });
      setWorkspaceForm(defaultWorkspaceForm);
      return;
    }
    setAgentForm({
      name: activeAgent.name,
      system_prompt: activeAgent.system_prompt,
      model_provider_id: activeAgent.model_provider_id,
      model_id: activeAgent.model_id,
      avatar_icon: activeAgent.avatar_icon,
      avatar_color: activeAgent.avatar_color,
    });
    setAgentFormDirty(false);
  }, [activeAgent, providers]);

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
      name: agentForm.name.trim(),
      system_prompt: agentForm.system_prompt.trim(),
      model_provider_id: agentForm.model_provider_id,
      model_id: agentForm.model_id.trim(),
      avatar_icon: agentForm.avatar_icon,
      avatar_color: agentForm.avatar_color,
    };
    if (!payload.name || !payload.system_prompt || !payload.model_provider_id || !payload.model_id) {
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
    if (!activeAgent || !activeAgent.can_delete) return;
    if (!window.confirm(t('deleteConfirm.message').replace('{name}', activeAgent.name))) return;
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
                title={agent.name}
                subtitle={agent.is_builtin ? agent.display_label ?? agentModelLabel(agent) : agentModelLabel(agent)}
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
            title={activeAgent ? activeAgent.name : t('agentConfig.newTitle')}
            description={activeAgent ? agentModelLabel(activeAgent) : t('agentConfig.selectAgent')}
            actions={activeAgent?.is_builtin ? (
              <span className="status-badge"><ShieldCheck size={15} />{activeAgent.display_label ?? t('agentConfig.builtin')}</span>
            ) : activeAgent ? (
              <IconButton variant="danger" type="button" onClick={handleDelete} label={t('agentConfig.delete')}>
                <Trash2 size={17} />
              </IconButton>
            ) : null}
          />

          {error ? <AlertBanner message={error} onDismiss={() => setError('')} dismissLabel={t('error.dismiss')} /> : null}

          <form className="form-card entity-form" onSubmit={handleSubmit}>
            <div className="form-grid two-column">
              <FormField label={t('agentConfig.name')} htmlFor="agent-name">
                <input
                  id="agent-name"
                  value={agentForm.name}
                  onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                  placeholder={t('agentConfig.placeholder_name')}
                  disabled={isSubmittingAgent}
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
              <FormField label={t('agentConfig.provider')} htmlFor="agent-provider">
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
              <FormField label={t('agentConfig.model')} htmlFor="agent-model">
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
            <FormField label={t('agentConfig.systemPrompt')} htmlFor="agent-system-prompt">
              <textarea
                id="agent-system-prompt"
                value={agentForm.system_prompt}
                onChange={(e) => { setAgentForm({ ...agentForm, system_prompt: e.target.value }); setAgentFormDirty(true); }}
                rows={10}
                disabled={isSubmittingAgent}
              />
            </FormField>
            {activeAgent?.model_note ? <p className="hint-text">{activeAgent.model_note}</p> : null}
            {activeAgent ? (
              <div className="workspace-editor">
                <PaneHeader compact title={t('agentConfig.workspace')} description={isLoadingWorkspace ? t('chat.loading') : t('agentConfig.workspaceHint')} />
                <div className="form-grid two-column">
                  <FormField label={t('agentConfig.personality')} htmlFor="agent-personality">
                    <textarea
                      id="agent-personality"
                      value={workspaceForm.personality}
                      onChange={(e) => { setWorkspaceForm({ ...workspaceForm, personality: e.target.value }); setAgentFormDirty(true); }}
                      rows={8}
                      disabled={isSubmittingAgent || isLoadingWorkspace}
                    />
                  </FormField>
                  <FormField label={t('agentConfig.memory')} htmlFor="agent-memory">
                    <textarea
                      id="agent-memory"
                      value={workspaceForm.memory}
                      onChange={(e) => { setWorkspaceForm({ ...workspaceForm, memory: e.target.value }); setAgentFormDirty(true); }}
                      rows={8}
                      disabled={isSubmittingAgent || isLoadingWorkspace}
                    />
                  </FormField>
                </div>
                <FormField label={t('agentConfig.instructions')} htmlFor="agent-instructions">
                  <textarea
                    id="agent-instructions"
                    value={workspaceForm.instructions}
                    onChange={(e) => { setWorkspaceForm({ ...workspaceForm, instructions: e.target.value }); setAgentFormDirty(true); }}
                    rows={9}
                    disabled={isSubmittingAgent || isLoadingWorkspace}
                  />
                </FormField>
              </div>
            ) : (
              <p className="hint-text">{t('agentConfig.workspaceCreateHint')}</p>
            )}
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
