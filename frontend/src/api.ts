import type {
  Agent,
  AgentWorkspace,
  AppSettings,
  ChatMessage,
  Conversation,
  CreateAgentPayload,
  CreateConversationPayload,
  FetchProviderModelsPayload,
  LlmLogDetail,
  LlmLogEntry,
  ModelProvider,
  ModelProviderPayload,
  SendMessageResponse,
  StreamEvent,
  UpdateAgentWorkspacePayload,
  UpdateAgentPayload,
} from './types';

const DEFAULT_TIMEOUT_MS = 30000;

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const timeoutSignal = AbortSignal.timeout(DEFAULT_TIMEOUT_MS);
  const combinedSignal = options?.signal
    ? AbortSignal.any([options.signal, timeoutSignal])
    : timeoutSignal;

  const response = await fetch(path, {
    ...options,
    signal: combinedSignal,
    headers: {
      'content-type': 'application/json',
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

  return JSON.parse(text) as T;
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

export function updateAgentWorkspace(
  agentId: string,
  payload: UpdateAgentWorkspacePayload,
): Promise<AgentWorkspace> {
  return request<AgentWorkspace>(`/api/agents/${agentId}/workspace`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
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

export function deleteConversation(conversationId: string): Promise<void> {
  return request<void>(`/api/conversations/${conversationId}`, {
    method: 'DELETE',
  });
}

export function listLlmLogs(): Promise<LlmLogEntry[]> {
  return request<LlmLogEntry[]>('/api/llm-logs');
}

export function getLlmLog(logId: string): Promise<LlmLogDetail> {
  return request<LlmLogDetail>(`/api/llm-logs/${logId}`);
}

export function deleteLlmLog(logId: string): Promise<void> {
  return request<void>(`/api/llm-logs/${logId}`, {
    method: 'DELETE',
  });
}


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
};

export type AgentToolsSettings = {
  defaults: AgentToolsConfig;
  overrides: AgentToolsOverrides;
  effective: AgentToolsConfig;
};

export type CodexRuntimeStatus = {
  supported: boolean;
  available: boolean;
  managed_installed: boolean;
  installing: boolean;
  target_version: string;
  platform?: string;
  source?: 'managed' | 'environment' | 'system';
  detected_version?: string;
  executable?: string;
  error?: string;
};

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
  runtime: 'codex_sandbox' | 'codex_full_access';
  tty: boolean;
  output_cursor: number;
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

export function readCodexRuntime(): Promise<CodexRuntimeStatus> {
  return request<CodexRuntimeStatus>('/api/tools/codex-runtime');
}

export function refreshCodexRuntime(): Promise<CodexRuntimeStatus> {
  return request<CodexRuntimeStatus>('/api/tools/codex-runtime/refresh', {
    method: 'POST',
  });
}

export async function installCodexRuntime(): Promise<CodexRuntimeStatus> {
  const response = await fetch('/api/tools/codex-runtime', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    signal: AbortSignal.timeout(15 * 60 * 1000),
  });
  if (!response.ok) {
    throw new Error(await readError(response));
  }
  return response.json() as Promise<CodexRuntimeStatus>;
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

export function getToolProcess(processId: string, agentId: string): Promise<ToolProcessInfo> {
  return request<ToolProcessInfo>(`/api/tools/processes/${encodeURIComponent(processId)}?agent_id=${encodeURIComponent(agentId)}`);
}

export function stopToolProcess(processId: string, agentId: string): Promise<ToolProcessInfo> {
  return request<ToolProcessInfo>(`/api/tools/processes/${encodeURIComponent(processId)}?agent_id=${encodeURIComponent(agentId)}`, {
    method: 'DELETE',
  });
}
export function readConversationMessages(conversationId: string): Promise<ChatMessage[]> {
  return request<ChatMessage[]>(`/api/conversations/${conversationId}/messages`);
}

export function sendMessage(
  conversationId: string,
  content: string,
  timeoutSeconds: number,
  signal?: AbortSignal,
): Promise<SendMessageResponse> {
  const timeoutSignal = AbortSignal.timeout(Math.max(1, timeoutSeconds) * 1000);
  const combinedSignal = signal ? AbortSignal.any([signal, timeoutSignal]) : timeoutSignal;
  return request<SendMessageResponse>(`/api/conversations/${conversationId}/messages`, {
    method: 'POST',
    body: JSON.stringify({ content }),
    signal: combinedSignal,
  });
}

export async function streamMessage(
  conversationId: string,
  content: string,
  onEvent: (event: StreamEvent) => void,
  timeoutSeconds: number,
  streamOutput: boolean,
  signal?: AbortSignal,
): Promise<void> {
  const timeoutSignal = AbortSignal.timeout(Math.max(1, timeoutSeconds, 3600) * 1000);
  const combinedSignal = signal ? AbortSignal.any([signal, timeoutSignal]) : timeoutSignal;
  const response = await fetch(`/api/conversations/${conversationId}/messages/stream`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content, stream: streamOutput }),
    signal: combinedSignal,
  });

  if (!response.ok || !response.body) {
    throw new Error(await readError(response));
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  const state = { sawDone: false, sawError: false };

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

  if (!state.sawDone && !state.sawError) {
    throw new Error('Stream ended before completion');
  }
}

function dispatchStreamEvent(
  rawEvent: string,
  onEvent: (event: StreamEvent) => void,
  state: { sawDone: boolean; sawError: boolean },
) {
  const lines = rawEvent.split(/\r?\n/);
  const event = lines.find((line) => line.startsWith('event: '))?.slice(7);
  const data = lines
    .filter((line) => line.startsWith('data: '))
    .map((line) => line.slice(6))
    .join('\n');

  if (!event) {
    return;
  }

  if (event === 'done') {
    state.sawDone = true;
    onEvent({ event: 'done', data });
    return;
  }

  if (event === 'error') {
    state.sawError = true;
    onEvent({ event: 'error', data });
    return;
  }

  if (event === 'user' || event === 'assistant') {
    try {
      onEvent({ event, data: JSON.parse(data) as ChatMessage });
    } catch {
      state.sawError = true;
      onEvent({ event: 'error', data: 'Failed to parse stream event' });
    }
    return;
  }

  if (event === 'delta') {
    onEvent({ event: 'delta', data });
    return;
  }

  if (event === 'phase' || event === 'tool_call' || event === 'tool_result' || event === 'approval_required') {
    try {
      onEvent({ event, data: JSON.parse(data) } as StreamEvent);
    } catch {
      state.sawError = true;
      onEvent({ event: 'error', data: 'Failed to parse stream event' });
    }
  }
}

function findEventBoundary(buffer: string): { index: number; length: number } | null {
  const lf = buffer.indexOf('\n\n');
  const crlf = buffer.indexOf('\r\n\r\n');

  if (lf < 0 && crlf < 0) return null;
  if (lf < 0) return { index: crlf, length: 4 };
  if (crlf < 0) return { index: lf, length: 2 };
  return crlf < lf ? { index: crlf, length: 4 } : { index: lf, length: 2 };
}
