mod results;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Effective terminal tool configuration assembled from SQLite defaults and agent overrides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalToolConfig {
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    /// User-facing permission mode for local terminal execution.
    #[serde(default)]
    pub permission_mode: TerminalPermissionMode,
    /// Exact, context-bound grants created by interactive "always allow".
    #[serde(default)]
    pub approval_grants: Vec<TerminalApprovalGrant>,
    /// Foreground wait timeout. A still-running command is moved to background after this time.
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
    /// Additional host roots that may be mounted/used by the tool. The dedicated
    /// terminal workspace under the WeiAgent data root is always the default root.
    #[serde(default)]
    pub allowed_cwd_roots: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalPermissionMode {
    /// Routine work uses the configured workspace roots. Expanded host access requires approval.
    #[default]
    RequestApproval,
    /// Local commands run with full host access and no approval prompts.
    FullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalApprovalGrant {
    pub command: String,
    pub cwd: Option<String>,
    pub permission_mode: TerminalPermissionMode,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_timeout_ms() -> u64 {
    30_000
}

pub const DEFAULT_MAX_LLM_ITERATIONS: u32 = 99;
pub const MAX_LLM_ITERATIONS: u32 = 999;
pub const DEFAULT_TASK_PLAN_PROGRESS_INTERVAL: u32 = 5;
pub const MAX_TASK_PLAN_PROGRESS_INTERVAL: u32 = 999;
pub const DEFAULT_WEB_SEARCH_MAX_RESULTS: u32 = 5;
pub const MAX_WEB_SEARCH_RESULTS: u32 = 20;
pub const DEFAULT_WEB_SEARCH_TIMEOUT_MS: u64 = 15_000;
pub const MAX_WEB_SEARCH_TIMEOUT_MS: u64 = 60_000;
pub const MAX_WEB_SEARCH_SOURCES: usize = 8;
pub const MAX_WEB_SEARCH_KEYS_PER_SOURCE: usize = 1_000;
pub const MAX_WEB_SEARCH_QUERY_CHARS: usize = 500;
pub const MAX_WEB_SEARCH_QUERIES: usize = 4;
pub const MAX_WEB_EXTRACT_URLS: usize = 5;
pub const DEFAULT_WEB_EXTRACT_MAX_REMOTE_ATTEMPTS: u32 = 3;
pub const MAX_WEB_EXTRACT_REMOTE_ATTEMPTS: u32 = 5;
pub const DEFAULT_EVIDENCE_RETENTION_DAYS: u32 = 30;
pub const MAX_EVIDENCE_RETENTION_DAYS: u32 = 3_650;
pub const DEFAULT_EVIDENCE_MAX_TOTAL_MB: u64 = 200;
pub const MAX_EVIDENCE_TOTAL_MB: u64 = 10_240;

fn default_max_llm_iterations() -> u32 {
    DEFAULT_MAX_LLM_ITERATIONS
}

pub(super) fn default_web_search_max_results() -> u32 {
    DEFAULT_WEB_SEARCH_MAX_RESULTS
}

pub(super) fn default_web_search_timeout_ms() -> u64 {
    DEFAULT_WEB_SEARCH_TIMEOUT_MS
}

impl Default for TerminalToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            permission_mode: TerminalPermissionMode::RequestApproval,
            approval_grants: Vec::new(),
            default_timeout_ms: default_timeout_ms(),
            allowed_cwd_roots: Vec::new(),
        }
    }
}

/// Effective tool configuration collection returned by the storage layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentToolsConfig {
    /// Maximum tool-capable LLM iterations for one user turn.
    /// After this budget is exhausted, one final no-tools summary call is made.
    #[serde(default = "default_max_llm_iterations")]
    pub max_llm_iterations: u32,
    #[serde(default)]
    pub terminal: TerminalToolConfig,
    #[serde(default)]
    pub web_search: WebSearchToolConfig,
    #[serde(default)]
    pub evidence_storage: EvidenceStorageConfig,
    #[serde(default)]
    pub task_planning: TaskPlanningToolConfig,
}

impl Default for AgentToolsConfig {
    fn default() -> Self {
        Self {
            max_llm_iterations: default_max_llm_iterations(),
            terminal: TerminalToolConfig::default(),
            web_search: WebSearchToolConfig::default(),
            evidence_storage: EvidenceStorageConfig::default(),
            task_planning: TaskPlanningToolConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskPlanningToolConfig {
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    #[serde(default = "default_task_plan_progress_interval")]
    pub progress_interval: u32,
}

impl Default for TaskPlanningToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            progress_interval: DEFAULT_TASK_PLAN_PROGRESS_INTERVAL,
        }
    }
}

