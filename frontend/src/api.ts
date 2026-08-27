import type {
  Agent,
  AgentProfile,
  AgentRun,
  AgentWorkspace,
  AppSettings,
  Conversation,
  ConversationHeartbeat,
  ConversationHeartbeatExecution,
  ConversationUsage,
  ConversationMessagePage,
  CreateAgentPayload,
  CreateConversationPayload,
  FetchProviderModelsPayload,
  LlmLogDetail,
  LlmLogListPage,
  ModelProvider,
  ModelProviderPayload,
  ScheduledTask,
  ScheduledTaskExecution,
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
import type {
  AgentToolsConfig,
  AgentToolsOverrides,
  AgentToolsSettings,
  ApprovalDecision,
  EnvironmentRuntimeStatus,
  ListLlmLogsParams,
  McpAgentOverrides,
  McpAgentSettings,
  McpAuthState,
  McpServer,
  McpServerDraft,
  McpTransport,
  PickDirectoryResponse,
  RuntimeComponentKind,
  SkillChangeFeed,
  SkillContent,
  SkillImportTask,
  SkillOverrides,
  SkillSettings,
  TaskPlan,
  ToolProcessInfo,
  WebSearchKeyCheckResponse,
  WebSearchKeyDeleteResponse,
  WebSearchKeyImportResponse,
} from './toolTypes';
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
  if (params.isCompaction != null) search.set('is_compaction', String(params.isCompaction));
  const query = search.toString();
  return request<LlmLogListPage>(query ? `/api/llm-logs?${query}` : '/api/llm-logs');
}

export function getLlmLog(logId: string): Promise<LlmLogDetail> {
  return request<LlmLogDetail>(`/api/llm-logs/${logId}`);
}

export {
  MAX_WEB_SEARCH_KEYS_PER_SOURCE,
} from './toolTypes';
export type {
  ListLlmLogsParams,
  WebSearchProviderKind,
  WebSearchKeySelection,
  WebSearchSourceRole,
  WebSearchSourceConfig,
  WebContentExtractionConfig,
  WebSearchToolConfig,
  EvidenceStorageConfig,
  WebSearchKeyStatusRecord,
  WebSearchKeyStatus,
  WebSearchKeyCheckResult,
  WebSearchKeyCheckResponse,
  WebSearchKeyImportResponse,
  WebSearchKeyDeleteResponse,
  AgentToolsConfig,
  AgentToolsOverrides,
  AgentToolsSettings,
  WebSearchSourceOverrides,
  McpTransport,
  McpIdentityHeader,
  McpOAuthConfig,
  McpTool,
  McpAuthState,
  McpServer,
  McpDiagnostics,
  McpServerDraft,
  McpServerOverride,
  McpToolOverride,
  McpAgentOverrides,
  McpAgentSettings,
  SkillFile,
  SkillDiagnostic,
  SkillConflict,
  SkillManifest,
  SkillGroupState,
  SkillOverride,
  SkillGroupOverride,
  SkillOverrides,
  SkillSettings,
  SkillImportTask,
  SkillContent,
  SkillChangeEvent,
  SkillChangeFeed,
  TaskPlan,
  EnvironmentRuntimeStatus,
  RuntimeComponentStatus,
  RuntimeComponentKind,
  ToolProcessStatus,
  ToolProcessInfo,
  PickDirectoryResponse,
  ApprovalDecision,
} from './toolTypes';



export function readTaskPlans(conversationId: string): Promise<TaskPlan[]> {
  return request<TaskPlan[]>(`/api/conversations/${conversationId}/task-plans`);
}







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
  apiKeyLabels: string[] = [],
): Promise<WebSearchKeyImportResponse> {
  return request<WebSearchKeyImportResponse>(
    '/api/tools/web-search-keys/import',
    {
      method: 'POST',
      body: JSON.stringify({ source_id: sourceId, api_keys: apiKeys, api_key_labels: apiKeyLabels }),
    },
  );
}

