import type {
  Agent,
  AgentProfile,
  AgentRun,
  AgentWorkspace,
  AppSettings,
  Conversation,
  ConversationUsage,
  ConversationMessagePage,
  CreateAgentPayload,
  CreateConversationPayload,
  FetchProviderModelsPayload,
  LlmLogDetail,
  LlmLogListPage,
  ModelProvider,
  ModelProviderPayload,
  ImageAttachment,
  GenerationMode,
  IpProxy,
  IpProxyImportEntry,
  IpProxyImportResult,
  IpProxyPayload,
  ThinkingLevel,
  StreamEvent,
  UpdateAgentProfilePayload,
  UpdateAgentPayload,
  UpdateConversationPayload,
} from './types';
import {
  agentRunListRuntimeSchema,
  agentRunRuntimeSchema,
  conversationMessagePageRuntimeSchema,
  nullableAgentRunRuntimeSchema,
  parseJsonStreamEvent,
} from './apiSchemas';
import { createAbortSignal } from './utils/abort';
import { apiAuthorizationHeaders } from './apiAuth';

const DEFAULT_TIMEOUT_MS = 30000;
const CONVERSATION_TITLE_REQUEST_TIMEOUT_MS = 65000;

type RuntimeSchema<T> = {
  parse: (value: unknown) => T;
};