fn default_task_plan_progress_interval() -> u32 {
    DEFAULT_TASK_PLAN_PROGRESS_INTERVAL
}

impl AgentToolsConfig {
    pub fn max_llm_iterations(&self) -> usize {
        self.max_llm_iterations.clamp(1, MAX_LLM_ITERATIONS) as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceStorageConfig {
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    #[serde(default = "default_evidence_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_evidence_max_total_mb")]
    pub max_total_mb: u64,
}

fn default_evidence_retention_days() -> u32 {
    DEFAULT_EVIDENCE_RETENTION_DAYS
}

fn default_evidence_max_total_mb() -> u64 {
    DEFAULT_EVIDENCE_MAX_TOTAL_MB
}

impl Default for EvidenceStorageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: DEFAULT_EVIDENCE_RETENTION_DAYS,
            max_total_mb: DEFAULT_EVIDENCE_MAX_TOTAL_MB,
        }
    }
}

mod web_search;
pub use web_search::*;

pub fn api_key_fingerprint(api_key: &str) -> String {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(super) fn is_api_key_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn mask_api_key(api_key: &str) -> String {
    mask_secret_tail(api_key, false)
}

/// Mask a secret as `****` plus the last 4 characters.
/// When `mask_short` is true, values of 4 characters or fewer become `****`.
pub fn mask_secret_tail(value: &str, mask_short: bool) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return if mask_short {
            "****".to_string()
        } else {
            String::new()
        };
    }
    if mask_short && trimmed.chars().count() <= 4 {
        return "****".to_string();
    }
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("****{suffix}")
}

pub fn is_masked_api_key(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("****") && trimmed.len() <= 8
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentToolsOverrides {
    #[serde(default = "tool_overrides_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_llm_iterations: Option<u32>,
    #[serde(default)]
    pub terminal: TerminalToolOverrides,
    #[serde(default)]
    pub web_search: WebSearchToolOverrides,
    #[serde(default)]
    pub task_planning: TaskPlanningToolOverrides,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalToolOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<TerminalPermissionMode>,
    #[serde(default)]
    pub approval_grants: Vec<TerminalApprovalGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_cwd_roots: Option<Vec<String>>,
}

/// Agent-level overrides for web_search. Secrets stay global-only.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchToolOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(default)]
    pub source_overrides: Vec<WebSearchSourceOverrides>,
}

/// Agent-level overrides for one global web search source. Credentials and
/// provider metadata intentionally stay in the global source configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchSourceOverrides {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<WebSearchSourceRole>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "crate::domain::serde_defaults::is_false"
    )]
    pub proxy_url_override: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskPlanningToolOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_interval: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolsSettings {
    pub defaults: AgentToolsConfig,
    pub overrides: AgentToolsOverrides,
    pub effective: AgentToolsConfig,
}

fn tool_overrides_version() -> u32 {
    1
}

impl Default for AgentToolsOverrides {
    fn default() -> Self {
        Self {
            version: tool_overrides_version(),
            max_llm_iterations: None,
            terminal: TerminalToolOverrides::default(),
            web_search: WebSearchToolOverrides::default(),
            task_planning: TaskPlanningToolOverrides::default(),
        }
    }
}

impl AgentToolsOverrides {
    pub fn remove_expired_approval_grants(&mut self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let previous_len = self.terminal.approval_grants.len();
        self.terminal
            .approval_grants
            .retain(|grant| grant.expires_at.is_some_and(|expires_at| expires_at > now));
        self.terminal.approval_grants.len() != previous_len
    }

