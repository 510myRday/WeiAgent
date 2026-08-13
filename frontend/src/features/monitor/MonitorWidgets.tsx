import type { Agent, LlmLogEntry, ModelConfig, ModelProvider } from '../../types';
import { agentDisplayName } from '../../components/AgentListItem';
import { formatSecondsValue } from './monitorFormatting';

type Translate = (path: string, params?: Record<string, string | number>) => string;

export function resolveAgent(
  log: Pick<LlmLogEntry, 'agent_id' | 'agent_name'>,
  agentById: Map<string, Agent>,
  agentByName: Map<string, Agent>,
): Agent | undefined {
  if (log.agent_id) {
    const byId = agentById.get(log.agent_id);
    if (byId) return byId;
  }
  if (log.agent_name) return agentByName.get(log.agent_name);
  return undefined;
}

export function resolveAgentName(
  log: Pick<LlmLogEntry, 'agent_id' | 'agent_name'>,
  agentById: Map<string, Agent>,
  agentByName: Map<string, Agent>,
  t: Translate,
): string {
  const agent = resolveAgent(log, agentById, agentByName);
  if (agent) return agentDisplayName(agent, t);
  return log.agent_name || t('llmLogs.unknownAgent');
}

export function findModelConfig(
  providers: ModelProvider[],
  providerName: string,
  modelId: string,
): ModelConfig | undefined {
  const provider = providers.find((item) => item.name === providerName)
    ?? providers.find((item) => item.model_configs.some((config) => config.model_id === modelId)
      || item.models.includes(modelId));
  if (!provider) return undefined;
  return provider.model_configs.find((config) => config.model_id === modelId);
}

/** Unit prices are USD / 1M tokens. Missing prices count as 0. */
export function calculateCostUsd(
  log: Pick<LlmLogEntry, 'provider_name' | 'model_id' | 'input_tokens' | 'output_tokens' | 'cache_write_tokens' | 'cache_read_tokens' | 'cost_usd'>,
  providers: ModelProvider[],
): number {
  const config = findModelConfig(providers, log.provider_name, log.model_id);
  if (!config) {
    return log.cost_usd ?? 0;
  }
  const input = log.input_tokens ?? 0;
  const output = log.output_tokens ?? 0;
  const cacheWrite = log.cache_write_tokens ?? 0;
  const cacheRead = log.cache_read_tokens ?? 0;
  return (
    input * (config.input_price ?? 0)
    + output * (config.output_price ?? 0)
    + cacheWrite * (config.cache_write_price ?? 0)
    + cacheRead * (config.cache_read_price ?? 0)
  ) / 1_000_000;
}

type LatencyTone =
  | 'under5'
  | 's5_10'
  | 's10_15'
  | 's15_20'
  | 's20_25'
  | 's25_30'
  | 's30_60'
  | 'm1_5'
  | 'm5_10'
  | 'over10'
  | 'unknown';

function latencyTone(ms: number | undefined): LatencyTone {
  if (ms == null) return 'unknown';
  if (ms < 5_000) return 'under5';
  if (ms < 10_000) return 's5_10';
  if (ms < 15_000) return 's10_15';
  if (ms < 20_000) return 's15_20';
  if (ms < 25_000) return 's20_25';
  if (ms < 30_000) return 's25_30';
  if (ms < 60_000) return 's30_60';
  if (ms < 5 * 60_000) return 'm1_5';
  if (ms < 10 * 60_000) return 'm5_10';
  return 'over10';
}

export function LatencyBadge({
  ms,
  t,
}: {
  ms: number | undefined;
  t: Translate;
}) {
  const tone = latencyTone(ms);
  return (
    <span className={`monitor-latency-badge is-${tone}`}>
      {formatSecondsValue(ms, t)}
    </span>
  );
}

export function SplitValue({
  left,
  right,
  leftMono = false,
  rightMono = true,
}: {
  left: string;
  right: string;
  leftMono?: boolean;
  rightMono?: boolean;
}) {
  return (
    <span className="monitor-split-cell">
      <span className={leftMono ? 'mono' : undefined}>{left}</span>
      <span className="monitor-split-sep" aria-hidden="true">|</span>
      <span className={rightMono ? 'mono' : undefined}>{right}</span>
    </span>
  );
}

export function isFailedRequest(status?: number, error?: string | null) {
  if (status != null && status >= 400) return true;
  if (error && error.trim()) return true;
  return false;
}

export function StatusBadge({
  status,
  unknownLabel,
}: {
  status?: number;
  unknownLabel: string;
}) {
  const tone = status == null
    ? 'unknown'
    : status >= 400
      ? 'error'
      : status >= 300
        ? 'warning'
        : status >= 200
          ? 'success'
          : 'warning';
  return (
    <span className={`monitor-status is-${tone}`}>
      <span className="monitor-status-dot" aria-hidden="true" />
      <span>{status ?? unknownLabel}</span>
    </span>
  );
}
