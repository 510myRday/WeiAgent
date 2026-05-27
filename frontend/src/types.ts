export type Agent = {
  id: string;
  name: string;
  system_prompt: string;
  model: string;
  created_at: string;
  updated_at: string;
};

export type ChatRole = 'user' | 'assistant';

export type ChatMessage = {
  id?: string;
  role: ChatRole;
  content: string;
  created_at: string;
};

export type UiMessage = ChatMessage & { clientId: string };

export type CreateAgentPayload = {
  name: string;
  system_prompt: string;
  model: string;
};

export type UpdateAgentPayload = Partial<CreateAgentPayload>;

export type SendMessageResponse = {
  user: ChatMessage;
  assistant: ChatMessage;
};

export type ModelListResponse = {
  models: string[];
};

export type StreamEvent =
  | { event: 'user'; data: ChatMessage }
  | { event: 'assistant'; data: ChatMessage }
  | { event: 'delta'; data: string }
  | { event: 'done'; data: string }
  | { event: 'error'; data: string };
