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
  model_configs: ModelConfig[];
  extra_body: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type ModelConfig = {
  model_id: string;
  multimodal: boolean;
  thinking: boolean;
  input_price?: number;
  output_price?: number;
  cache_write_price?: number;
  cache_read_price?: number;
};

export type ProviderType =
  | 'openai_compatible'
  | 'openai_responses'
  | 'anthropic_messages'
  | 'google_gemini';

export type ModelProviderPayload = {
  provider_type: ProviderType;
  name: string;
  base_url: string;
  api_key: string;
  models: string[];
  model_configs: ModelConfig[];
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
  description: string;
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
  is_pinned: boolean;
  created_at: string;
  updated_at: string;
};

export type ChatRole = 'user' | 'assistant' | 'tool';
export type ThinkingLevel = 'off' | 'low' | 'medium' | 'high' | 'x_high';

export type ImageAttachment = {
  id: string;
  name: string;
  media_type: 'image/png' | 'image/jpeg' | 'image/webp' | 'image/gif';
} & (
  | { source_type: 'base64'; data: string }
  | { source_type: 'url'; url: string }
);

export type TokenUsage = {
  input_tokens: number;
  output_tokens: number;
  cache_write_tokens: number;
  cache_read_tokens: number;
};

export type ModelUsage = TokenUsage & {
  total_tokens: number;
  cost_usd?: number | null;
  provider_id: string;
  model_id: string;
};

export type ConversationUsage = TokenUsage & {
  total_tokens: number;
  cost_usd?: number | null;
};

export type ChatToolCall = {
  id: string;
  type: 'function';
  permission_mode?: 'request_approval' | 'full_access';
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
  reasoning_content?: string;
  status?: 'done' | 'streaming' | 'failed';
  created_at: string;
  tool_calls?: ChatToolCall[];
  tool_call_id?: string;
  attachments?: ImageAttachment[];
  usage?: ModelUsage;
};

export type ConversationMessagePage = {
  messages: ChatMessage[];
  next_cursor?: string | null;
  has_more: boolean;
  usage?: ConversationUsage;
};

export type UiMessage = ChatMessage & { clientId: string };

export type CreateAgentPayload = {
  name: string;
  description: string;
  model_provider_id: string;
  model_id: string;
  avatar_icon: AvatarIcon;
  avatar_color: string;
};

export type UpdateAgentPayload = Partial<CreateAgentPayload>;

export type UpdateAgentWorkspacePayload = Pick<AgentWorkspace, 'personality' | 'memory' | 'instructions'>;

export type UpdateAgentProfilePayload = {
  agent: UpdateAgentPayload;
  workspace: UpdateAgentWorkspacePayload;
};

export type AgentProfile = {
  agent: Agent;
  workspace: AgentWorkspace;
};

export type CreateConversationPayload = {
  title?: string;
};

export type UpdateConversationPayload = {
  is_pinned: boolean;
};

export type ToolCallStatus = 'running' | 'pending_approval' | 'completed' | 'blocked' | 'rejected' | 'error';
export type AgentRunPhase = 'idle' | 'waiting_for_model' | 'executing_tool' | 'waiting_for_approval';
export type AgentRunStatus = 'running' | 'completed' | 'failed' | 'cancelled';

export type AgentRun = {
  id: string;
  conversation_id: string;
  user_message_id?: string | null;
  status: AgentRunStatus;
  phase?: Exclude<AgentRunPhase, 'idle'> | null;
  latest_sequence: number;
  error?: string | null;
  created_at: string;
  finished_at?: string | null;
};

export type ToolResultData = {
  stdout?: string;
  stderr?: string;
  exit_code?: number;
  duration_ms?: number;
  process_id?: string;
  output_cursor?: number;
  message?: string;
  reason?: string;
  /** Actual runtime used: codex_sandbox / codex_full_access */
  runtime?: string;
};

export type ToolArguments = Record<string, unknown>;

export type ToolCallMessage = {
  type: 'tool_call';
  call_id: string;
  name: string;
  arguments: ToolArguments;
  permission_mode?: 'request_approval' | 'full_access';
  status: ToolCallStatus;
  result?: ToolResultData;
};

export type UiMessageItem = UiMessage | ToolCallMessage;

export type StreamEvent =
  | { event: 'user'; data: ChatMessage; sequence?: number }
  | { event: 'assistant'; data: ChatMessage; sequence?: number }
  | { event: 'delta'; data: string; sequence?: number }
  | { event: 'reasoning_delta'; data: string; sequence?: number }
  | { event: 'done'; data: string; sequence?: number }
  | { event: 'cancelled'; data: string; sequence?: number }
  | { event: 'error'; data: string; sequence?: number }
  | {
      event: 'phase';
      sequence?: number;
      data: {
        phase: Exclude<AgentRunPhase, 'idle'>;
        iteration: number;
        call_id?: string;
      };
    }
  | {
      event: 'tool_call';
      sequence?: number;
      data: {
        call_id: string;
        name: string;
        arguments: ToolArguments;
        permission_mode: 'request_approval' | 'full_access';
      };
    }
  | { event: 'tool_result'; data: { call_id: string; result: ToolResultData & { status: ToolCallStatus } }; sequence?: number }
  | {
      event: 'approval_required';
      sequence?: number;
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

export type NavKey = 'chat' | 'agents' | 'tools' | 'settings';

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
