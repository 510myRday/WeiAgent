import {
  AUTO_SAVE_DELAY_MS,
  AUTO_SAVE_MAX_RETRY_DELAY_MS,
  AUTO_SAVE_RETRY_DELAY_MS,
  type AutoSaveStatus,
} from '../../../hooks/useSerializedAutoSave';

export {
  AUTO_SAVE_DELAY_MS,
  AUTO_SAVE_MAX_RETRY_DELAY_MS,
  AUTO_SAVE_RETRY_DELAY_MS,
};
export type { AutoSaveStatus };

import type {
  AgentToolsConfig,
  AgentToolsOverrides,
  WebSearchSourceRole,
} from '../../../toolTypes';

export type SourceSection = 'preferences' | 'keys' | 'connection';
export type ExpandedSourceSections = Record<string, Partial<Record<SourceSection, boolean>>>;
export type KeyCheckProgress = {
  total: number;
  completed: number;
  healthy: number;
  issues: number;
  activeKeyIndex: number | null;
  paused: boolean;
  pauseRequested: boolean;
};

export type PendingSave =
  | {
    contextKey: string;
    serialized: string;
    scope: 'defaults';
    config: AgentToolsConfig;
  }
  | {
    contextKey: string;
    serialized: string;
    scope: 'agent';
    agentId: string;
    overrides: AgentToolsOverrides;
  };

export type ProxyPickerField = 'proxy_url';
export type ProxyLatencySort = 'latency_asc' | 'latency_desc';

export const MAX_RESULTS = 20;
export const MAX_EVIDENCE_RETENTION_DAYS = 3650;
export const MAX_EVIDENCE_TOTAL_MB = 10240;
export const PRESERVE_KEYS_MARKER = '__preserve__';
export const SOURCE_ROLE_GROUPS: Array<{
  key: 'discovery' | 'content' | 'intent';
  roles: WebSearchSourceRole[];
}> = [
  {
    key: 'discovery',
    roles: ['general_search', 'semantic_research', 'multilingual_search'],
  },
  {
    key: 'content',
    roles: ['current_news', 'official_docs', 'technical_research', 'academic_research'],
  },
  {
    key: 'intent',
    roles: ['fact_checking', 'market_research', 'product_research'],
  },
];
