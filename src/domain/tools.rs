use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Effective terminal tool configuration assembled from SQLite defaults and agent overrides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalToolConfig {
    #[serde(default = "default_true")]
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
    #[serde(default = "default_true")]
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
    #[serde(default = "default_true")]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProviderKind {
    Tavily,
    Exa,
    Jina,
    Grok,
    Firecrawl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchSourceRole {
    GeneralSearch,
    SemanticResearch,
    CurrentNews,
    OfficialDocs,
    TechnicalResearch,
    AcademicResearch,
    FactChecking,
    MarketResearch,
    ProductResearch,
    MultilingualSearch,
    AiResearch,
    SocialSearch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchLatencyTier {
    Fast,
    Medium,
    Slow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchCostTier {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchSourceCapabilities {
    pub domain_filter: bool,
    pub recency_filter: bool,
    pub native_citations: bool,
    pub ai_synthesis: bool,
    pub social_results: bool,
    pub url_extract: bool,
    pub batch_extract: bool,
    pub javascript_rendering: bool,
    pub pdf_extract: bool,
    pub ai_derived_extract: bool,
}

impl WebSearchProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::Exa => "exa",
            Self::Jina => "jina",
            Self::Grok => "grok",
            Self::Firecrawl => "firecrawl",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            Self::Tavily => "https://api.tavily.com",
            Self::Exa => "https://api.exa.ai",
            Self::Jina => "https://s.jina.ai",
            Self::Grok => "https://grok.com",
            Self::Firecrawl => "https://api.firecrawl.dev/v2",
        }
    }

    pub fn default_extract_base_url(&self) -> &'static str {
        match self {
            Self::Tavily => "https://api.tavily.com",
            Self::Exa => "https://api.exa.ai",
            Self::Jina => "https://r.jina.ai",
            Self::Grok => "https://grok.com",
            Self::Firecrawl => "https://api.firecrawl.dev/v2",
        }
    }

    pub fn default_display_name(&self) -> &'static str {
        match self {
            Self::Tavily => "Tavily",
            Self::Exa => "Exa",
            Self::Jina => "Jina Search",
            Self::Grok => "Grok Research",
            Self::Firecrawl => "Firecrawl",
        }
    }

    pub fn supports_anonymous_search(&self) -> bool {
        matches!(self, Self::Jina | Self::Firecrawl)
    }

    pub fn supports_anonymous_extract(&self) -> bool {
        matches!(self, Self::Jina | Self::Firecrawl)
    }

    pub fn provider_kind(&self) -> &'static str {
        match self {
            Self::Tavily => "search_index",
            Self::Exa | Self::Jina => "semantic_search",
            Self::Grok => "ai_research",
            Self::Firecrawl => "search_and_extract",
        }
    }

    pub fn evidence_kind(&self) -> &'static str {
        match self {
            Self::Grok => "ai_synthesis_and_candidates",
            Self::Firecrawl => "web_evidence_with_extracted_content",
            _ => "web_evidence_candidates",
        }
    }

    pub fn default_roles(&self) -> Vec<WebSearchSourceRole> {
        match self {
            Self::Tavily => vec![
                WebSearchSourceRole::GeneralSearch,
                WebSearchSourceRole::CurrentNews,
                WebSearchSourceRole::FactChecking,
                WebSearchSourceRole::MarketResearch,
                WebSearchSourceRole::ProductResearch,
            ],
            Self::Exa => vec![
                WebSearchSourceRole::SemanticResearch,
                WebSearchSourceRole::OfficialDocs,
                WebSearchSourceRole::TechnicalResearch,
                WebSearchSourceRole::AcademicResearch,
                WebSearchSourceRole::FactChecking,
                WebSearchSourceRole::ProductResearch,
            ],
            Self::Jina => vec![
                WebSearchSourceRole::GeneralSearch,
                WebSearchSourceRole::SemanticResearch,
                WebSearchSourceRole::OfficialDocs,
                WebSearchSourceRole::TechnicalResearch,
                WebSearchSourceRole::FactChecking,
                WebSearchSourceRole::MultilingualSearch,
            ],
            Self::Grok => vec![
                WebSearchSourceRole::AiResearch,
                WebSearchSourceRole::CurrentNews,
                WebSearchSourceRole::SocialSearch,
                WebSearchSourceRole::FactChecking,
                WebSearchSourceRole::MarketResearch,
                WebSearchSourceRole::ProductResearch,
            ],
            Self::Firecrawl => vec![
                WebSearchSourceRole::GeneralSearch,
                WebSearchSourceRole::CurrentNews,
                WebSearchSourceRole::OfficialDocs,
                WebSearchSourceRole::TechnicalResearch,
                WebSearchSourceRole::AcademicResearch,
                WebSearchSourceRole::FactChecking,
                WebSearchSourceRole::MarketResearch,
                WebSearchSourceRole::ProductResearch,
                WebSearchSourceRole::MultilingualSearch,
            ],
        }
    }

    pub fn capabilities(&self) -> WebSearchSourceCapabilities {
        match self {
            Self::Tavily => WebSearchSourceCapabilities {
                domain_filter: true,
                recency_filter: true,
                native_citations: true,
                ai_synthesis: false,
                social_results: false,
                url_extract: true,
                batch_extract: true,
                javascript_rendering: true,
                pdf_extract: true,
                ai_derived_extract: false,
            },
            Self::Exa => WebSearchSourceCapabilities {
                domain_filter: true,
                recency_filter: true,
                native_citations: true,
                ai_synthesis: false,
                social_results: false,
                url_extract: true,
                batch_extract: true,
                javascript_rendering: true,
                pdf_extract: true,
                ai_derived_extract: false,
            },
            Self::Jina => WebSearchSourceCapabilities {
                domain_filter: true,
                recency_filter: false,
                native_citations: true,
                ai_synthesis: false,
                social_results: false,
                url_extract: true,
                batch_extract: false,
                javascript_rendering: true,
                pdf_extract: true,
                ai_derived_extract: false,
            },
            Self::Grok => WebSearchSourceCapabilities {
                domain_filter: false,
                recency_filter: true,
                native_citations: true,
                ai_synthesis: true,
                social_results: true,
                url_extract: true,
                batch_extract: false,
                javascript_rendering: true,
                pdf_extract: false,
                ai_derived_extract: true,
            },
            Self::Firecrawl => WebSearchSourceCapabilities {
                domain_filter: true,
                recency_filter: true,
                native_citations: true,
                ai_synthesis: false,
                social_results: false,
                url_extract: true,
                batch_extract: true,
                javascript_rendering: true,
                pdf_extract: true,
                ai_derived_extract: false,
            },
        }
    }

    pub fn latency_tier(&self) -> WebSearchLatencyTier {
        match self {
            Self::Tavily | Self::Exa => WebSearchLatencyTier::Fast,
            Self::Jina | Self::Firecrawl => WebSearchLatencyTier::Medium,
            Self::Grok => WebSearchLatencyTier::Slow,
        }
    }

    pub fn cost_tier(&self) -> WebSearchCostTier {
        match self {
            Self::Jina => WebSearchCostTier::Low,
            Self::Tavily | Self::Exa | Self::Firecrawl => WebSearchCostTier::Medium,
            Self::Grok => WebSearchCostTier::High,
        }
    }

    pub fn strengths(&self) -> &'static [&'static str] {
        match self {
            Self::Tavily => &["broad web coverage", "current events", "clean snippets"],
            Self::Exa => &[
                "semantic discovery",
                "technical research",
                "related documents",
            ],
            Self::Jina => &[
                "multilingual search",
                "page-oriented results",
                "simple retrieval",
            ],
            Self::Grok => &[
                "research synthesis",
                "query expansion",
                "X/social discovery",
            ],
            Self::Firecrawl => &[
                "search with page extraction",
                "news, GitHub, research, and PDF discovery",
                "JavaScript-ready page content",
            ],
        }
    }

    pub fn limitations(&self) -> &'static [&'static str] {
        match self {
            Self::Tavily => &["snippets are evidence candidates, not full pages"],
            Self::Exa => &["semantic ranking may miss exact-keyword matches"],
            Self::Jina => &["limited native recency controls"],
            Self::Grok => &[
                "generated synthesis is unverified",
                "model-provided URLs require extraction before citation",
                "higher latency and cost",
            ],
            Self::Firecrawl => &[
                "deep-mode page scraping increases latency and credit usage",
                "upstream search coverage may overlap other general indexes",
                "image results are outside the unified text-result schema",
            ],
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSearchKeyOperation {
    Search,
    Extract,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WebSearchKeyStatusUpdate {
    pub operation: WebSearchKeyOperation,
    pub source_id: String,
    pub key_index: usize,
    pub api_key: String,
    pub record: WebSearchKeyStatusRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchSourceConfig {
    pub id: String,
    pub provider: WebSearchProviderKind,
    #[serde(default)]
    pub display_name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub deep_enabled: bool,
    #[serde(default = "default_true")]
    pub extract_enabled: bool,
    #[serde(default = "default_web_search_source_priority")]
    pub priority: u16,
    #[serde(default)]
    pub independence_group: String,
    #[serde(default)]
    pub roles: Vec<WebSearchSourceRole>,
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// SHA-256 fingerprints of keys confirmed as officially blocked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_api_key_hashes: Vec<String>,
    /// Status slots aligned with api_keys. Null means this key has not been checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_statuses: Vec<Option<WebSearchKeyStatusRecord>>,
    /// Extraction status slots aligned with api_keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract_key_statuses: Vec<Option<WebSearchKeyStatusRecord>>,
    /// Upstream proxy for Grok/CF traffic. Supports HTTP and SOCKS proxy URLs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::domain::ip_proxies::serialize_optional_proxy_url"
    )]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebContentExtractionConfig {
    #[serde(default = "default_true")]
    pub remote_enabled: bool,
    #[serde(default = "default_web_extract_max_remote_attempts")]
    pub max_remote_attempts_per_url: u32,
    #[serde(default = "default_true")]
    pub allow_grok_fallback: bool,
}

fn default_web_extract_max_remote_attempts() -> u32 {
    DEFAULT_WEB_EXTRACT_MAX_REMOTE_ATTEMPTS
}

impl Default for WebContentExtractionConfig {
    fn default() -> Self {
        Self {
            remote_enabled: true,
            max_remote_attempts_per_url: DEFAULT_WEB_EXTRACT_MAX_REMOTE_ATTEMPTS,
            allow_grok_fallback: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_web_search_source_priority() -> u16 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_search_max_results")]
    pub max_results: u32,
    #[serde(default = "default_web_search_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub key_selection: WebSearchKeySelection,
    #[serde(default)]
    pub extraction: WebContentExtractionConfig,
    #[serde(default = "default_web_search_sources")]
    pub sources: Vec<WebSearchSourceConfig>,
}

fn default_web_search_sources() -> Vec<WebSearchSourceConfig> {
    vec![
        default_web_search_source("tavily", WebSearchProviderKind::Tavily, 10),
        default_web_search_source("exa", WebSearchProviderKind::Exa, 20),
        default_web_search_source("jina", WebSearchProviderKind::Jina, 30),
        default_web_search_source("grok", WebSearchProviderKind::Grok, 40),
        default_web_search_source("firecrawl", WebSearchProviderKind::Firecrawl, 50),
    ]
}

fn default_web_search_source(
    id: &str,
    provider: WebSearchProviderKind,
    priority: u16,
) -> WebSearchSourceConfig {
    let enabled = provider.supports_anonymous_search();
    let roles = provider.default_roles();
    WebSearchSourceConfig {
        id: id.to_string(),
        display_name: provider.default_display_name().to_string(),
        independence_group: provider.as_str().to_string(),
        roles,
        provider,
        enabled,
        deep_enabled: true,
        extract_enabled: true,
        priority,
        api_keys: Vec::new(),
        blocked_api_key_hashes: Vec::new(),
        key_statuses: Vec::new(),
        extract_key_statuses: Vec::new(),
        proxy_url: None,
    }
}

impl Default for WebSearchToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
            timeout_ms: DEFAULT_WEB_SEARCH_TIMEOUT_MS,
            key_selection: WebSearchKeySelection::Sequential,
            extraction: WebContentExtractionConfig::default(),
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

    pub fn max_remote_extract_attempts(&self) -> usize {
        self.extraction
            .max_remote_attempts_per_url
            .clamp(1, MAX_WEB_EXTRACT_REMOTE_ATTEMPTS) as usize
    }

    pub(crate) fn ensure_builtin_sources(&mut self, previous: Option<&Self>) -> bool {
        let mut changed = false;
        for default_source in default_web_search_sources() {
            if self
                .sources
                .iter()
                .any(|source| source.id == default_source.id)
            {
                continue;
            }
            let source = previous
                .and_then(|config| {
                    config
                        .sources
                        .iter()
                        .find(|source| source.id == default_source.id)
                })
                .cloned()
                .unwrap_or(default_source);
            self.sources.push(source);
            changed = true;
        }
        changed
    }

    pub(crate) fn disable_unavailable_sources(&mut self) -> bool {
        let mut changed = false;
        for source in &mut self.sources {
            if source.enabled && !source.can_search() {
                source.enabled = false;
                changed = true;
            }
        }
        changed
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
            source.blocked_api_key_hashes.clear();
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
            let incoming_blocked_hashes = std::mem::take(&mut source.blocked_api_key_hashes);
            let previous_source = self
                .sources
                .iter()
                .find(|existing| existing.id == source.id);
            let previous_keys = previous_source
                .map(|existing| existing.api_keys.clone())
                .unwrap_or_default();
            source.blocked_api_key_hashes = previous_source
                .map(|existing| existing.blocked_api_key_hashes.clone())
                .unwrap_or_default();
            for hash in incoming_blocked_hashes {
                if is_api_key_fingerprint(&hash)
                    && !source
                        .blocked_api_key_hashes
                        .iter()
                        .any(|value| value == &hash)
                {
                    source.blocked_api_key_hashes.push(hash);
                }
            }
            if let Some(previous_source) = previous_source {
                for (key_index, status) in previous_source.key_statuses.iter().enumerate() {
                    if status
                        .as_ref()
                        .is_some_and(|record| record.status == "officially_blocked")
                        && let Some(api_key) = previous_source.api_keys.get(key_index)
                    {
                        source.remember_blocked_api_key(api_key);
                    }
                }
            }

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
                    if let Some(previous) = previous_keys.get(index)
                        && !previous.trim().is_empty()
                    {
                        next_keys.push(previous.clone());
                    }
                    continue;
                }
                if source.is_api_key_blocked(trimmed) {
                    continue;
                }
                next_keys.push(trimmed.to_string());
            }
            source.api_keys = next_keys;
        }
        merged
    }
}

impl WebSearchSourceConfig {
    pub fn display_name(&self) -> &str {
        let value = self.display_name.trim();
        if value.is_empty() {
            self.provider.default_display_name()
        } else {
            value
        }
    }

    pub fn independence_group(&self) -> &str {
        let value = self.independence_group.trim();
        if value.is_empty() {
            self.provider.as_str()
        } else {
            value
        }
    }

    pub fn roles(&self) -> Vec<WebSearchSourceRole> {
        if self.roles.is_empty() {
            self.provider.default_roles()
        } else {
            self.roles.clone()
        }
    }

    pub fn has_available_key(&self) -> bool {
        self.api_keys
            .iter()
            .any(|key| !key.trim().is_empty() && !self.is_api_key_blocked(key))
    }

    pub fn can_search(&self) -> bool {
        self.provider.supports_anonymous_search() || self.has_available_key()
    }

    pub fn can_extract(&self) -> bool {
        self.provider.supports_anonymous_extract() || self.has_available_key()
    }
}

impl WebSearchSourceConfig {
    pub fn is_api_key_blocked(&self, api_key: &str) -> bool {
        let hash = api_key_fingerprint(api_key);
        !hash.is_empty()
            && self
                .blocked_api_key_hashes
                .iter()
                .any(|value| value == &hash)
    }

    pub fn remember_blocked_api_key(&mut self, api_key: &str) -> bool {
        let hash = api_key_fingerprint(api_key);
        if hash.is_empty()
            || self
                .blocked_api_key_hashes
                .iter()
                .any(|value| value == &hash)
        {
            return false;
        }
        self.blocked_api_key_hashes.push(hash);
        true
    }
}

pub fn api_key_fingerprint(api_key: &str) -> String {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_api_key_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
                sources: defaults.web_search.sources.clone(),
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
            },
            task_planning: TaskPlanningToolOverrides {
                enabled: Some(config.task_planning.enabled),
                progress_interval: Some(config.task_planning.progress_interval),
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    PendingApproval {
        command: String,
        approval_id: String,
    },
    Rejected {
        reason: String,
    },
    Interrupted {
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
