import type { Agent } from '../../types';
import { agentDisplayName } from '../../components/AgentListItem';
import { useI18n } from '../../i18n';

type ChatAgentSelectProps = {
  agents: Agent[];
  value: string;
  disabled?: boolean;
  onChange: (agentId: string) => void;
};

export function ChatAgentSelect({
  agents,
  value,
  disabled = false,
  onChange,
}: ChatAgentSelectProps) {
  const { t } = useI18n();

  return (
    <select
      className="chat-agent-select"
      value={value}
      aria-label={t('chat.currentAgent')}
      disabled={disabled || agents.length === 0}
      title={t('chat.currentAgent')}
      onChange={(event) => onChange(event.target.value)}
    >
      {agents.length === 0 ? (
        <option value="">{t('chat.selectAgent')}</option>
      ) : null}
      {agents.map((agent) => (
        <option key={agent.id} value={agent.id}>
          {agentDisplayName(agent, t)}
        </option>
      ))}
    </select>
  );
}
