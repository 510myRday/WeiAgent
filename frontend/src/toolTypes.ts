/** Domain types previously defined in api.ts. Request helpers stay in api.ts. */

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
  isCompaction?: boolean;
};

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
  api_key_fingerprints?: string[];
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
  key_fingerprint?: string;
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
    source_overrides: WebSearchSourceOverrides[];
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

export type WebSearchSourceOverrides = {
  source_id: string;
  enabled?: boolean;
  deep_enabled?: boolean;
  extract_enabled?: boolean;
  priority?: number;
  roles?: WebSearchSourceRole[];
  proxy_url?: string | null;
  proxy_url_override?: boolean;
};

export type McpTransport =
  | { type: 'stdio'; command: string; args: string[]; cwd?: string | null; env: Record<string, string>; inherit_env?: boolean; env_allowlist?: string[]; long_running?: boolean; timeout_ms?: number }
  | { type: 'streamable_http'; url: string; bearer_token?: string | null; headers: Record<string, string>; client_cert?: string | null; client_key?: string | null; identity_header?: McpIdentityHeader | null; oauth?: McpOAuthConfig | null; timeout_ms: number }
  | { type: 'sse'; url: string; bearer_token?: string | null; headers: Record<string, string>; client_cert?: string | null; client_key?: string | null; identity_header?: McpIdentityHeader | null; oauth?: McpOAuthConfig | null; timeout_ms: number };

export type McpIdentityHeader = { name: string; value: string };

export type McpOAuthConfig = {
  authorize_url: string;
  token_url: string;
  client_id: string;
  client_secret?: string | null;
  scopes: string[];
  redirect_port?: number | null;
  redirect_uri?: string | null;
};

export type McpTool = {
  tool_id: string;
  name: string;
  qualified_name: string;
  title?: string;
  description: string;
  input_schema: Record<string, unknown>;
  enabled: boolean;
  updated_at?: string;
};

export type McpAuthState = {
  status: 'idle' | 'starting' | 'auth_pending' | 'browser_opened' | 'waiting_callback' | 'authorized' | 'connected' | 'auth_timeout' | 'auth_failed' | 'launch_failed' | 'init_failed' | 'unsupported';
  authorize_url?: string | null;
  last_success_at?: string | null;
  last_updated_at?: string | null;
  last_error?: string | null;
  browser_opened: boolean;
  session_dir?: string | null;
};

export type McpServer = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  supports_parallel_tool_calls?: boolean;
  provider?: string | null;
  provider_url?: string | null;
  logo_url?: string | null;
  tags: string[];
  transport: McpTransport;
  tools: McpTool[];
  status: 'unknown' | 'connecting' | 'ready' | 'offline' | 'error';
  last_synced_at?: string;
  last_error?: string;
  diagnostics?: McpDiagnostics;
};

export type McpDiagnostics = {
  protocol_version?: string | null;
  server_name?: string | null;
  server_version?: string | null;
  capabilities?: unknown;
  last_operation?: string | null;
  last_operation_at?: string | null;
  last_duration_ms?: number | null;
  retry_count: number;
  last_error_code?: string | null;
};

export type McpServerDraft = {
  id?: string;
  name: string;
  description: string;
  enabled: boolean;
  supports_parallel_tool_calls: boolean;
  provider?: string | null;
  provider_url?: string | null;
  logo_url?: string | null;
  tags: string[];
  transport: McpTransport;
};

export type McpServerOverride = { server_id: string; enabled?: boolean; customized?: boolean };

export type McpToolOverride = { server_id: string; tool_name: string; enabled?: boolean };

export type McpAgentOverrides = { servers: McpServerOverride[]; tools: McpToolOverride[] };

export type McpAgentSettings = { defaults: McpServer[]; overrides: McpAgentOverrides; effective: McpServer[] };

export type SkillFile = { path: string; kind: string; size_bytes: number };

export type SkillDiagnostic = { path: string; message: string };

export type SkillConflict = { name: string; count: number; paths: string[] };

export type SkillManifest = {
  name: string;
  description: string;
  version?: string | null;
  group?: string | null;
  group_id?: string | null;
  tags: string[];
  icon_url?: string | null;
  source_url?: string | null;
  root_dir: string;
  skill_md: string;
  scripts: SkillFile[];
  references: SkillFile[];
  examples: SkillFile[];
  content_hash: string;
  modified_at: string;
  created_at: string;
  updated_at: string;
  diagnostics: SkillDiagnostic[];
  enabled: boolean;
  global_override?: boolean | null;
  group_override?: boolean | null;
  agent_override?: boolean | null;
  agent_group_override?: boolean | null;
};

export type SkillGroupState = {
  id: string;
  name: string;
  sort_order: number;
  skill_count: number;
  enabled: boolean;
  global_override?: boolean | null;
  agent_override?: boolean | null;
};

export type SkillOverride = { name: string; enabled?: boolean | null };

export type SkillGroupOverride = { group: string; group_id?: string | null; enabled?: boolean | null };

export type SkillOverrides = { skills: SkillOverride[]; groups: SkillGroupOverride[] };

export type SkillSettings = {
  skills_dir: string;
  defaults: SkillManifest[];
  overrides: SkillOverrides;
  effective: SkillManifest[];
  groups: SkillGroupState[];
  orphan_skills: string[];
  orphan_groups: string[];
  diagnostics: SkillDiagnostic[];
  conflicts: SkillConflict[];
};

export type SkillImportTask = {
  id: string;
  source: string;
  kind: 'local' | 'network' | string;
  group_id?: string | null;
  status: 'queued' | 'running' | 'completed' | 'failed' | string;
  phase: string;
  progress: number;
  error?: string | null;
  created_at: string;
  updated_at: string;
  finished_at?: string | null;
};

export type SkillContent = {
  name: string;
  path: string;
  root_dir: string;
  cwd: string;
  content: string;
  media_type: string;
  data_base64?: string | null;
  files: SkillFile[];
  created_at: string;
  updated_at: string;
  scripts: SkillFile[];
  references: SkillFile[];
  examples: SkillFile[];
};

export type SkillChangeEvent = {
  version: number;
  list_changed: boolean;
  changed_skills: string[];
};

export type SkillChangeFeed = {
  version: number;
  events: SkillChangeEvent[];
  full_refresh: boolean;
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

export type ApprovalDecision = 'approve' | 'reject' | 'always_approve';
