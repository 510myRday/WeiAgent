use serde::{Deserialize, Serialize};

use crate::domain::{
    WebSearchCostTier, WebSearchLatencyTier, WebSearchSourceCapabilities, WebSearchSourceConfig,
    WebSearchSourceRole,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Auto,
    Fast,
    Balanced,
    Deep,
    Custom,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchHitKind {
    #[default]
    WebPage,
    GrokNativeSearchResult,
    GrokModelProvidedUrl,
    XSearchResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_excerpt: Option<String>,
    #[serde(default)]
    pub kind: SearchHitKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiSynthesis {
    pub source_id: String,
    pub provider: String,
    pub query: String,
    pub answer: String,
    pub verification_status: String,
    pub evidence_counted: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProviderSearchOutput {
    pub results: Vec<WebSearchHit>,
    pub ai_syntheses: Vec<AiSynthesis>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSearchRequest {
    pub query: String,
    pub max_results: u32,
    pub mode: SearchMode,
    pub allowed_domains: Vec<String>,
    pub blocked_domains: Vec<String>,
    pub recency_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AggregatedSearchResult {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_excerpt: Option<String>,
    pub evidence_kind: SearchHitKind,
    pub source_ids: Vec<String>,
    pub provider_kinds: Vec<String>,
    pub independence_groups: Vec<String>,
    pub matched_queries: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub corroboration_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchSourceDescriptor {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub provider_kind: String,
    pub evidence_kind: String,
    pub roles: Vec<WebSearchSourceRole>,
    pub strengths: Vec<String>,
    pub limitations: Vec<String>,
    pub capabilities: WebSearchSourceCapabilities,
    pub latency_tier: WebSearchLatencyTier,
    pub cost_tier: WebSearchCostTier,
    pub priority: u16,
    pub deep_enabled: bool,
    pub extract_enabled: bool,
    pub independence_group: String,
    pub enabled: bool,
    pub configured: bool,
    pub health: String,
    pub extract_configured: bool,
    pub extract_health: String,
}

impl SearchSourceDescriptor {
    pub fn from_config(source: &WebSearchSourceConfig) -> Self {
        let can_search = source.can_search();
        let health = if !source.enabled {
            "disabled"
        } else if !can_search {
            "missing_credentials"
        } else if source
            .key_statuses
            .iter()
            .flatten()
            .any(|status| status.status == "normal")
        {
            "healthy"
        } else if source.key_statuses.iter().flatten().any(|status| {
            matches!(
                status.status.as_str(),
                "officially_blocked" | "invalid_key" | "unauthorized" | "quota_exhausted"
            )
        }) {
            "degraded"
        } else {
            "unknown"
        };
        let can_extract = source.can_extract();
        let extract_health = if !source.extract_enabled {
            "disabled"
        } else if !can_extract {
            "missing_credentials"
        } else if source
            .extract_key_statuses
            .iter()
            .flatten()
            .any(|status| status.status == "normal")
        {
            "healthy"
        } else if source.extract_key_statuses.iter().flatten().any(|status| {
            matches!(
                status.status.as_str(),
                "officially_blocked" | "invalid_key" | "unauthorized" | "quota_exhausted"
            )
        }) {
            "degraded"
        } else {
            "unknown"
        };
        Self {
            id: source.id.clone(),
            display_name: source.display_name().to_string(),
            provider: source.provider.as_str().to_string(),
            provider_kind: source.provider.provider_kind().to_string(),
            evidence_kind: source.provider.evidence_kind().to_string(),
            roles: source.roles(),
            strengths: source
                .provider
                .strengths()
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            limitations: source
                .provider
                .limitations()
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            capabilities: source.provider.capabilities(),
            latency_tier: source.provider.latency_tier(),
            cost_tier: source.provider.cost_tier(),
            priority: source.priority,
            deep_enabled: source.deep_enabled,
            extract_enabled: source.extract_enabled,
            independence_group: source.independence_group().to_string(),
            enabled: source.enabled,
            configured: can_search,
            health: health.to_string(),
            extract_configured: can_extract,
            extract_health: extract_health.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExtractRequest {
    pub url: String,
    pub query: String,
    pub hard_page: bool,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderExtractDocument {
    pub requested_url: String,
    pub final_url: String,
    pub title: String,
    pub content: String,
    pub content_format: String,
    pub published_at: Option<String>,
    pub ai_derived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExtractFailure {
    pub requested_url: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderExtractOutput {
    pub documents: Vec<ProviderExtractDocument>,
    pub failures: Vec<ProviderExtractFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceRunSummary {
    pub source_id: String,
    pub provider: String,
    pub query: String,
    pub status: String,
    pub result_count: usize,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearchOutput {
    pub mode: SearchMode,
    pub queries: Vec<String>,
    pub selected_sources: Vec<SearchSourceDescriptor>,
    pub results: Vec<AggregatedSearchResult>,
    pub result_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_syntheses: Vec<AiSynthesis>,
    pub source_runs: Vec<SourceRunSummary>,
    pub citation_instruction: String,
}

#[derive(Debug, Clone)]
pub enum ProviderError {
    Retryable(String),
    Fatal(String),
}

impl ProviderError {
    pub fn message(&self) -> &str {
        match self {
            Self::Retryable(message) | Self::Fatal(message) => message,
        }
    }
}

pub fn sanitize_error_message(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return message.to_string();
    }
    message.replace(api_key, "[redacted]")
}
