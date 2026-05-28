import { useState, useEffect, useRef, FormEvent } from 'react';
import { Plus, Save, ShieldCheck, Trash2 } from 'lucide-react';
import type { Agent, AgentWorkspace, ModelProvider, CreateAgentPayload } from '../types';
import { readAgentWorkspace, updateAgentWorkspace } from '../api';
import { useI18n } from '../i18n';
import { AvatarBadge, AvatarPicker } from './AvatarPicker';

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
  onBackToRail: () => void;
}

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  system_prompt: 'You are a helpful AI assistant.',
  model_provider_id: '',
  model_id: '',
  avatar_icon: 'bot',
  avatar_color: '#22c55e',
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
  onBackToRail,
}: AgentsPanelProps) {
  const { t, locale } = useI18n();
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [workspaceForm, setWorkspaceForm] = useState(defaultWorkspaceForm);
  const [isLoadingWorkspace, setIsLoadingWorkspace] = useState(false);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [error, setError] = useState('');
  const [showAgentAvatarPicker, setShowAgentAvatarPicker] = useState(false);
  const agentFormDirtyRef = useRef(false);
  agentFormDirtyRef.current = agentFormDirty;

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const selectedAgentProvider = providers.find((p) => p.id === agentForm.model_provider_id);

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
    <section className="config-panel agents-panel">
      <div className="config-sidebar context-pane agents-panel-sidebar">
        <div className="panel-header mobile-pane-header">
          <button className="back-button" type="button" onClick={onBackToRail}>{t('common.back')}</button>
          <h2>{t('nav.agents')}</h2>
          <button
            className="icon-button light"
            type="button"
              onClick={() => { onAgentChange(''); setAgentFormDirty(false); onOpenMain(); }}
            title={t('agentConfig.create')}
            aria-label={t('agentConfig.create')}
          >
            <Plus size={17} />
          </button>
        </div>
        <div className="agent-list">
          {isLoadingAgents && agents.length === 0 ? <p className="muted">{t('chat.loading')}</p> : null}
          {!isLoadingAgents && agents.length === 0 ? <p className="muted">{t('agentConfig.noAgents')}</p> : null}
          {agents.map((agent) => (
            <button
              className={agent.id === activeAgentId ? 'agent-item active' : 'agent-item'}
              key={agent.id}
              type="button"
              onClick={() => switchAgent(agent.id)}
            >
              <AvatarBadge icon={agent.avatar_icon} color={agent.avatar_color} size={18} className="row-avatar agent-list-avatar" />
              <span>
                <strong>{agent.name}</strong>
                <small>{agent.is_builtin ? agent.display_label ?? agentModelLabel(agent) : agentModelLabel(agent)}</small>
              </span>
            </button>
          ))}
        </div>
      </div>

      <div className="config-main main-pane agents-panel-main">
        <div className="config-page-header">
          <div>
            <p className="eyebrow">{t('agentConfig.title')}</p>
            <h2>{activeAgent ? activeAgent.name : t('agentConfig.newTitle')}</h2>
            <p>{activeAgent ? agentModelLabel(activeAgent) : t('agentConfig.selectAgent')}</p>
          </div>
          {activeAgent?.is_builtin ? (
            <span className="builtin-badge"><ShieldCheck size={15} />{activeAgent.display_label ?? t('agentConfig.builtin')}</span>
          ) : activeAgent ? (
            <button className="icon-button danger light" type="button" onClick={handleDelete} title={t('agentConfig.delete')} aria-label={t('agentConfig.delete')}>
              <Trash2 size={17} />
            </button>
          ) : null}
        </div>

        {error ? (
          <div className="error-banner">
            <span>{error}</span>
            <button className="error-dismiss" type="button" onClick={() => setError('')} aria-label={t('error.dismiss')}>&times;</button>
          </div>
        ) : null}

        <form className="agent-form form-card" onSubmit={handleSubmit}>
          <div className="form-grid two-column">
            <div className="field">
              <label htmlFor="agent-name">{t('agentConfig.name')}</label>
              <input
                id="agent-name"
                value={agentForm.name}
                onChange={(e) => { setAgentForm({ ...agentForm, name: e.target.value }); setAgentFormDirty(true); }}
                placeholder={t('agentConfig.placeholder_name')}
                disabled={isSubmittingAgent}
              />
            </div>
            <div className="field">
              <label>{t('agentConfig.avatar')}</label>
              <div className="agent-avatar-picker-wrapper">
                <button
                  type="button"
                  className="agent-avatar-trigger"
                  onClick={() => setShowAgentAvatarPicker(v => !v)}
                >
                  <AvatarBadge
                    icon={agentForm.avatar_icon}
                    color={agentForm.avatar_color}
                    size={24}
                    className="row-avatar circular-avatar-inner"
                  />
                  <span className="agent-avatar-trigger-label">{t('agentConfig.avatarChange')}</span>
                </button>
                {showAgentAvatarPicker && (
                  <>
                    <div className="avatar-picker-backdrop" onClick={() => setShowAgentAvatarPicker(false)} />
                    <div className="avatar-picker agent-avatar-popover">
                      <p className="avatar-picker-title">{t('agentConfig.avatarChange')}</p>
                      <AvatarPicker
                        icon={agentForm.avatar_icon}
                        color={agentForm.avatar_color}
                        title={locale === 'zh' ? '智能体头像' : 'Agent Avatar'}
                        onChange={(next) => {
                          setAgentForm({ ...agentForm, avatar_icon: next.icon, avatar_color: next.color });
                          setAgentFormDirty(true);
                        }}
                      />
                    </div>
                  </>
                )}
              </div>
            </div>
            <div className="field">
              <label htmlFor="agent-provider">{t('agentConfig.provider')}</label>
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
                {providers.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
            </div>
            <div className="field">
              <label htmlFor="agent-model">{t('agentConfig.model')}</label>
              <select
                id="agent-model"
                value={agentForm.model_id}
                onChange={(e) => { setAgentForm({ ...agentForm, model_id: e.target.value }); setAgentFormDirty(true); }}
                disabled={isSubmittingAgent || !selectedAgentProvider}
              >
                <option value="">{t('agentConfig.placeholder_model')}</option>
                {selectedAgentProvider?.models.map((model) => <option key={model} value={model}>{model}</option>)}
              </select>
            </div>
          </div>
          <div className="field">
            <label htmlFor="agent-system-prompt">{t('agentConfig.systemPrompt')}</label>
            <textarea
              id="agent-system-prompt"
              value={agentForm.system_prompt}
              onChange={(e) => { setAgentForm({ ...agentForm, system_prompt: e.target.value }); setAgentFormDirty(true); }}
              rows={10}
              disabled={isSubmittingAgent}
            />
          </div>
          {activeAgent?.model_note ? <p className="hint-text">{activeAgent.model_note}</p> : null}
          {activeAgent ? (
            <div className="workspace-editor">
              <div className="panel-header compact">
                <div>
                  <h3>{t('agentConfig.workspace')}</h3>
                  <p className="muted">{isLoadingWorkspace ? t('chat.loading') : t('agentConfig.workspaceHint')}</p>
                </div>
              </div>
              <div className="form-grid two-column">
                <div className="field">
                  <label htmlFor="agent-personality">{t('agentConfig.personality')}</label>
                  <textarea
                    id="agent-personality"
                    value={workspaceForm.personality}
                    onChange={(e) => { setWorkspaceForm({ ...workspaceForm, personality: e.target.value }); setAgentFormDirty(true); }}
                    rows={8}
                    disabled={isSubmittingAgent || isLoadingWorkspace}
                  />
                </div>
                <div className="field">
                  <label htmlFor="agent-memory">{t('agentConfig.memory')}</label>
                  <textarea
                    id="agent-memory"
                    value={workspaceForm.memory}
                    onChange={(e) => { setWorkspaceForm({ ...workspaceForm, memory: e.target.value }); setAgentFormDirty(true); }}
                    rows={8}
                    disabled={isSubmittingAgent || isLoadingWorkspace}
                  />
                </div>
              </div>
              <div className="field">
                <label htmlFor="agent-instructions">{t('agentConfig.instructions')}</label>
                <textarea
                  id="agent-instructions"
                  value={workspaceForm.instructions}
                  onChange={(e) => { setWorkspaceForm({ ...workspaceForm, instructions: e.target.value }); setAgentFormDirty(true); }}
                  rows={9}
                  disabled={isSubmittingAgent || isLoadingWorkspace}
                />
              </div>
            </div>
          ) : (
            <p className="hint-text">{t('agentConfig.workspaceCreateHint')}</p>
          )}
          <div className="agent-form-actions form-actions-sticky">
            <button className="primary-button" type="submit" disabled={isSubmittingAgent}>
              <Save size={17} />
              {activeAgent ? t('agentConfig.save') : t('agentConfig.create')}
            </button>
          </div>
        </form>
      </div>
    </section>
  );
}
