import type { Agent } from '../types';

export type AgentActivityMap = Record<string, string>;

export function sortAgentsByRecentConversation(
  agents: Agent[],
  latestConversationUpdatedAtByAgent: AgentActivityMap,
): Agent[] {
  return agents
    .map((agent, index) => ({
      agent,
      index,
      latestActivity: parseActivityTime(latestConversationUpdatedAtByAgent[agent.id]),
    }))
    .sort((left, right) => {
      if (right.latestActivity !== left.latestActivity) {
        return right.latestActivity - left.latestActivity;
      }
      return left.index - right.index;
    })
    .map(({ agent }) => agent);
}

function parseActivityTime(value: string | undefined): number {
  if (!value) return Number.NEGATIVE_INFINITY;
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? Number.NEGATIVE_INFINITY : timestamp;
}
