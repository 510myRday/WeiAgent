export type AppSettings = {
  host: string;
  port: number;
  log_level: string;
  avatar_icon: AvatarIcon;
  avatar_color: string;
  language: 'zh' | 'en';
  request_timeout_seconds: number;
  request_retry_attempts: number;
  llm_log_enabled: boolean;
};

export type ModelProvider = {
  id: string;
  provider_type: ProviderType;
  name: string;
  base_url: string;
  has_api_key: boolean;
  masked_api_key: string;
  models: string[];
  extra_body: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type ProviderType = 'openai_compatible';

export type ModelProviderPayload = {
  provider_type: ProviderType;
  name: string;
  base_url: string;
  api_key: string;
  models: string[];
  extra_body?: Record<string, unknown>;
};

export type FetchProviderModelsPayload = {
  provider_type: ProviderType;
  base_url: string;
  api_key: string;
  extra_body?: Record<string, unknown>;
};

export type Agent = {
  id: string;
  name: string;
  model_provider_id: string;
  model_id: string;
  avatar_icon: AvatarIcon;
  avatar_color: string;
  is_builtin: boolean;
  created_at: string;
  updated_at: string;
};

export type AvatarIcon =
  | 'bot'
  | 'sparkles'
  | 'zap'
  | 'heart'
  | 'star'
  | 'smile'
  | 'crown'
  | 'brain'
  | 'compass';

export type AgentWorkspace = {
  personality: string;
  memory: string;
  instructions: string;
};

export type Conversation = {
  id: string;
  agent_id: string;
  title: string;
  message_count: number;
  created_at: string;
  updated_at: string;
};

export type ChatRole = 'user' | 'assistant' | 'tool';

export type ChatToolCall = {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
};

export type ChatMessage = {
  id: string;
  agent_id: string;
  conversation_id: string;
  role: ChatRole;
  content: string;
  status?: 'done' | 'streaming' | 'failed';
  created_at: string;
  tool_calls?: ChatToolCall[];
  tool_call_id?: string;
};

export type UiMessage = ChatMessage & { clientId: string };

export type CreateAgentPayload = {
  name: string;
  model_provider_id: string;
  model_id: string;
  avatar_icon: AvatarIcon;
  avatar_color: string;
};

export type UpdateAgentPayload = Partial<CreateAgentPayload>;

export type UpdateAgentWorkspacePayload = Pick<AgentWorkspace, 'personality' | 'memory' | 'instructions'>;

export type CreateConversationPayload = {
  title?: string;
};

export type SendMessageResponse = {
  user: ChatMessage;
  assistant: ChatMessage;
};

export type ToolCallStatus = 'running' | 'pending_approval' | 'completed' | 'blocked' | 'rejected' | 'error';

export type ToolResultData = {
  stdout?: string;
  stderr?: string;
  exit_code?: number;
  duration_ms?: number;
  process_id?: string;
  message?: string;
  reason?: string;
  /** Actual runtime used: os_sandbox / full_access */
  runtime?: string;
};

export type ToolCallMessage = {
  type: 'tool_call';
  call_id: string;
  name: string;
  arguments: Record<string, any>;
  status: ToolCallStatus;
  result?: ToolResultData;
};

export type UiMessageItem = UiMessage | ToolCallMessage;

export type StreamEvent =
  | { event: 'user'; data: ChatMessage }
  | { event: 'assistant'; data: ChatMessage }
  | { event: 'delta'; data: string }
  | { event: 'done'; data: string }
  | { event: 'error'; data: string }
  | { event: 'tool_call'; data: { call_id: string; name: string; arguments: Record<string, any> } }
  | { event: 'tool_result'; data: { call_id: string; result: ToolResultData & { status: ToolCallStatus } } }
  | {
      event: 'approval_required';
      data: {
        call_id: string;
        agent_id: string;
        command: string;
        approval_id: string;
        cwd?: string;
        runtime: string;
        permission: string;
        justification?: string;
        background: boolean;
      };
    };

export type NavKey = 'chat' | 'agents' | 'groups' | 'tools' | 'settings';

export type MobileView = 'list' | 'main';

export type LlmLogEntry = {
  id: string;
  agent_name: string;
  model_id: string;
  provider_name: string;
  created_at: string;
};

export type LlmLogDetail = LlmLogEntry & {
  raw_request: string;
  raw_response?: string;
  raw_status?: number;
  error?: string;
};