    pub fn apply_to(&self, defaults: &AgentToolsConfig) -> AgentToolsConfig {
        AgentToolsConfig {
            max_llm_iterations: self
                .max_llm_iterations
                .unwrap_or(defaults.max_llm_iterations),
            terminal: TerminalToolConfig {
                enabled: self.terminal.enabled.unwrap_or(defaults.terminal.enabled),
                permission_mode: self
                    .terminal
                    .permission_mode
                    .clone()
                    .unwrap_or_else(|| defaults.terminal.permission_mode.clone()),
                approval_grants: self.terminal.approval_grants.clone(),
                default_timeout_ms: self
                    .terminal
                    .default_timeout_ms
                    .unwrap_or(defaults.terminal.default_timeout_ms),
                allowed_cwd_roots: self
                    .terminal
                    .allowed_cwd_roots
                    .clone()
                    .unwrap_or_else(|| defaults.terminal.allowed_cwd_roots.clone()),
            },
            web_search: WebSearchToolConfig {
                enabled: self
                    .web_search
                    .enabled
                    .unwrap_or(defaults.web_search.enabled),
                max_results: self
                    .web_search
                    .max_results
                    .unwrap_or(defaults.web_search.max_results),
                timeout_ms: defaults.web_search.timeout_ms,
                key_selection: defaults.web_search.key_selection.clone(),
                extraction: defaults.web_search.extraction.clone(),
                sources: apply_web_search_source_overrides(
                    &defaults.web_search.sources,
                    &self.web_search.source_overrides,
                ),
            },
            evidence_storage: defaults.evidence_storage.clone(),
            task_planning: TaskPlanningToolConfig {
                enabled: self
                    .task_planning
                    .enabled
                    .unwrap_or(defaults.task_planning.enabled),
                progress_interval: self
                    .task_planning
                    .progress_interval
                    .unwrap_or(defaults.task_planning.progress_interval),
            },
        }
    }

    #[cfg(test)]
    pub fn full_override(config: &AgentToolsConfig) -> AgentToolsOverrides {
        AgentToolsOverrides {
            version: tool_overrides_version(),
            max_llm_iterations: Some(config.max_llm_iterations),
            terminal: TerminalToolOverrides {
                enabled: Some(config.terminal.enabled),
                permission_mode: Some(config.terminal.permission_mode.clone()),
                approval_grants: config.terminal.approval_grants.clone(),
                default_timeout_ms: Some(config.terminal.default_timeout_ms),
                allowed_cwd_roots: Some(config.terminal.allowed_cwd_roots.clone()),
            },
            web_search: WebSearchToolOverrides {
                enabled: Some(config.web_search.enabled),
                max_results: Some(config.web_search.max_results),
                source_overrides: config
                    .web_search
                    .sources
                    .iter()
                    .map(|source| WebSearchSourceOverrides {
                        source_id: source.id.clone(),
                        enabled: Some(source.enabled),
                        deep_enabled: Some(source.deep_enabled),
                        extract_enabled: Some(source.extract_enabled),
                        priority: Some(source.priority),
                        roles: Some(source.roles.clone()),
                        proxy_url: source.proxy_url.clone(),
                        proxy_url_override: true,
                    })
                    .collect(),
            },
            task_planning: TaskPlanningToolOverrides {
                enabled: Some(config.task_planning.enabled),
                progress_interval: Some(config.task_planning.progress_interval),
            },
        }
    }
}

fn apply_web_search_source_overrides(
    defaults: &[WebSearchSourceConfig],
    overrides: &[WebSearchSourceOverrides],
) -> Vec<WebSearchSourceConfig> {
    let mut sources = defaults.to_vec();
    for source_override in overrides {
        let Some(source) = sources
            .iter_mut()
            .find(|source| source.id == source_override.source_id)
        else {
            continue;
        };
        if let Some(enabled) = source_override.enabled {
            source.enabled = enabled;
        }
        if let Some(deep_enabled) = source_override.deep_enabled {
            source.deep_enabled = deep_enabled;
        }
        if let Some(extract_enabled) = source_override.extract_enabled {
            source.extract_enabled = extract_enabled;
        }
        if let Some(priority) = source_override.priority {
            source.priority = priority;
        }
        if let Some(roles) = &source_override.roles {
            source.roles = roles.clone();
        }
        if source_override.proxy_url_override {
            source.proxy_url = source_override.proxy_url.clone();
        }
    }
    sources.sort_by_key(|source| source.priority);
    sources
}

impl TerminalToolConfig {
    pub fn has_context_grant(&self, command: &str, cwd: Option<&str>) -> bool {
        self.approval_grants.iter().any(|grant| {
            grant.command == command
                && grant.cwd.as_deref() == cwd
                && grant.permission_mode == TerminalPermissionMode::RequestApproval
                && grant
                    .expires_at
                    .is_some_and(|expires_at| expires_at > chrono::Utc::now())
        })
    }
}