export function deleteWebSearchKey(
  sourceId: string,
  keyIndex: number,
  keyFingerprint?: string,
  keyHint?: string,
): Promise<WebSearchKeyDeleteResponse> {
  return request<WebSearchKeyDeleteResponse>(
    '/api/tools/web-search-keys/delete',
    {
      method: 'POST',
      body: JSON.stringify({
        source_id: sourceId,
        key_index: keyIndex,
        key_fingerprint: keyFingerprint,
        key_hint: keyHint,
      }),
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

export function listMcpServers(): Promise<McpServer[]> {
  return request<McpServer[]>('/api/tools/mcp');
}

export function getMcpServerSecrets(id: string): Promise<McpTransport> {
  return request<McpTransport>(`/api/tools/mcp/${encodeURIComponent(id)}/secrets`);
}

export function createMcpServer(draft: McpServerDraft): Promise<McpServer> {
  return request<McpServer>('/api/tools/mcp', { method: 'POST', body: JSON.stringify(draft) });
}

export function updateMcpServer(id: string, draft: McpServerDraft): Promise<McpServer> {
  return request<McpServer>(`/api/tools/mcp/${encodeURIComponent(id)}`, { method: 'PUT', body: JSON.stringify(draft) });
}

export function updateMcpServerEnabled(id: string, enabled: boolean): Promise<McpServer> {
  return request<McpServer>(`/api/tools/mcp/${encodeURIComponent(id)}/enabled`, {
    method: 'PUT',
    body: JSON.stringify({ enabled }),
  });
}

export function deleteMcpServer(id: string): Promise<void> {
  return request<void>(`/api/tools/mcp/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export function refreshMcpServer(id: string): Promise<McpServer> {
  return request<McpServer>(`/api/tools/mcp/${encodeURIComponent(id)}/refresh`, { method: 'POST' }, undefined, 180000);
}

export function testMcpServer(id: string): Promise<McpServer> {
  return request<McpServer>(`/api/tools/mcp/${encodeURIComponent(id)}/test`, { method: 'POST' }, undefined, 180000);
}

export function getMcpAuthState(id: string): Promise<McpAuthState> {
  return request<McpAuthState>(`/api/tools/mcp/${encodeURIComponent(id)}/auth`);
}

export function reauthorizeMcpServer(id: string): Promise<McpAuthState> {
  return request<McpAuthState>(`/api/tools/mcp/${encodeURIComponent(id)}/auth/reauthorize`, { method: 'POST' }, undefined, 130000);
}

export function clearMcpAuth(id: string): Promise<McpAuthState> {
  return request<McpAuthState>(`/api/tools/mcp/${encodeURIComponent(id)}/auth/clear`, { method: 'POST' });
}

export function updateMcpToolEnabled(serverId: string, toolName: string, enabled: boolean): Promise<McpServer> {
  return request<McpServer>(`/api/tools/mcp/${encodeURIComponent(serverId)}/tools/${encodeURIComponent(toolName)}`, {
    method: 'PUT',
    body: JSON.stringify({ enabled }),
  });
}

export function updateMcpToolsEnabled(serverId: string, toolNames: string[], enabled: boolean): Promise<McpServer> {
  return request<McpServer>(`/api/tools/mcp/${encodeURIComponent(serverId)}/tools`, {
    method: 'PUT',
    body: JSON.stringify({ tool_names: toolNames, enabled }),
  });
}

export function readAgentMcpConfig(agentId: string): Promise<McpAgentSettings> {
  return request<McpAgentSettings>(`/api/agents/${encodeURIComponent(agentId)}/mcp-config`);
}

export function listSkills(): Promise<SkillSettings> {
  return request<SkillSettings>('/api/skills');
}

export function readSkillChanges(since: number): Promise<SkillChangeFeed> {
  return request<SkillChangeFeed>(`/api/skills/changes?since=${since}`);
}

export function importSkill(source: string, proxyId?: string, kind?: 'local' | 'network', groupId?: string): Promise<SkillImportTask> {
  return request<SkillImportTask>('/api/skills/import', {
    method: 'POST',
    body: JSON.stringify({
      source,
      proxy_id: proxyId,
      kind,
      group_id: groupId,
    }),
  });
}

export function listSkillImportTasks(): Promise<SkillImportTask[]> {
  return request<SkillImportTask[]>('/api/skills/import-tasks');
}

export function readSkillContent(name: string, path?: string): Promise<SkillContent> {
  const query = path ? `?path=${encodeURIComponent(path)}` : '';
  return request<SkillContent>(`/api/skills/${encodeURIComponent(name)}/content${query}`);
}

export function updateSkill(name: string): Promise<SkillImportTask> {
  return request<SkillImportTask>(`/api/skills/${encodeURIComponent(name)}/update`, {
    method: 'POST',
  });
}

export function updateSkillEnabled(name: string, enabled: boolean): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/${encodeURIComponent(name)}/enabled`, {
    method: 'PUT',
    body: JSON.stringify({ enabled }),
  });
}

export function listScheduledTasks(): Promise<ScheduledTask[]> {
  return request<ScheduledTask[]>('/api/cron');
}

export function createScheduledTask(payload: {
  agent_id: string;
  source_conversation_id?: string | null;
  schedule: string;
  prompt: string;
  timezone_offset?: string | null;
}): Promise<ScheduledTask> {
  return request<ScheduledTask>('/api/cron', { method: 'POST', body: JSON.stringify(payload) });
}

export function updateScheduledTask(taskId: string, payload: {
  agent_id?: string;
  schedule?: string;
  prompt?: string;
  timezone_offset?: string;
}): Promise<ScheduledTask> {
  return request<ScheduledTask>(`/api/cron/${encodeURIComponent(taskId)}`, { method: 'PATCH', body: JSON.stringify(payload) });
}

export function setScheduledTaskEnabled(taskId: string, enabled: boolean): Promise<ScheduledTask> {
  return request<ScheduledTask>(`/api/cron/${encodeURIComponent(taskId)}/${enabled ? 'resume' : 'pause'}`, { method: 'POST' });
}

export function runScheduledTask(taskId: string): Promise<ScheduledTask> {
  return request<ScheduledTask>(`/api/cron/${encodeURIComponent(taskId)}/run`, { method: 'POST' });
}

export function deleteScheduledTask(taskId: string): Promise<void> {
  return request<void>(`/api/cron/${encodeURIComponent(taskId)}`, { method: 'DELETE' });
}

export function listScheduledTaskExecutions(taskId: string): Promise<ScheduledTaskExecution[]> {
  return request<ScheduledTaskExecution[]>(`/api/cron/${encodeURIComponent(taskId)}/executions`);
}

export function listHeartbeats(): Promise<ConversationHeartbeat[]> {
  return request<ConversationHeartbeat[]>('/api/heartbeats');
}

export function listHeartbeatExecutions(conversationId: string): Promise<ConversationHeartbeatExecution[]> {
  return request<ConversationHeartbeatExecution[]>(`/api/conversations/${encodeURIComponent(conversationId)}/heartbeat/executions`);
}

export function setHeartbeat(conversationId: string, payload: { conversation_id?: string; interval_seconds: number; prompt: string }): Promise<ConversationHeartbeat> {
  return request<ConversationHeartbeat>(`/api/conversations/${encodeURIComponent(conversationId)}/heartbeat`, { method: 'PUT', body: JSON.stringify(payload) });
}

export function setHeartbeatEnabled(conversationId: string, enabled: boolean): Promise<ConversationHeartbeat> {
  return request<ConversationHeartbeat>(`/api/conversations/${encodeURIComponent(conversationId)}/heartbeat/${enabled ? 'resume' : 'pause'}`, { method: 'POST' });
}

export function runHeartbeat(conversationId: string): Promise<ConversationHeartbeat> {
  return request<ConversationHeartbeat>(`/api/conversations/${encodeURIComponent(conversationId)}/heartbeat/run`, { method: 'POST' });
}

export function deleteHeartbeat(conversationId: string): Promise<void> {
  return request<void>(`/api/conversations/${encodeURIComponent(conversationId)}/heartbeat`, { method: 'DELETE' });
}

export function createSkillGroup(name: string): Promise<SkillSettings> {
  return request<SkillSettings>('/api/skills/groups', {
    method: 'POST',
    body: JSON.stringify({ name }),
  });
}

export function renameSkillGroup(groupId: string, name: string): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/groups/${encodeURIComponent(groupId)}`, {
    method: 'PATCH',
    body: JSON.stringify({ name }),
  });
}

export function deleteSkillGroup(groupId: string): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/groups/${encodeURIComponent(groupId)}`, { method: 'DELETE' });
}

export function reorderSkillGroups(ids: string[]): Promise<SkillSettings> {
  return request<SkillSettings>('/api/skills/groups/order', {
    method: 'PUT',
    body: JSON.stringify({ ids }),
  });
}

export function assignSkillGroup(name: string, groupId: string | null): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/${encodeURIComponent(name)}/group`, {
    method: 'PUT',
    body: JSON.stringify({ group_id: groupId }),
  });
}

export function reorderSkillGroupItems(groupId: string, skillNames: string[]): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/groups/${encodeURIComponent(groupId)}/skills/order`, {
    method: 'PUT',
    body: JSON.stringify({ skill_names: skillNames }),
  });
}

