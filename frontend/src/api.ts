import type {
  Agent,
  ChatMessage,
  CreateAgentPayload,
  ModelListResponse,
  SendMessageResponse,
  StreamEvent,
  UpdateAgentPayload,
} from './types';

const API_BASE_URL = __WEIAGENT_API_BASE_URL__;

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE_URL}${path}`, {
    ...options,
    headers: {
      'content-type': 'application/json',
      ...options?.headers,
    },
  });

  if (!response.ok) {
    throw new Error(await readError(response));
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return (await response.json()) as T;
}

async function readError(response: Response): Promise<string> {
  try {
    const body = await response.json();
    return body.error ?? response.statusText;
  } catch {
    return response.statusText;
  }
}

export function listAgents(): Promise<Agent[]> {
  return request<Agent[]>('/api/agents');
}

export async function listModels(): Promise<string[]> {
  const response = await request<ModelListResponse>('/api/models');
  return response.models;
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

export function readChat(agentId: string): Promise<ChatMessage[]> {
  return request<ChatMessage[]>(`/api/chats/${agentId}`);
}

export function sendMessage(agentId: string, content: string): Promise<SendMessageResponse> {
  return request<SendMessageResponse>(`/api/chats/${agentId}/messages`, {
    method: 'POST',
    body: JSON.stringify({ content }),
  });
}

export async function streamMessage(
  agentId: string,
  content: string,
  onEvent: (event: StreamEvent) => void,
): Promise<void> {
  const response = await fetch(`${API_BASE_URL}/api/chats/${agentId}/messages/stream`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content }),
  });

  if (!response.ok || !response.body) {
    throw new Error(await readError(response));
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }

    buffer += decoder.decode(value, { stream: true });
    let boundary = buffer.indexOf('\n\n');
    while (boundary >= 0) {
      const rawEvent = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      dispatchStreamEvent(rawEvent, onEvent);
      boundary = buffer.indexOf('\n\n');
    }
  }

  if (buffer.trim()) {
    dispatchStreamEvent(buffer, onEvent);
  }
}

function dispatchStreamEvent(rawEvent: string, onEvent: (event: StreamEvent) => void) {
  const lines = rawEvent.split(/\r?\n/);
  const event = lines.find((line) => line.startsWith('event: '))?.slice(7);
  const data = lines
    .filter((line) => line.startsWith('data: '))
    .map((line) => line.slice(6))
    .join('\n');

  if (!event) {
    return;
  }

  if (event === 'user' || event === 'assistant') {
    onEvent({ event, data: JSON.parse(data) as ChatMessage });
    return;
  }

  if (event === 'delta' || event === 'done' || event === 'error') {
    onEvent({ event, data });
  }
}