async function request<T>(
  path: string,
  options?: RequestInit,
  schema?: RuntimeSchema<T>,
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<T> {
  const abortHandle = createAbortSignal(timeoutMs, options?.signal ?? undefined);
  try {
    const response = await fetch(path, {
      ...options,
      signal: abortHandle.signal,
      headers: {
        'content-type': 'application/json',
        ...apiAuthorizationHeaders(),
        ...options?.headers,
      },
    });

    if (!response.ok) {
      throw new Error(await readError(response));
    }

    const text = await response.text();
    if (!text.trim()) {
      return undefined as T;
    }

    const parsed: unknown = JSON.parse(text);
    return schema ? schema.parse(parsed) : parsed as T;
  } finally {
    abortHandle.dispose();
  }
}

async function readError(response: Response): Promise<string> {
  try {
    const text = await response.text();
    if (!text.trim()) return response.statusText;
    const body = JSON.parse(text);
    return body.error ?? response.statusText;
  } catch {
    return response.statusText;
  }
}

export function readSettings(): Promise<AppSettings> {
  return request<AppSettings>('/api/settings');
}

export function updateSettings(payload: AppSettings): Promise<AppSettings> {
  return request<AppSettings>('/api/settings', {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function listModelProviders(): Promise<ModelProvider[]> {
  return request<ModelProvider[]>('/api/model-providers');
}

export function createModelProvider(payload: ModelProviderPayload): Promise<ModelProvider> {
  return request<ModelProvider>('/api/model-providers', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateModelProvider(
  providerId: string,
  payload: Partial<ModelProviderPayload>,
): Promise<ModelProvider> {
  return request<ModelProvider>(`/api/model-providers/${providerId}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteModelProvider(providerId: string): Promise<void> {
  return request<void>(`/api/model-providers/${providerId}`, {
    method: 'DELETE',
  });
}

export function fetchModels(providerId: string): Promise<string[]> {
  return request<string[]>(`/api/model-providers/${providerId}/models`);
}

export function fetchModelsForProvider(payload: FetchProviderModelsPayload): Promise<string[]> {
  return request<string[]>('/api/model-providers/models', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function listAgents(): Promise<Agent[]> {
  return request<Agent[]>('/api/agents');
}

export function createAgent(payload: CreateAgentPayload): Promise<Agent> {
  return request<Agent>('/api/agents', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateAgent(agentId: string, payload: UpdateAgentPayload): Promise<Agent> {
  return request<Agent>(`/api/agents/${agentId}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteAgent(agentId: string): Promise<void> {
  return request<void>(`/api/agents/${agentId}`, {
    method: 'DELETE',
  });
}

export function readAgentWorkspace(agentId: string): Promise<AgentWorkspace> {
  return request<AgentWorkspace>(`/api/agents/${agentId}/workspace`);
}

export function listConversations(agentId: string): Promise<Conversation[]> {
  return request<Conversation[]>(`/api/agents/${agentId}/conversations`);
}

export function createConversation(
  agentId: string,
  payload: CreateConversationPayload,
): Promise<Conversation> {
  return request<Conversation>(`/api/agents/${agentId}/conversations`, {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateAgentProfile(
  agentId: string,
  payload: UpdateAgentProfilePayload,
): Promise<AgentProfile> {
  return request<AgentProfile>(`/api/agents/${agentId}/profile`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function updateConversation(
  conversationId: string,
  payload: UpdateConversationPayload,
): Promise<Conversation> {
  return request<Conversation>(`/api/conversations/${conversationId}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteConversation(conversationId: string): Promise<void> {
  return request<void>(`/api/conversations/${conversationId}`, {
    method: 'DELETE',
  });
}

export type ListLlmLogsParams = {
  offset?: number;
  limit?: number;
  agentId?: string;
  agentName?: string;
  providerName?: string;
  modelId?: string;
  q?: string;
  since?: string;
  until?: string;
  status?: 'success' | 'error';
};

export function listLlmLogs(params: ListLlmLogsParams = {}): Promise<LlmLogListPage> {
  const search = new URLSearchParams();
  if (params.offset != null) search.set('offset', String(params.offset));
  if (params.limit != null) search.set('limit', String(params.limit));
  if (params.agentId) search.set('agent_id', params.agentId);
  if (params.agentName) search.set('agent_name', params.agentName);
  if (params.providerName) search.set('provider_name', params.providerName);
  if (params.modelId) search.set('model_id', params.modelId);
  if (params.q) search.set('q', params.q);
  if (params.since) search.set('since', params.since);
  if (params.until) search.set('until', params.until);
  if (params.status) search.set('status', params.status);
  const query = search.toString();
  return request<LlmLogListPage>(query ? `/api/llm-logs?${query}` : '/api/llm-logs');
}

export function getLlmLog(logId: string): Promise<LlmLogDetail> {
  return request<LlmLogDetail>(`/api/llm-logs/${logId}`);
}


export type WebSearchProviderKind = 'tavily' | 'exa' | 'jina' | 'grok' | 'firecrawl';
export const MAX_WEB_SEARCH_KEYS_PER_SOURCE = 1_000;
export type WebSearchKeySelection = 'sequential' | 'round_robin' | 'random';
export type WebSearchSourceRole =
  | 'general_search'
  | 'semantic_research'
  | 'current_news'
  | 'official_docs'
  | 'technical_research'
  | 'academic_research'
  | 'fact_checking'
  | 'market_research'
  | 'product_research'
  | 'multilingual_search'
  | 'ai_research'
  | 'social_search';

export type WebSearchSourceConfig = {
  id: string;
  provider: WebSearchProviderKind;
  display_name: string;
  enabled: boolean;
  deep_enabled: boolean;
  extract_enabled: boolean;
  priority: number;
  independence_group: string;
  roles: WebSearchSourceRole[];
  api_keys: string[];
  /** SHA-256 fingerprints of keys confirmed as officially blocked (omitted when empty). */
  blocked_api_key_hashes?: string[];
  key_statuses?: Array<WebSearchKeyStatusRecord | null>;
  extract_key_statuses?: Array<WebSearchKeyStatusRecord | null>;
  proxy_url?: string | null;
};

export type WebContentExtractionConfig = {
  remote_enabled: boolean;
  max_remote_attempts_per_url: number;
  allow_grok_fallback: boolean;
};

export type WebSearchToolConfig = {
  enabled: boolean;
  max_results: number;
  timeout_ms: number;
  key_selection: WebSearchKeySelection;
  extraction: WebContentExtractionConfig;
  sources: WebSearchSourceConfig[];
};

export type EvidenceStorageConfig = {
  enabled: boolean;
  retention_days: number;
  max_total_mb: number;
};

export type WebSearchKeyStatusRecord = {
  status: WebSearchKeyStatus;
  detail: string;
  checked_at: string;
};

export type WebSearchKeyStatus =
  | 'normal'
  | 'officially_blocked'
  | 'invalid_key'
  | 'quota_exhausted'
  | 'rate_limited'
  | 'permission_denied'
  | 'configuration_error'
  | 'service_error'
  // Grok-classifier vocabulary, actively produced by the backend key checker.
  | 'quota_limited'
  | 'unauthorized'
  | 'anti_bot'
  | 'network_error'
  | 'error';

export type WebSearchKeyCheckResult = {
  key_index: number;
  key_hint: string;
  status: WebSearchKeyStatus;
  detail: string;
  latency_ms: number;
  checked_at: string;
};

export type WebSearchKeyCheckResponse = {
  source_id: string;
  results: WebSearchKeyCheckResult[];
};

export type WebSearchKeyImportResponse = {
  source_id: string;
  imported: number;
  duplicates: number;
  blocked: number;
  source: WebSearchSourceConfig;
};

export type WebSearchKeyDeleteResponse = {
  source_id: string;
  source: WebSearchSourceConfig;
};

export type AgentToolsConfig = {
  max_llm_iterations: number;
  terminal: {
    enabled: boolean;
    permission_mode: 'request_approval' | 'full_access';
    approval_grants: Array<{
      command: string;
      cwd?: string;
      permission_mode: 'request_approval';
      created_at?: string;
      expires_at?: string;
    }>;
    default_timeout_ms: number;
    allowed_cwd_roots: string[];
  };
  web_search: WebSearchToolConfig;
  evidence_storage: EvidenceStorageConfig;
  task_planning: {
    enabled: boolean;
    progress_interval: number;
  };
};

export type AgentToolsOverrides = {
  version: 1;
  max_llm_iterations?: number;
  terminal: {
    enabled?: boolean;
    permission_mode?: 'request_approval' | 'full_access';
    approval_grants: AgentToolsConfig['terminal']['approval_grants'];
    default_timeout_ms?: number;
    allowed_cwd_roots?: string[];
  };
  web_search: {
    enabled?: boolean;
    max_results?: number;
  };
  task_planning: {
    enabled?: boolean;
    progress_interval?: number;
  };
};

export type AgentToolsSettings = {
  defaults: AgentToolsConfig;
  overrides: AgentToolsOverrides;
  effective: AgentToolsConfig;
};

export type TaskPlan = {
  conversation_id: string;
  planning_id: string;
  conversation_message_id: string | null;
  explanation?: string;
  plan: Array<{
    step: string;
    status: 'pending' | 'in_progress' | 'completed';
  }>;
  created_at: string;
  updated_at: string;
  active: boolean;
};

export function readTaskPlan(conversationId: string): Promise<TaskPlan | null> {
  return request<TaskPlan | null>(`/api/conversations/${conversationId}/task-plan`);
}

export function readTaskPlans(conversationId: string): Promise<TaskPlan[]> {
  return request<TaskPlan[]>(`/api/conversations/${conversationId}/task-plans`);
}

export type EnvironmentRuntimeStatus = {
  supported: boolean;
  available: boolean;
  managed_installed: boolean;
  installing: boolean;
  target_version: string;
  update_available: boolean;
  platform?: string;
  source?: 'managed' | 'environment' | 'system';
  detected_version?: string;
  executable?: string;
  error?: string;
  components?: RuntimeComponentStatus[];
};

export type RuntimeComponentStatus = {
  kind: 'node' | 'python' | 'uv' | 'git';
  available: boolean;
  required: boolean;
  managed_installed: boolean;
  installing: boolean;
  source?: 'managed' | 'environment' | 'system';
  detected_version?: string;
  target_version?: string;
  update_available: boolean;
  error?: string;
};

export type RuntimeComponentKind = RuntimeComponentStatus['kind'];

export type ToolProcessStatus = 'running' | 'completed' | 'failed' | 'stopped';

export type ToolProcessInfo = {
  process_id: string;
  agent_id: string;
  conversation_id: string;
  tool_call_id: string;
  command: string;
  cwd?: string;
  status: ToolProcessStatus;
  stdout: string;
  stderr: string;
  exit_code?: number;
  started_at: string;
  finished_at?: string;
  duration_ms: number;
  runtime: 'local_shell' | 'full_access';
  tty: boolean;
  output_cursor: number;
};

export type PickDirectoryResponse = {
  path: string | null;
};

export function readToolDefaults(): Promise<AgentToolsConfig> {
  return request<AgentToolsConfig>('/api/tools/defaults');
}

export function updateToolDefaults(config: AgentToolsConfig): Promise<AgentToolsConfig> {
  return request<AgentToolsConfig>('/api/tools/defaults', {
    method: 'PUT',
    body: JSON.stringify(config),
  });
}

export function listIpProxies(): Promise<IpProxy[]> {
  return request<IpProxy[]>('/api/ip-proxies');
}

export function createIpProxy(payload: IpProxyPayload): Promise<IpProxy> {
  return request<IpProxy>('/api/ip-proxies', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateIpProxy(id: string, payload: Partial<IpProxyPayload>): Promise<IpProxy> {
  return request<IpProxy>(`/api/ip-proxies/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteIpProxy(id: string): Promise<void> {
  return request<void>(`/api/ip-proxies/${id}`, { method: 'DELETE' });
}

export function importIpProxies(
  entries: IpProxyImportEntry[],
  enabled: boolean,
): Promise<IpProxyImportResult> {
  return request<IpProxyImportResult>('/api/ip-proxies/import', {
    method: 'POST',
    body: JSON.stringify({ entries, enabled }),
  });
}

export function testIpProxy(id: string): Promise<IpProxy> {
  return request<IpProxy>(`/api/ip-proxies/${id}/test`, { method: 'POST' }, undefined, 30000);
}

export function generateConversationTitle(
  conversationId: string,
  content: string,
  locale?: string,
): Promise<Conversation> {
  return request<Conversation>(`/api/conversations/${conversationId}/title`, {
    method: 'POST',
    body: JSON.stringify({ content, locale }),
  }, undefined, CONVERSATION_TITLE_REQUEST_TIMEOUT_MS);
}

export function checkWebSearchKeys(
  sourceId: string,
  keyIndices: number[] = [],
  signal?: AbortSignal,
): Promise<WebSearchKeyCheckResponse> {
  return request<WebSearchKeyCheckResponse>(
    '/api/tools/web-search-keys/check',
    {
      method: 'POST',
      body: JSON.stringify({ source_id: sourceId, key_indices: keyIndices }),
      signal,
    },
    undefined,
    180000,
  );
}

export function importWebSearchKeys(
  sourceId: string,
  apiKeys: string[],
): Promise<WebSearchKeyImportResponse> {
  return request<WebSearchKeyImportResponse>(
    '/api/tools/web-search-keys/import',
    {
      method: 'POST',
      body: JSON.stringify({ source_id: sourceId, api_keys: apiKeys }),
    },
  );
}

export function deleteWebSearchKey(
  sourceId: string,
  keyIndex: number,
  keyHint?: string,
): Promise<WebSearchKeyDeleteResponse> {
  return request<WebSearchKeyDeleteResponse>(
    '/api/tools/web-search-keys/delete',
    {
      method: 'POST',
      body: JSON.stringify({ source_id: sourceId, key_index: keyIndex, key_hint: keyHint }),
    },
  );
}

export function readAgentToolsConfig(agentId: string): Promise<AgentToolsSettings> {
  return request<AgentToolsSettings>(`/api/agents/${agentId}/tools-config`);
}

export function updateAgentToolsConfig(
  agentId: string,
  config: AgentToolsOverrides,
): Promise<AgentToolsSettings> {
  return request<AgentToolsSettings>(`/api/agents/${agentId}/tools-config`, {
    method: 'PUT',
    body: JSON.stringify(config),
  });
}

export function readEnvironmentRuntime(): Promise<EnvironmentRuntimeStatus> {
  return request<EnvironmentRuntimeStatus>('/api/tools/environment-runtime');
}

export function pickDirectory(initialDirectory?: string): Promise<PickDirectoryResponse> {
  return request<PickDirectoryResponse>(
    '/api/tools/pick-directory',
    {
      method: 'POST',
      body: JSON.stringify({ initial_directory: initialDirectory || null }),
    },
    undefined,
    15 * 60 * 1000,
  );
}


export async function installEnvironmentComponent(
  component: RuntimeComponentKind,
): Promise<EnvironmentRuntimeStatus> {
  const abortHandle = createAbortSignal(15 * 60 * 1000);
  try {
    const response = await fetch(`/api/tools/environment-runtime/${component}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...apiAuthorizationHeaders() },
      signal: abortHandle.signal,
    });
    if (!response.ok) {
      throw new Error(await readError(response));
    }
    return response.json() as Promise<EnvironmentRuntimeStatus>;
  } finally {
    abortHandle.dispose();
  }
}

export function refreshEnvironmentComponent(
  component: RuntimeComponentKind,
): Promise<EnvironmentRuntimeStatus> {
  return request<EnvironmentRuntimeStatus>(`/api/tools/environment-runtime/${component}/refresh`, {
    method: 'POST',
  });
}


export type ApprovalDecision = 'approve' | 'reject' | 'always_approve';

export function approveToolCall(
  approvalId: string,
  agentId: string,
  decision: ApprovalDecision,
): Promise<{ status: string; message?: string }> {
  return request<{ status: string; message?: string }>('/api/tools/approve', {
    method: 'POST',
    body: JSON.stringify({ approval_id: approvalId, agent_id: agentId, decision }),
  });
}


export function listToolProcesses(agentId?: string): Promise<ToolProcessInfo[]> {
  const query = agentId ? `?agent_id=${encodeURIComponent(agentId)}` : '';
  return request<ToolProcessInfo[]>(`/api/tools/processes${query}`);
}

export function stopToolProcess(processId: string, agentId: string): Promise<ToolProcessInfo> {
  return request<ToolProcessInfo>(`/api/tools/processes/${encodeURIComponent(processId)}?agent_id=${encodeURIComponent(agentId)}`, {
    method: 'DELETE',
  });
}

export function writeTerminalSession(
  processId: string,
  agentId: string,
  data: string,
  closeStdin = false,
): Promise<ToolProcessInfo> {
  return request<ToolProcessInfo>(
    `/api/terminal/sessions/${encodeURIComponent(processId)}/input?agent_id=${encodeURIComponent(agentId)}`,
    {
      method: 'POST',
      body: JSON.stringify({ data, close_stdin: closeStdin }),
    },
  );
}

export function interruptTerminalSession(processId: string, agentId: string): Promise<ToolProcessInfo> {
  return request<ToolProcessInfo>(
    `/api/terminal/sessions/${encodeURIComponent(processId)}/interrupt?agent_id=${encodeURIComponent(agentId)}`,
    { method: 'POST' },
  );
}

export function resizeTerminalSession(
  processId: string,
  agentId: string,
  rows: number,
  cols: number,
): Promise<ToolProcessInfo> {
  return request<ToolProcessInfo>(
    `/api/terminal/sessions/${encodeURIComponent(processId)}/resize?agent_id=${encodeURIComponent(agentId)}`,
    {
      method: 'POST',
      body: JSON.stringify({ rows, cols }),
    },
  );
}
export function readConversationMessages(
  conversationId: string,
  before?: string,
  limit = 80,
): Promise<ConversationMessagePage> {
  const query = new URLSearchParams({ limit: String(limit) });
  if (before) query.set('before', before);
  return request(
    `/api/conversations/${conversationId}/messages/page?${query}`,
    undefined,
    conversationMessagePageRuntimeSchema,
  );
}

export function createMessageRun(
  conversationId: string,
  content: string,
  streamOutput: boolean,
  attachments: ImageAttachment[],
  thinkingLevel: ThinkingLevel,
  generationMode: GenerationMode = 'chat',
  restartFromMessageId?: string,
  signal?: AbortSignal,
): Promise<AgentRun> {
  return request(`/api/conversations/${conversationId}/runs`, {
    method: 'POST',
    body: JSON.stringify({
      content,
      attachments,
      thinking_level: thinkingLevel,
      stream: streamOutput,
      generation_mode: generationMode,
      ...(restartFromMessageId ? { restart_from_message_id: restartFromMessageId } : {}),
    }),
    signal,
  }, agentRunRuntimeSchema);
}

export function readConversationUsage(conversationId: string): Promise<ConversationUsage> {
  return request<ConversationUsage>(`/api/conversations/${conversationId}/usage`);
}

export function readActiveConversationRun(conversationId: string): Promise<AgentRun | null> {
  return request(
    `/api/conversations/${conversationId}/run`,
    undefined,
    nullableAgentRunRuntimeSchema,
  );
}

export function listActiveAgentRuns(agentId: string): Promise<AgentRun[]> {
  return request(`/api/agents/${agentId}/runs`, undefined, agentRunListRuntimeSchema);
}

export function cancelMessageRun(runId: string): Promise<AgentRun> {
  return request(`/api/runs/${runId}`, { method: 'DELETE' }, agentRunRuntimeSchema);
}

export async function streamRunEvents(
  runId: string,
  after: number,
  onEvent: (event: StreamEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const response = await fetch(`/api/runs/${runId}/events?after=${after}`, {
    signal,
    headers: apiAuthorizationHeaders(),
  });
  if (!response.ok || !response.body) {
    throw new Error(await readError(response));
  }
  await consumeEventStream(response.body, onEvent);
}

async function consumeEventStream(
  body: ReadableStream<Uint8Array>,
  onEvent: (event: StreamEvent) => void,
): Promise<void> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  const state = { sawDone: false, sawError: false, sawCancelled: false };

  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }

    buffer += decoder.decode(value, { stream: true });
    let boundary = findEventBoundary(buffer);
    while (boundary) {
      const rawEvent = buffer.slice(0, boundary.index);
      buffer = buffer.slice(boundary.index + boundary.length);
      dispatchStreamEvent(rawEvent, onEvent, state);
      boundary = findEventBoundary(buffer);
    }
  }

  buffer += decoder.decode();
  if (buffer.trim()) {
    dispatchStreamEvent(buffer, onEvent, state);
  }

  if (!state.sawDone && !state.sawError && !state.sawCancelled) {
    throw new Error('Stream ended before completion');
  }
}

function dispatchStreamEvent(
  rawEvent: string,
  onEvent: (event: StreamEvent) => void,
  state: { sawDone: boolean; sawError: boolean; sawCancelled: boolean },
) {
  const lines = rawEvent.split(/\r?\n/);
  const event = lines.find((line) => line.startsWith('event: '))?.slice(7);
  const rawSequence = lines.find((line) => line.startsWith('id: '))?.slice(4);
  const parsedSequence = rawSequence ? Number(rawSequence) : undefined;
  const sequence = Number.isFinite(parsedSequence) ? parsedSequence : undefined;
  const data = lines
    .filter((line) => line.startsWith('data: '))
    .map((line) => line.slice(6))
    .join('\n');

  if (!event) {
    return;
  }

  if (event === 'done') {
    state.sawDone = true;
    onEvent({ event: 'done', data, sequence });
    return;
  }

  if (event === 'cancelled') {
    state.sawCancelled = true;
    onEvent({ event: 'cancelled', data, sequence });
    return;
  }

  if (event === 'error') {
    state.sawError = true;
    onEvent({ event: 'error', data, sequence });
    return;
  }

  if (event === 'delta') {
    try {
      onEvent({ event: 'delta', data: decodeTextStreamData(data), sequence });
    } catch {
      state.sawError = true;
      onEvent({ event: 'error', data: 'Invalid delta stream event', sequence });
    }
    return;
  }

  if (event === 'reasoning_delta') {
    try {
      onEvent({ event: 'reasoning_delta', data: decodeTextStreamData(data), sequence });
    } catch {
      state.sawError = true;
      onEvent({ event: 'error', data: 'Invalid reasoning_delta stream event', sequence });
    }
    return;
  }

  if (
    event === 'user'
    || event === 'assistant'
    || event === 'phase'
    || event === 'retry'
    || event === 'tool_call_started'
    || event === 'tool_call_args_delta'
    || event === 'tool_call'
    || event === 'tool_result'
    || event === 'approval_required'
  ) {
    try {
      onEvent(parseJsonStreamEvent(event, data, sequence));
    } catch {
      state.sawError = true;
      onEvent({ event: 'error', data: `Invalid ${event} stream event`, sequence });
    }
  }
}

/** Text deltas are always `{"text": "..."}` JSON; anything else is malformed. */
function decodeTextStreamData(data: string): string {
  const decoded: unknown = JSON.parse(data);
  if (
    decoded
    && typeof decoded === 'object'
    && typeof (decoded as { text?: unknown }).text === 'string'
  ) {
    return (decoded as { text: string }).text;
  }
  throw new Error('Malformed text delta payload');
}

function findEventBoundary(buffer: string): { index: number; length: number } | null {
  const lf = buffer.indexOf('\n\n');
  const crlf = buffer.indexOf('\r\n\r\n');

  if (lf < 0 && crlf < 0) return null;
  if (lf < 0) return { index: crlf, length: 4 };
  if (crlf < 0) return { index: lf, length: 2 };
  return crlf < lf ? { index: crlf, length: 4 } : { index: lf, length: 2 };
}
