import { useState, useEffect, useRef, FormEvent } from 'react';
import { Bot, Plus, Save, Trash2 } from 'lucide-react';
import type { Agent, ModelProvider, CreateAgentPayload } from '../types';
import { useI18n } from '../i18n';

interface AgentsPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  activeAgentId: string;
  isLoadingAgents: boolean;
  onAgentChange: (id: string) => void;
  onAgentAdd: (payload: CreateAgentPayload) => Promise<Agent>;
  onAgentEdit: (id: string, payload: Partial<CreateAgentPayload>) => Promise<Agent>;
  onAgentRemove: (id: string) => Promise<void>;
}

const defaultAgentForm: CreateAgentPayload = {
  name: '',
  system_prompt: 'You are a helpful AI assistant.',
  model_provider_id: '',
  model_id: '',
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
}: AgentsPanelProps) {
  const { t } = useI18n();
  const [agentForm, setAgentForm] = useState<CreateAgentPayload>(defaultAgentForm);
  const [agentFormDirty, setAgentFormDirty] = useState(false);
  const [isSubmittingAgent, setIsSubmittingAgent] = useState(false);
  const [error, setError] = useState('');
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
      return;
    }
    setAgentForm({
      name: activeAgent.name,
      system_prompt: activeAgent.system_prompt,
      model_provider_id: activeAgent.model_provider_id,
      model_id: activeAgent.model_id,
    });
    setAgentFormDirty(false);
  }, [activeAgent, providers]);

  function switchAgent(id: string) {
    if (agentFormDirty && !window.confirm(t('agentConfig.discardConfirm'))) return;
    setError('');
    onAgentChange(id);
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
    };
    if (!payload.name || !payload.system_prompt || !payload.model_provider_id || !payload.model_id) {
      setError(t('error.validation'));
      return;
    }
    setIsSubmittingAgent(true);
    setError('');
    try {
      if (activeAgent) {
        await onAgentEdit(activeAgent.id, payload);
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
    if (!activeAgent) return;
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
      <div className="config-sidebar agents-panel-sidebar">
        <div className="panel-header">
          <h2>{t('nav.agents')}</h2>
          <button
            className="icon-button light"
            type="button"
            onClick={() => { onAgentChange(''); setAgentFormDirty(false); }}
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
              <Bot size={17} />
              <span>
                <strong>{agent.name}</strong>
                <small>{agentModelLabel(agent)}</small>
              </span>
            </button>
          ))}
        </div>
      </div>

      <div className="config-main agents-panel-main">
        <div className="config-page-header">
          <div>
            <p className="eyebrow">{t('agentConfig.title')}</p>
            <h2>{activeAgent ? activeAgent.name : t('agentConfig.newTitle')}</h2>
            <p>{activeAgent ? agentModelLabel(activeAgent) : t('agentConfig.selectAgent')}</p>
          </div>
          {activeAgent ? (
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