impl AgentToolsConfig {
    pub fn redact_secrets(&self) -> Self {
        let mut redacted = self.clone();
        redacted.web_search = self.web_search.redact_secrets();
        redacted
    }
}

impl AgentToolsSettings {
    pub fn redact_secrets(&self) -> Self {
        Self {
            defaults: self.defaults.redact_secrets(),
            overrides: self.overrides.clone(),
            effective: self.effective.redact_secrets(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_loop_defaults_to_ninety_nine_iterations() {
        assert_eq!(
            AgentToolsConfig::default().max_llm_iterations,
            DEFAULT_MAX_LLM_ITERATIONS
        );
        assert_eq!(DEFAULT_MAX_LLM_ITERATIONS, 99);
    }

    #[test]
    fn runtime_iteration_count_is_bounded_by_the_supported_maximum() {
        let config = AgentToolsConfig {
            max_llm_iterations: MAX_LLM_ITERATIONS + 1,
            ..AgentToolsConfig::default()
        };
        assert_eq!(config.max_llm_iterations(), MAX_LLM_ITERATIONS as usize);
    }

    #[test]
    fn empty_agent_grants_are_present_in_api_json() {
        let value = serde_json::to_value(AgentToolsOverrides::default()).unwrap();
        assert_eq!(value["terminal"]["approval_grants"], serde_json::json!([]));
    }

    #[test]
    fn expired_and_undated_approval_grants_are_removed() {
        let now = chrono::Utc::now();
        let mut overrides = AgentToolsOverrides::default();
        overrides.terminal.approval_grants = vec![
            TerminalApprovalGrant {
                command: "stale".to_string(),
                cwd: None,
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(now - chrono::Duration::hours(2)),
                expires_at: Some(now - chrono::Duration::hours(1)),
            },
            TerminalApprovalGrant {
                command: "undated".to_string(),
                cwd: None,
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(now),
                expires_at: None,
            },
            TerminalApprovalGrant {
                command: "active".to_string(),
                cwd: None,
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(now),
                expires_at: Some(now + chrono::Duration::hours(1)),
            },
        ];
        assert!(overrides.remove_expired_approval_grants(now));
        assert_eq!(overrides.terminal.approval_grants.len(), 1);
        assert_eq!(overrides.terminal.approval_grants[0].command, "active");
        assert!(!overrides.remove_expired_approval_grants(now));
    }

    #[test]
    fn web_search_defaults_enable_anonymous_sources() {
        let config = AgentToolsConfig::default();
        assert!(config.web_search.enabled);
        assert_eq!(
            config.web_search.max_results,
            DEFAULT_WEB_SEARCH_MAX_RESULTS
        );
        assert_eq!(config.web_search.sources.len(), 5);
        assert_eq!(config.web_search.sources[0].id, "tavily");
        assert_eq!(config.web_search.sources[1].id, "exa");
        assert_eq!(config.web_search.sources[2].id, "jina");
        assert_eq!(config.web_search.sources[3].id, "grok");
        assert_eq!(config.web_search.sources[4].id, "firecrawl");
        assert!(!config.web_search.sources[0].enabled);
        assert!(!config.web_search.sources[1].enabled);
        assert!(config.web_search.sources[2].enabled);
        assert!(!config.web_search.sources[3].enabled);
        assert!(config.web_search.sources[4].enabled);
        assert!(!config.web_search.sources[0].has_available_key());
        assert!(config.web_search.sources[2].can_search());
        assert!(config.web_search.sources[4].can_search());
    }

    #[test]
    fn terminal_enabled_defaults_to_true_when_omitted() {
        let terminal: TerminalToolConfig = serde_json::from_value(serde_json::json!({
            "permission_mode": "request_approval",
            "approval_grants": [],
            "default_timeout_ms": 30000,
            "allowed_cwd_roots": []
        }))
        .unwrap();
        assert!(terminal.enabled);
    }

    #[test]
    fn evidence_storage_defaults_are_enabled_and_backward_compatible() {
        let config = AgentToolsConfig::default();
        assert!(config.evidence_storage.enabled);
        assert_eq!(
            config.evidence_storage.retention_days,
            DEFAULT_EVIDENCE_RETENTION_DAYS
        );
        assert_eq!(
            config.evidence_storage.max_total_mb,
            DEFAULT_EVIDENCE_MAX_TOTAL_MB
        );

        let mut value = serde_json::to_value(config).unwrap();
        value.as_object_mut().unwrap().remove("evidence_storage");
        let parsed: AgentToolsConfig = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.evidence_storage, EvidenceStorageConfig::default());
    }

    #[test]
    fn task_planning_defaults_to_enabled_and_agent_override_is_inherited() {
        let defaults = AgentToolsConfig::default();
        assert!(defaults.task_planning.enabled);
        assert_eq!(
            defaults.task_planning.progress_interval,
            DEFAULT_TASK_PLAN_PROGRESS_INTERVAL
        );

        let overrides = AgentToolsOverrides::default();
        assert_eq!(overrides.task_planning.enabled, None);
        assert!(overrides.apply_to(&defaults).task_planning.enabled);
        assert_eq!(
            overrides
                .apply_to(&defaults)
                .task_planning
                .progress_interval,
            DEFAULT_TASK_PLAN_PROGRESS_INTERVAL
        );

        let overrides = AgentToolsOverrides {
            task_planning: TaskPlanningToolOverrides {
                enabled: Some(false),
                progress_interval: None,
            },
            ..AgentToolsOverrides::default()
        };
        assert!(!overrides.apply_to(&defaults).task_planning.enabled);
    }

    #[test]
    fn web_search_agent_overrides_do_not_touch_secrets() {
        let mut defaults = AgentToolsConfig::default();
        defaults.web_search.enabled = false;
        defaults.web_search.max_results = 5;
        defaults.web_search.sources[0].api_keys = vec!["sk-secret".to_string()];

        let overrides = AgentToolsOverrides {
            web_search: WebSearchToolOverrides {
                enabled: Some(true),
                max_results: Some(8),
                source_overrides: Vec::new(),
            },
            ..AgentToolsOverrides::default()
        };
        let effective = overrides.apply_to(&defaults);
        assert!(effective.web_search.enabled);
        assert_eq!(effective.web_search.max_results, 8);
        assert_eq!(
            effective.web_search.sources[0].api_keys,
            vec!["sk-secret".to_string()]
        );
        assert_eq!(
            effective.web_search.timeout_ms,
            defaults.web_search.timeout_ms
        );
    }

    #[test]
    fn web_search_secret_merge_preserves_and_clears() {
        let mut stored = WebSearchToolConfig {
            enabled: true,
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                display_name: "Tavily".to_string(),
                enabled: true,
                deep_enabled: true,
                extract_enabled: true,
                priority: 10,
                independence_group: "tavily".to_string(),
                roles: WebSearchProviderKind::Tavily.default_roles(),
                api_keys: vec!["sk-live-secret".to_string()],
                api_key_fingerprints: Vec::new(),
                blocked_api_key_hashes: Vec::new(),
                key_statuses: Vec::new(),
                extract_key_statuses: Vec::new(),
                proxy_url: None,
            }],
            ..WebSearchToolConfig::default()
        };
        stored.sources[0].remember_blocked_api_key("sk-blocked");

        let preserve = WebSearchToolConfig {
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                display_name: "Tavily".to_string(),
                enabled: true,
                deep_enabled: true,
                extract_enabled: true,
                priority: 10,
                independence_group: "tavily".to_string(),
                roles: WebSearchProviderKind::Tavily.default_roles(),
                api_keys: vec!["__preserve__".to_string()],
                api_key_fingerprints: Vec::new(),
                blocked_api_key_hashes: Vec::new(),
                key_statuses: Vec::new(),
                extract_key_statuses: Vec::new(),
                proxy_url: None,
            }],
            ..stored.clone()
        };
        let merged = stored.merge_secret_update(&preserve);
        assert_eq!(
            merged.sources[0].api_keys,
            vec!["sk-live-secret".to_string()]
        );

        let clear = WebSearchToolConfig {
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                display_name: "Tavily".to_string(),
                enabled: true,
                deep_enabled: true,
                extract_enabled: true,
                priority: 10,
                independence_group: "tavily".to_string(),
                roles: WebSearchProviderKind::Tavily.default_roles(),
                api_keys: vec![],
                api_key_fingerprints: Vec::new(),
                blocked_api_key_hashes: Vec::new(),
                key_statuses: Vec::new(),
                extract_key_statuses: Vec::new(),
                proxy_url: None,
            }],
            ..stored.clone()
        };
        let cleared = stored.merge_secret_update(&clear);
        assert!(cleared.sources[0].api_keys.is_empty());

        let replace = WebSearchToolConfig {
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                display_name: "Tavily".to_string(),
                enabled: true,
                deep_enabled: true,
                extract_enabled: true,
                priority: 10,
                independence_group: "tavily".to_string(),
                roles: WebSearchProviderKind::Tavily.default_roles(),
                api_keys: vec!["sk-new".to_string(), "sk-blocked".to_string()],
                api_key_fingerprints: Vec::new(),
                blocked_api_key_hashes: Vec::new(),
                key_statuses: Vec::new(),
                extract_key_statuses: Vec::new(),
                proxy_url: None,
            }],
            ..stored.clone()
        };
        let replaced = stored.merge_secret_update(&replace);
        assert_eq!(replaced.sources[0].api_keys, vec!["sk-new".to_string()]);
        assert!(replaced.sources[0].is_api_key_blocked("sk-blocked"));
        assert!(
            stored.redact_secrets().sources[0]
                .blocked_api_key_hashes
                .is_empty()
        );

        let mut backend_update = stored.clone();
        backend_update.sources[0].remember_blocked_api_key("sk-newly-blocked");
        let merged = stored.merge_secret_update(&backend_update);
        assert!(merged.sources[0].is_api_key_blocked("sk-newly-blocked"));
    }

    #[test]
    fn legacy_tool_defaults_without_web_search_deserialize() {
        let config: AgentToolsConfig = serde_json::from_value(serde_json::json!({
            "max_llm_iterations": 99,
            "terminal": {
                "enabled": true,
                "permission_mode": "request_approval",
                "approval_grants": [],
                "default_timeout_ms": 30000,
                "allowed_cwd_roots": []
            }
        }))
        .unwrap();
        assert!(config.web_search.enabled);
        assert_eq!(config.web_search.sources.len(), 5);
        assert_eq!(config.web_search.sources[3].id, "grok");
        assert_eq!(config.web_search.sources[4].id, "firecrawl");
    }

    #[test]
    fn missing_builtin_search_source_is_restored_from_previous_config() {
        let mut previous = WebSearchToolConfig::default();
        let firecrawl = previous
            .sources
            .iter_mut()
            .find(|source| source.id == "firecrawl")
            .unwrap();
        firecrawl.api_keys = vec!["fc-secret".to_string()];

        let mut incoming = previous.clone();
        incoming.sources.retain(|source| source.id != "firecrawl");
        assert!(incoming.ensure_builtin_sources(Some(&previous)));

        let restored = incoming
            .sources
            .iter()
            .find(|source| source.id == "firecrawl")
            .unwrap();
        assert_eq!(restored.api_keys, vec!["fc-secret".to_string()]);
    }

    #[test]
    fn redacted_web_search_config_hides_keys() {
        let mut config = AgentToolsConfig::default();
        config.web_search.sources[0].api_keys = vec!["sk-abcdefgh".to_string()];
        let redacted = config.redact_secrets();
        assert_eq!(
            redacted.web_search.sources[0].api_keys,
            vec!["****efgh".to_string()]
        );
        assert_ne!(
            redacted.web_search.sources[0].api_keys[0],
            config.web_search.sources[0].api_keys[0]
        );
        assert_eq!(
            redacted.web_search.sources[0].api_key_fingerprints,
            vec![api_key_fingerprint("sk-abcdefgh")]
        );
    }

    #[test]
    fn masked_key_merge_uses_fingerprints_after_middle_key_removed() {
        let mut stored = WebSearchToolConfig::default();
        let source = stored
            .sources
            .iter_mut()
            .find(|source| source.id == "grok")
            .unwrap();
        source.api_keys = vec![
            "key-a-1234".to_string(),
            "key-b-5678".to_string(),
            "key-c-9012".to_string(),
        ];

        let redacted = stored.redact_secrets();
        let mut incoming = redacted.clone();
        let source = incoming
            .sources
            .iter_mut()
            .find(|source| source.id == "grok")
            .unwrap();
        source.api_keys.remove(1);
        source.api_key_fingerprints.remove(1);

        let merged = stored.merge_secret_update(&incoming);
        let source = merged
            .sources
            .iter()
            .find(|source| source.id == "grok")
            .unwrap();
        assert_eq!(
            source.api_keys,
            vec!["key-a-1234".to_string(), "key-c-9012".to_string()]
        );
    }
}

pub use results::*;
