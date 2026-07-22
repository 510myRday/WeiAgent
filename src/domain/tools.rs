use serde::{Deserialize, Serialize};

/// Terminal tool configuration stored in data/agents/{agent_id}/tools.json.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalToolConfig {
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
    /// Host roots that may be mounted/used by the tool. Empty means the WeiAgent data directory.
    #[serde(default)]
    pub allowed_cwd_roots: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalPermissionMode {
    /// Routine work runs in the local OS sandbox. Full host access requires user approval.
    #[default]
    RequestApproval,
    /// Codex runs commands with full host access and no approval prompts.
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
pub const DEFAULT_WEB_SEARCH_MAX_RESULTS: u32 = 5;
pub const MAX_WEB_SEARCH_RESULTS: u32 = 20;
pub const DEFAULT_WEB_SEARCH_TIMEOUT_MS: u64 = 15_000;
pub const MAX_WEB_SEARCH_TIMEOUT_MS: u64 = 60_000;
pub const MAX_WEB_SEARCH_SOURCES: usize = 8;
pub const MAX_WEB_SEARCH_KEYS_PER_SOURCE: usize = 16;
pub const MAX_WEB_SEARCH_QUERY_CHARS: usize = 500;

fn default_max_llm_iterations() -> u32 {
    DEFAULT_MAX_LLM_ITERATIONS
}

fn default_web_search_max_results() -> u32 {
    DEFAULT_WEB_SEARCH_MAX_RESULTS
}

fn default_web_search_timeout_ms() -> u64 {
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

/// Agent tool configuration collection (tools.json top-level structure).
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
}

impl Default for AgentToolsConfig {
    fn default() -> Self {
        Self {
            max_llm_iterations: default_max_llm_iterations(),
            terminal: TerminalToolConfig::default(),
            web_search: WebSearchToolConfig::default(),
        }
    }
}

impl AgentToolsConfig {
    pub fn max_llm_iterations(&self) -> usize {
        self.max_llm_iterations.clamp(1, MAX_LLM_ITERATIONS) as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProviderKind {
    Tavily,
    Exa,
    Jina,
    Grok,
}

impl WebSearchProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::Exa => "exa",
            Self::Jina => "jina",
            Self::Grok => "grok",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            Self::Tavily => "https://api.tavily.com",
            Self::Exa => "https://api.exa.ai",
            Self::Jina => "https://s.jina.ai",
            Self::Grok => "https://grok.com",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchKeySelection {
    #[default]
    Sequential,
    RoundRobin,
    Random,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchKeyStatusRecord {
    pub status: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchSourceConfig {
    pub id: String,
    pub provider: WebSearchProviderKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// Status slots aligned with api_keys. Null means this key has not been checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_statuses: Vec<Option<WebSearchKeyStatusRecord>>,
    /// FlareSolverr base URL, e.g. http://127.0.0.1:8191
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flaresolverr_url: Option<String>,
    /// Proxy used by FlareSolverr browser when opening grok.com (LAN-reachable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flaresolverr_proxy_url: Option<String>,
    /// Upstream proxy for Grok/CF traffic. Supports http:// and socks5://
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchToolConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_search_max_results")]
    pub max_results: u32,
    #[serde(default = "default_web_search_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub key_selection: WebSearchKeySelection,
    #[serde(default = "default_web_search_sources")]
    pub sources: Vec<WebSearchSourceConfig>,
}

fn default_web_search_sources() -> Vec<WebSearchSourceConfig> {
    vec![
        WebSearchSourceConfig {
            id: "tavily".to_string(),
            provider: WebSearchProviderKind::Tavily,
            enabled: true,
            base_url: None,
            api_keys: Vec::new(),
            key_statuses: Vec::new(),
            flaresolverr_url: None,
            flaresolverr_proxy_url: None,
            proxy_url: None,
        },
        WebSearchSourceConfig {
            id: "exa".to_string(),
            provider: WebSearchProviderKind::Exa,
            enabled: true,
            base_url: None,
            api_keys: Vec::new(),
            key_statuses: Vec::new(),
            flaresolverr_url: None,
            flaresolverr_proxy_url: None,
            proxy_url: None,
        },
        WebSearchSourceConfig {
            id: "jina".to_string(),
            provider: WebSearchProviderKind::Jina,
            enabled: true,
            base_url: None,
            api_keys: Vec::new(),
            key_statuses: Vec::new(),
            flaresolverr_url: None,
            flaresolverr_proxy_url: None,
            proxy_url: None,
        },
        WebSearchSourceConfig {
            id: "grok".to_string(),
            provider: WebSearchProviderKind::Grok,
            enabled: true,
            base_url: None,
            api_keys: Vec::new(),
            key_statuses: Vec::new(),
            flaresolverr_url: None,
            flaresolverr_proxy_url: None,
            proxy_url: None,
        },
    ]
}

impl Default for WebSearchToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            timeout_ms: DEFAULT_WEB_SEARCH_TIMEOUT_MS,
            key_selection: WebSearchKeySelection::Sequential,
            sources: default_web_search_sources(),
        }
    }
}

impl WebSearchToolConfig {
    pub fn max_results(&self) -> u32 {
        self.max_results.clamp(1, MAX_WEB_SEARCH_RESULTS)
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.clamp(1, MAX_WEB_SEARCH_TIMEOUT_MS)
    }

    pub fn has_usable_source(&self) -> bool {
        self.sources.iter().any(|source| {
            source.enabled
                && source
                    .api_keys
                    .iter()
                    .any(|key| !key.trim().is_empty())
        })
    }

    pub fn usable_sources(&self) -> impl Iterator<Item = &WebSearchSourceConfig> {
        self.sources.iter().filter(|source| {
            source.enabled
                && source
                    .api_keys
                    .iter()
                    .any(|key| !key.trim().is_empty())
        })
    }

    /// Replace secrets with masked values for API responses.
    pub fn redact_secrets(&self) -> Self {
        let mut redacted = self.clone();
        for source in &mut redacted.sources {
            source.api_keys = source
                .api_keys
                .iter()
                .map(|key| mask_api_key(key))
                .filter(|key| !key.is_empty())
                .collect();
        }
        redacted
    }

    /// Merge a client update into stored secrets.
    /// - omitted / null-equivalent sources keep previous keys when id matches and incoming keys empty with sentinel
    /// - empty api_keys list clears keys for that source
    /// - non-empty list replaces keys (entries that look like masks keep previous when possible)
    pub fn merge_secret_update(&self, incoming: &Self) -> Self {
        let mut merged = incoming.clone();
        for source in &mut merged.sources {
            let previous_keys = self
                .sources
                .iter()
                .find(|existing| existing.id == source.id)
                .map(|existing| existing.api_keys.clone())
                .unwrap_or_default();

            if source.api_keys.is_empty() {
                // Distinguish clear vs preserve:
                // Frontend sends [] to clear. When keys are omitted via preserve marker "__preserve__",
                // keep previous. For simplicity: if previous had keys and client sends only masks or
                // empty with preserve flag via special single entry.
                // Contract used by frontend:
                // - api_keys: ["__preserve__"] => keep previous
                // - api_keys: [] => clear
                // - api_keys: ["sk-..."] => replace
                continue;
            }

            if source.api_keys.len() == 1 && source.api_keys[0] == "__preserve__" {
                source.api_keys = previous_keys;
                continue;
            }

            // If every provided key is a masked placeholder matching previous count, preserve.
            let all_masked = source.api_keys.iter().all(|key| is_masked_api_key(key));
            if all_masked && !previous_keys.is_empty() {
                source.api_keys = previous_keys;
                continue;
            }

            // Replace: drop blank entries; if a slot is masked and index maps, keep previous.
            let mut next_keys = Vec::new();
            for (index, key) in source.api_keys.iter().enumerate() {
                let trimmed = key.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if is_masked_api_key(trimmed) {
                    if let Some(previous) = previous_keys.get(index) {
                        if !previous.trim().is_empty() {
                            next_keys.push(previous.clone());
                        }
                    }
                    continue;
                }
                next_keys.push(trimmed.to_string());
            }
            source.api_keys = next_keys;
        }
        merged
    }
}

pub fn mask_api_key(api_key: &str) -> String {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
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
                sources: defaults.web_search.sources.clone(),
            },
        }
    }

    pub fn from_full_config(
        config: &AgentToolsConfig,
        defaults: &AgentToolsConfig,
    ) -> AgentToolsOverrides {
        AgentToolsOverrides {
            version: tool_overrides_version(),
            max_llm_iterations: (config.max_llm_iterations != defaults.max_llm_iterations)
                .then_some(config.max_llm_iterations),
            terminal: TerminalToolOverrides {
                enabled: (config.terminal.enabled != defaults.terminal.enabled)
                    .then_some(config.terminal.enabled),
                permission_mode: (config.terminal.permission_mode
                    != defaults.terminal.permission_mode)
                    .then_some(config.terminal.permission_mode.clone()),
                approval_grants: config.terminal.approval_grants.clone(),
                default_timeout_ms: (config.terminal.default_timeout_ms
                    != defaults.terminal.default_timeout_ms)
                    .then_some(config.terminal.default_timeout_ms),
                allowed_cwd_roots: (config.terminal.allowed_cwd_roots
                    != defaults.terminal.allowed_cwd_roots)
                    .then_some(config.terminal.allowed_cwd_roots.clone()),
            },
            web_search: WebSearchToolOverrides {
                enabled: (config.web_search.enabled != defaults.web_search.enabled)
                    .then_some(config.web_search.enabled),
                max_results: (config.web_search.max_results != defaults.web_search.max_results)
                    .then_some(config.web_search.max_results),
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
            },
        }
    }
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
    fn web_search_defaults_are_disabled_with_builtin_sources() {
        let config = AgentToolsConfig::default();
        assert!(!config.web_search.enabled);
        assert_eq!(config.web_search.max_results, DEFAULT_WEB_SEARCH_MAX_RESULTS);
        assert_eq!(config.web_search.sources.len(), 4);
        assert_eq!(config.web_search.sources[0].id, "tavily");
        assert_eq!(config.web_search.sources[1].id, "exa");
        assert_eq!(config.web_search.sources[2].id, "jina");
        assert_eq!(config.web_search.sources[3].id, "grok");
        assert!(!config.web_search.has_usable_source());
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
        let stored = WebSearchToolConfig {
            enabled: true,
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                enabled: true,
                base_url: None,
                api_keys: vec!["sk-live-secret".to_string()],
                key_statuses: Vec::new(),
                flaresolverr_url: None,
                flaresolverr_proxy_url: None,
                proxy_url: None,
            }],
            ..WebSearchToolConfig::default()
        };

        let preserve = WebSearchToolConfig {
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                enabled: true,
                base_url: None,
                api_keys: vec!["__preserve__".to_string()],
                key_statuses: Vec::new(),
                flaresolverr_url: None,
                flaresolverr_proxy_url: None,
                proxy_url: None,
            }],
            ..stored.clone()
        };
        let merged = stored.merge_secret_update(&preserve);
        assert_eq!(merged.sources[0].api_keys, vec!["sk-live-secret".to_string()]);

        let clear = WebSearchToolConfig {
            sources: vec![WebSearchSourceConfig {
                id: "tavily".to_string(),
                provider: WebSearchProviderKind::Tavily,
                enabled: true,
                base_url: None,
                api_keys: vec![],
                key_statuses: Vec::new(),
                flaresolverr_url: None,
                flaresolverr_proxy_url: None,
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
                enabled: true,
                base_url: None,
                api_keys: vec!["sk-new".to_string()],
                key_statuses: Vec::new(),
                flaresolverr_url: None,
                flaresolverr_proxy_url: None,
                proxy_url: None,
            }],
            ..stored.clone()
        };
        let replaced = stored.merge_secret_update(&replace);
        assert_eq!(replaced.sources[0].api_keys, vec!["sk-new".to_string()]);
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
        assert!(!config.web_search.enabled);
        assert_eq!(config.web_search.sources.len(), 4);
        assert_eq!(config.web_search.sources[3].id, "grok");
    }

    #[test]
    fn redacted_web_search_config_hides_keys() {
        let mut config = AgentToolsConfig::default();
        config.web_search.sources[0].api_keys = vec!["sk-abcdefgh".to_string()];
        let redacted = config.redact_secrets();
        assert_eq!(redacted.web_search.sources[0].api_keys, vec!["****efgh".to_string()]);
        assert_ne!(
            redacted.web_search.sources[0].api_keys[0],
            config.web_search.sources[0].api_keys[0]
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
    AlwaysApprove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub agent_id: uuid::Uuid,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub approval_id: String,
    pub agent_id: uuid::Uuid,
    pub conversation_id: uuid::Uuid,
    pub tool_call_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub runtime: String,
    pub permission: String,
    pub justification: Option<String>,
    pub background: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolResult {
    Completed {
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u64,
        /// Execution runtime actually used for this command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        #[serde(default)]
        output_cursor: u64,
    },
    Running {
        process_id: String,
        message: String,
        #[serde(default)]
        stdout: String,
        #[serde(default)]
        stderr: String,
        #[serde(default)]
        duration_ms: u64,
        /// Execution runtime actually used for this command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        #[serde(default)]
        output_cursor: u64,
    },
    Blocked {
        reason: String,
    },
    PendingApproval {
        command: String,
        approval_id: String,
    },
    Rejected {
        reason: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub process_id: String,
    pub agent_id: uuid::Uuid,
    pub conversation_id: uuid::Uuid,
    pub tool_call_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub status: ProcessStatus,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: u64,
    pub runtime: String,
    pub tty: bool,
    pub output_cursor: u64,
}
