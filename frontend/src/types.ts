export type AppSettings = {
  host: string;
  port: number;
  log_level: string;
  avatar_icon: AvatarIcon;
  avatar_color: string;
  language: 'zh' | 'en';
  request_timeout_seconds: number;
  request_retry_attempts: number;
  /** Global auto-compact threshold shared by every model and agent. */
  context_compress_threshold_percent: number;
  llm_log_enabled: boolean;
  conversation_title_provider_id: string | null;
  conversation_title_model_id: string | null;
};

export type ApiKeyStrategy = 'failover' | 'round_robin' | 'random';

export type ModelProvider = {
  id: string;
  provider_type: ProviderType;
  name: string;
  base_url: string;
  image_generation_url?: string | null;
  image_edit_url?: string | null;
  has_api_key: boolean;
  /** One masked entry per stored key, in pool order. */
  masked_api_keys: string[];
  key_strategy: ApiKeyStrategy;
  models: string[];
  model_configs: ModelConfig[];
  extra_body: Record<string, unknown> | null;
  prompt_cache?: PromptCacheConfig;
  cache_capabilities?: ProviderCacheCapabilities | null;
  proxy_url?: string | null;
  created_at: string;
  updated_at: string;
};

export type ModelConfig = {
  model_id: string;
  multimodal: boolean;
  thinking: boolean;
  /** Context window size in tokens. Defaults to 128000 (128K). */
  context_size: number;
  input_price?: number;
  output_price?: number;
  cache_write_price?: number;
  cache_read_price?: number;
  image_options?: ImageModelConfig;
};

export type ImageModelConfig = {
  size?: string;
  quality?: string;
  n?: number;
  output_format?: string;
  output_compression?: number;
  background?: string;
  moderation?: string;
  style?: string;
  response_format?: string;
  input_fidelity?: string;
};

export type ProviderType =
  | 'openai_compatible'
  | 'openai_responses'
  | 'anthropic_messages'
  | 'google_gemini'
  | 'openai_images';

export type ModelProviderPayload = {
  provider_type: ProviderType;
  name: string;
  base_url: string;
  image_generation_url?: string | null;
  image_edit_url?: string | null;
  /** Masked entries keep the stored key at that slot; plaintext entries are new keys. */
  api_keys: string[];
  key_strategy: ApiKeyStrategy;
  models: string[];
  model_configs: ModelConfig[];
  extra_body?: Record<string, unknown> | null;
  prompt_cache?: PromptCacheConfig;
  cache_capabilities?: ProviderCacheCapabilities | null;
  proxy_url?: string | null;
};

export type PromptCacheMode = 'disabled' | 'automatic' | 'explicit';

export type PromptCacheConfig = {
  mode: PromptCacheMode;
  use_stable_key: boolean;
  retention?: string | null;
  anthropic_ttl?: string | null;
  gemini_cached_content?: string | null;
};

export type ProviderCacheCapabilities = {
  automatic_prefix_cache: boolean;
  explicit_breakpoints: boolean;
  stable_cache_key: boolean;
  supports_cache_retention: boolean;
};

