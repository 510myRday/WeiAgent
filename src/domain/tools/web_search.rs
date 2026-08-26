use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_WEB_EXTRACT_MAX_REMOTE_ATTEMPTS, DEFAULT_WEB_SEARCH_MAX_RESULTS,
    DEFAULT_WEB_SEARCH_TIMEOUT_MS, MAX_WEB_EXTRACT_REMOTE_ATTEMPTS, MAX_WEB_SEARCH_RESULTS,
    MAX_WEB_SEARCH_TIMEOUT_MS, api_key_fingerprint, default_web_search_max_results,
    default_web_search_timeout_ms, is_api_key_fingerprint, is_masked_api_key, mask_api_key,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProviderKind {
    Tavily,
    Exa,
    Jina,
    Grok,
    Firecrawl,
    Keenable,
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
            Self::Keenable => "keenable",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            Self::Tavily => "https://api.tavily.com",
            Self::Exa => "https://api.exa.ai",
            Self::Jina => "https://s.jina.ai",
            Self::Grok => "https://grok.com",
            Self::Firecrawl => "https://api.firecrawl.dev/v2",
            Self::Keenable => "https://api.keenable.ai/v1",
        }
    }

    pub fn default_extract_base_url(&self) -> &'static str {
        match self {
            Self::Tavily => "https://api.tavily.com",
            Self::Exa => "https://api.exa.ai",
            Self::Jina => "https://r.jina.ai",
            Self::Grok => "https://grok.com",
            Self::Firecrawl => "https://api.firecrawl.dev/v2",
            Self::Keenable => "https://api.keenable.ai/v1",
        }
    }

    pub fn default_display_name(&self) -> &'static str {
        match self {
            Self::Tavily => "Tavily",
            Self::Exa => "Exa",
            Self::Jina => "Jina Search",
            Self::Grok => "Grok Research",
            Self::Firecrawl => "Firecrawl",
            Self::Keenable => "Keenable",
        }
    }

    pub fn supports_anonymous_search(&self) -> bool {
        matches!(self, Self::Jina | Self::Firecrawl | Self::Keenable)
    }

    pub fn supports_anonymous_extract(&self) -> bool {
        matches!(self, Self::Jina | Self::Firecrawl | Self::Keenable)
    }

    pub fn provider_kind(&self) -> &'static str {
        match self {
            Self::Tavily => "search_index",
            Self::Exa | Self::Jina => "semantic_search",
            Self::Grok => "ai_research",
            Self::Firecrawl => "search_and_extract",
            Self::Keenable => "search_and_extract",
        }
    }

    pub fn evidence_kind(&self) -> &'static str {
        match self {
            Self::Grok => "ai_synthesis_and_candidates",
            Self::Firecrawl => "web_evidence_with_extracted_content",
            Self::Keenable => "web_evidence_with_extracted_content",
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
            Self::Keenable => vec![
                WebSearchSourceRole::GeneralSearch,
                WebSearchSourceRole::CurrentNews,
                WebSearchSourceRole::OfficialDocs,
                WebSearchSourceRole::TechnicalResearch,
                WebSearchSourceRole::FactChecking,
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
            Self::Keenable => WebSearchSourceCapabilities {
                domain_filter: true,
                recency_filter: true,
                native_citations: true,
                ai_synthesis: false,
                social_results: false,
                url_extract: true,
                batch_extract: false,
                javascript_rendering: false,
                pdf_extract: false,
                ai_derived_extract: false,
            },
        }
    }

    pub fn latency_tier(&self) -> WebSearchLatencyTier {
        match self {
            Self::Tavily | Self::Exa => WebSearchLatencyTier::Fast,
            Self::Jina | Self::Firecrawl | Self::Keenable => WebSearchLatencyTier::Medium,
            Self::Grok => WebSearchLatencyTier::Slow,
        }
    }

    pub fn cost_tier(&self) -> WebSearchCostTier {
        match self {
            Self::Jina => WebSearchCostTier::Low,
            Self::Tavily | Self::Exa | Self::Firecrawl | Self::Keenable => {
                WebSearchCostTier::Medium
            }
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
            Self::Keenable => &[
                "web search with indexed page retrieval",
                "current and multilingual web discovery",
                "simple REST API integration",
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
            Self::Keenable => &[
                "public access is rate limited per egress IP",
                "deep page extraction is limited to indexed content unless live fetch is requested",
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
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub deep_enabled: bool,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub extract_enabled: bool,
    #[serde(default = "default_web_search_source_priority")]
    pub priority: u16,
    #[serde(default)]
    pub independence_group: String,
    #[serde(default)]
    pub roles: Vec<WebSearchSourceRole>,
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// Stable, non-secret identifiers aligned with api_keys in redacted API responses.
    /// They are used to reconcile masked keys after a deletion or reorder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_key_fingerprints: Vec<String>,
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
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub remote_enabled: bool,
    #[serde(default = "default_web_extract_max_remote_attempts")]
    pub max_remote_attempts_per_url: u32,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
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

fn default_web_search_source_priority() -> u16 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchToolConfig {
    #[serde(default = "crate::domain::serde_defaults::default_true")]
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
        default_web_search_source("keenable", WebSearchProviderKind::Keenable, 10),
        default_web_search_source("tavily", WebSearchProviderKind::Tavily, 20),
        default_web_search_source("exa", WebSearchProviderKind::Exa, 30),
        default_web_search_source("jina", WebSearchProviderKind::Jina, 40),
        default_web_search_source("grok", WebSearchProviderKind::Grok, 50),
        default_web_search_source("firecrawl", WebSearchProviderKind::Firecrawl, 60),
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
        api_key_fingerprints: Vec::new(),
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
            let insert_first = default_source.id == "keenable";
            let source = previous
                .and_then(|config| {
                    config
                        .sources
                        .iter()
                        .find(|source| source.id == default_source.id)
                })
                .cloned()
                .unwrap_or(default_source);
            if insert_first {
                self.sources.insert(0, source);
            } else {
                self.sources.push(source);
            }
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
            source.api_key_fingerprints = source
                .api_keys
                .iter()
                .map(|key| api_key_fingerprint(key))
                .filter(|fingerprint| !fingerprint.is_empty())
                .collect();
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
            let incoming_fingerprints = source.api_key_fingerprints.clone();
            let previous_keys_by_fingerprint = previous_keys
                .iter()
                .map(|key| (api_key_fingerprint(key), key.clone()))
                .collect::<std::collections::HashMap<_, _>>();
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
                source.api_key_fingerprints.clear();
                continue;
            }

            let has_aligned_fingerprints = incoming_fingerprints.len() == source.api_keys.len();
            let mut next_keys = Vec::new();
            for (index, key) in source.api_keys.iter().enumerate() {
                let trimmed = key.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if is_masked_api_key(trimmed) {
                    let previous = if has_aligned_fingerprints {
                        incoming_fingerprints
                            .get(index)
                            .and_then(|fingerprint| previous_keys_by_fingerprint.get(fingerprint))
                    } else {
                        previous_keys.get(index)
                    };
                    if let Some(previous) = previous {
                        if !previous.trim().is_empty() {
                            next_keys.push(previous.clone());
                        }
                    }
                    continue;
                }
                if source.is_api_key_blocked(trimmed) {
                    continue;
                }
                next_keys.push(trimmed.to_string());
            }
            source.api_keys = next_keys;
            source.api_key_fingerprints.clear();
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