export function updateSkillSettings(
  name: string,
  iconUrl: string | null,
  sourceUrl: string | null,
  iconProxyUrl: string | null,
  updateProxyUrl: string | null,
): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify({
      icon_url: iconUrl,
      source_url: sourceUrl,
      icon_proxy_url: iconProxyUrl,
      update_proxy_url: updateProxyUrl,
    }),
  });
}

export function deleteSkill(name: string): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

export function updateSkillGroupEnabled(groupId: string, enabled: boolean): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/skills/groups/${encodeURIComponent(groupId)}/enabled`, {
    method: 'PUT',
    body: JSON.stringify({ enabled }),
  });
}

export function cleanupSkillOrphans(): Promise<SkillSettings> {
  return request<SkillSettings>('/api/skills/cleanup-orphans', { method: 'POST' });
}

export function readAgentSkillsConfig(agentId: string): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/agents/${encodeURIComponent(agentId)}/skills-config`);
}

export function updateAgentSkillsConfig(agentId: string, overrides: SkillOverrides): Promise<SkillSettings> {
  return request<SkillSettings>(`/api/agents/${encodeURIComponent(agentId)}/skills-config`, {
    method: 'PUT',
    body: JSON.stringify(overrides),
  });
}

export function updateAgentMcpConfig(agentId: string, overrides: McpAgentOverrides): Promise<McpAgentSettings> {
  return request<McpAgentSettings>(`/api/agents/${encodeURIComponent(agentId)}/mcp-config`, {
    method: 'PUT',
    body: JSON.stringify(overrides),
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

export function steerMessageRun(runId: string, content: string): Promise<void> {
  return request(`/api/runs/${runId}/steer`, {
    method: 'POST',
    body: JSON.stringify({ content }),
  });
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
    || event === 'usage'
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

