export type AppSettings = {
  host: string;
  port: number;
  log_level: string;
};

export type ModelProvider = {
  id: string;
  name: string;
  base_url: string;
  has_api_key: boolean;
  masked_api_key: string;
  models: string[];
  created_at: string;
  updated_at: string;
};

export type ModelProviderPayload = {
  name: string;
  base_url: string;
  api_key: string;
  models: string[];
};

export type Agent = {
  id: string;
  name: string;
  system_prompt: string;
  model_provider_id: string;
  model_id: string;
  created_at: string;
  updated_at: string;
};

export type Conversation = {
  id: string;
  agent_id: string;
  title: string;
  created_at: string;
  updated_at: string;
};

export type ChatRole = 'user' | 'assistant';

export type ChatMessage = {
  id: string;
  agent_id: string;
  conversation_id: string;
  role: ChatRole;
  content: string;
  created_at: string;
};

export type UiMessage = ChatMessage & { clientId: string };

export type CreateAgentPayload = {
  name: string;
  system_prompt: string;
  model_provider_id: string;
  model_id: string;
};

export type UpdateAgentPayload = Partial<CreateAgentPayload>;

export type CreateConversationPayload = {
  title?: string;
};

export type SendMessageResponse = {
  user: ChatMessage;
  assistant: ChatMessage;
};

export type StreamEvent =
  | { event: 'user'; data: ChatMessage }
  | { event: 'assistant'; data: ChatMessage }
  | { event: 'delta'; data: string }
  | { event: 'done'; data: string }
  | { event: 'error'; data: string };

export type NavKey = 'chat' | 'agents' | 'history' | 'settings';