export type FetchProviderModelsPayload = {
  provider_type: ProviderType;
  base_url: string;
  image_generation_url?: string | null;
  image_edit_url?: string | null;
  api_key?: string;
  extra_body?: Record<string, unknown>;
  proxy_url?: string | null;
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
export type GenerationMode = 'chat' | 'image';

export type ImageAttachment = {
  id: string;
  name: string;
  media_type: 'image/png' | 'image/jpeg' | 'image/webp' | 'image/gif';
  is_mask?: boolean;
} & (
  | { source_type: 'base64'; data: string }
  | { source_type: 'url'; url: string }
  | { source_type: 'local'; path: string }
);

export type TokenUsage = {
  input_tokens: number;
  output_tokens: number;
  cache_write_tokens: number;
  cache_read_tokens: number;
  total_prompt_tokens?: number;
  uncached_input_tokens?: number;
  cache_hit_ratio?: number;
};

export type ModelUsage = TokenUsage & {
  total_tokens: number;
  cost_usd?: number | null;
  image_count?: number;
  provider_id: string;
  model_id: string;
};

export type ConversationUsage = TokenUsage & {
  total_tokens: number;
  cost_usd?: number | null;
  image_count?: number;
};

export type ScheduledTask = {
  id: string;
  agent_id: string;
  source_conversation_id: string | null;
  schedule_expr: string;
  timezone_offset: string;
  prompt: string;
  enabled: boolean;
  next_run_at: string;
  last_run_at: string | null;
  last_status: string | null;
  last_error: string | null;
  active_execution_id: string | null;
  latest_execution_conversation_id: string | null;
  created_at: string;
  updated_at: string;
};

export type ScheduledTaskExecution = {
  id: string;
  task_id: string;
  conversation_id: string;
  scheduled_at: string;
  started_at: string | null;
  finished_at: string | null;
  status: string;
  error: string | null;
  trigger_type?: 'scheduled' | 'manual' | string;
  agent_name?: string | null;
  provider_name?: string | null;
  model_id?: string | null;
  output_preview?: string | null;
};

export type ConversationHeartbeat = {
  conversation_id: string;
  agent_id: string;
  interval_seconds: number;
  prompt: string;
  enabled: boolean;
  next_fire_at: string;
  pending: boolean;
  last_run_at: string | null;
  last_status: string | null;
  last_error: string | null;
};

export type ConversationHeartbeatExecution = {
  id: string;
  conversation_id: string;
  scheduled_at: string;
  started_at: string | null;
  finished_at: string | null;
  status: string;
  error: string | null;
  trigger_type?: 'scheduled' | 'manual' | string;
  agent_name?: string | null;
  provider_name?: string | null;
  model_id?: string | null;
  output_preview?: string | null;
};

export type ChatToolCall = {
  id: string;
  type: 'function';
  permission_mode?: 'request_approval' | 'full_access';
  started_at_ms?: number;
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

export type UiMessage = ChatMessage & {
  clientId: string;
  /** Ordered text/tool segments inside one assistant bubble (same agent run). */
  blocks?: AssistantBlock[];
};

export type CreateAgentPayload = {
  name: string;
  description: string;
  model_provider_id: string;
  model_id: string;
  avatar_icon: AvatarIcon;
  avatar_color: string;
};

export type IpProxyProtocol = 'http' | 'https' | 'socks4' | 'socks5';
export type IpProxyStatus = 'unchecked' | 'available' | 'unavailable';
export type IpProxyQualityStatus = 'unchecked' | 'available' | 'error';

export type IpProxyQualityProbe = {
  status: IpProxyQualityStatus;
  latency_ms: number | null;
  error: string | null;
};

export type IpProxyQuality = {
  google: IpProxyQualityProbe;
  openai: IpProxyQualityProbe;
  anthropic: IpProxyQualityProbe;
  grok: IpProxyQualityProbe;
};

export type IpProxy = {
  id: string;
  name: string;
  url: string;
  protocol: IpProxyProtocol;
  enabled: boolean;
  is_pinned: boolean;
  status: IpProxyStatus;
  latency_ms: number | null;
  last_checked_at: string | null;
  last_error: string | null;
  ip_address: string | null;
  country: string | null;
  country_code: string | null;
  region: string | null;
  city: string | null;
  api_quality: IpProxyQuality;
  created_at: string;
  updated_at: string;
};

export type IpProxyPayload = {
  name: string;
  url: string;
  enabled: boolean;
  is_pinned?: boolean;
};

export type IpProxyImportEntry = {
  name?: string;
  url: string;
};

export type IpProxyImportResult = {
  imported_count: number;
  duplicate_count: number;
  invalid_count: number;
  proxies: IpProxy[];
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
  title?: string;
};

export type ToolCallStatus = 'running' | 'pending_approval' | 'completed' | 'blocked' | 'rejected' | 'interrupted' | 'error';
export type AgentRunPhase = 'idle' | 'waiting_for_model' | 'compacting_context' | 'executing_tool' | 'waiting_for_approval';
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
  code?: string;
  /** Execution backend used for the terminal command. */
  runtime?: string;
  /** Structured protocol payload preserved from MCP-backed tools. */
  structured_content?: unknown;
  is_error?: boolean;
};

export type ToolArguments = Record<string, unknown>;

export type ToolCallState = {
  call_id: string;
  name: string;
  arguments: ToolArguments;
  /** Raw JSON while the model is still assembling the tool call. */
  arguments_text?: string;
  phase?: 'generating_arguments' | 'executing';
  started_at_ms?: number;
  permission_mode?: 'request_approval' | 'full_access';
  status: ToolCallStatus;
  result?: ToolResultData;
};

export type AssistantBlock =
  | { type: 'text'; content: string }
  | { type: 'tool'; tool: ToolCallState };

/** Chat timeline item: one bubble per message (tools nest inside assistant). */
export type UiMessageItem = UiMessage;

export type StreamEvent =
  | { event: 'user'; data: ChatMessage; sequence?: number }
  | { event: 'assistant'; data: ChatMessage; sequence?: number }
  | { event: 'usage'; data: ModelUsage; sequence?: number }
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
      event: 'retry';
      sequence?: number;
      data: {
        attempt: number;
        max_attempts: number;
        reason: string;
      };
    }
  | {
      event: 'tool_call_started';
      sequence?: number;
      data: {
        call_id: string;
        name: string;
        permission_mode: 'request_approval' | 'full_access';
      };
    }
  | {
      event: 'tool_call_args_delta';
      sequence?: number;
      data: {
        call_id: string;
        arguments: string;
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

export type NavKey = 'chat' | 'monitor' | 'agents' | 'tools' | 'skills' | 'scheduling' | 'settings';

export type SettingsSection = 'runtime' | 'agent' | 'providers' | 'proxies';

export type MobileView = 'list' | 'main';

export type LlmLogEntry = {
  id: string;
  agent_name: string;
  agent_id?: string;
  conversation_id?: string;
  model_id: string;
  provider_name: string;
  created_at: string;
  raw_status?: number;
  first_token_ms?: number;
  duration_ms?: number;
  input_tokens?: number;
  output_tokens?: number;
  cache_write_tokens?: number;
  cache_read_tokens?: number;
  cost_usd?: number;
  error?: string;
  has_payload?: boolean;
  is_compaction: boolean;
};

export type LlmLogDetail = LlmLogEntry & {
  raw_request: string;
  raw_response?: string;
};

export type LlmLogAgentStat = {
  agent_key: string;
  agent_id?: string;
  agent_name: string;
  request_count: number;
  latest_at: string;
};

export type LlmLogModelStat = {
  model_key: string;
  provider_name: string;
  model_id: string;
  request_count: number;
  latest_at: string;
};

export type LlmLogSummary = {
  request_count: number;
  success_count: number;
  error_count: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cost_usd: number;
};

export type LlmLogKindSummary = {
  all_count: number;
  compaction_count: number;
  regular_count: number;
};

export type LlmLogListPage = {
  items: LlmLogEntry[];
  total: number;
  offset: number;
  limit: number;
  agent_stats?: LlmLogAgentStat[];
  model_stats?: LlmLogModelStat[];
  summary?: LlmLogSummary;
  kind_summary?: LlmLogKindSummary;
};

export {
  MAX_WEB_SEARCH_KEYS_PER_SOURCE,
} from './toolTypes';
export type * from './toolTypes';
