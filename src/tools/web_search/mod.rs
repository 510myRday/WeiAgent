use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};

use crate::domain::tools::{
    MAX_WEB_SEARCH_QUERIES, MAX_WEB_SEARCH_QUERY_CHARS, MAX_WEB_SEARCH_RESULTS,
    WebSearchKeyOperation, WebSearchKeyStatusUpdate,
};
use crate::domain::{
    ToolResult, WebSearchKeyStatusRecord, WebSearchProviderKind, WebSearchSourceConfig,
    WebSearchSourceRole, WebSearchToolConfig,
};

use self::key_pool::key_attempt_order;
use self::providers::{dispatch_search, support::truncate};
use self::types::{
    AggregatedSearchResult, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind, SearchMode,
    SearchSourceDescriptor, SourceRunSummary, WebSearchOutput, merge_status_update,
    sanitize_error_message,
};

pub mod client_cache;
pub mod evidence;
pub mod key_pool;
pub mod providers;
pub mod types;

const MAX_SELECTED_SOURCES: usize = 6;
const MAX_FILTER_DOMAINS: usize = 20;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchArgs {
    pub query: String,
    #[serde(default)]
    pub additional_queries: Vec<String>,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub source_roles: Vec<WebSearchSourceRole>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub max_results_per_source: Option<u32>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    #[serde(default)]
    pub recency_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebSearchSourcesOutput {
    pub sources: Vec<SearchSourceDescriptor>,
    pub routing_modes: Vec<&'static str>,
    pub guidance: &'static str,
}

pub fn describe_web_search_sources(config: &WebSearchToolConfig) -> WebSearchSourcesOutput {
    let mut sources = config
        .sources
        .iter()
        .map(SearchSourceDescriptor::from_config)
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    WebSearchSourcesOutput {
        sources,
        routing_modes: vec!["auto", "fast", "balanced", "deep", "custom"],
        guidance: "Use source_ids only when a documented source characteristic materially fits the task. AI syntheses are hypotheses, not independent evidence.",
    }
}

pub(crate) struct WebSearchExecution {
    pub result: ToolResult,
    pub status_updates: Vec<WebSearchKeyStatusUpdate>,
}

pub(crate) async fn execute_web_search_with_status_updates(
    args: WebSearchArgs,
    config: &WebSearchToolConfig,
) -> WebSearchExecution {
    let started = Instant::now();
    let queries = match normalize_queries(&args) {
        Ok(queries) => queries,
        Err(message) => return execution_error(message),
    };
    if args.allowed_domains.len() > MAX_FILTER_DOMAINS
        || args.blocked_domains.len() > MAX_FILTER_DOMAINS
    {
        return execution_error(format!(
            "allowed_domains and blocked_domains each support at most {MAX_FILTER_DOMAINS} entries"
        ));
    }
    if args.recency_days == Some(0) {
        return execution_error("recency_days must be at least 1".to_string());
    }

    let actual_mode = resolve_mode(&args.mode, &queries);
    let selected_sources = match select_sources(config, &args, &actual_mode) {
        Ok(sources) => sources,
        Err(message) => return execution_error(message),
    };
    if selected_sources.is_empty() {
        return execution_error("no enabled web search source matches this request".to_string());
    }

    let max_results = args
        .max_results_per_source
        .unwrap_or_else(|| config.max_results())
        .clamp(1, MAX_WEB_SEARCH_RESULTS);
    let allowed_domains = normalize_domains(&args.allowed_domains);
    let blocked_domains = normalize_domains(&args.blocked_domains);
    let batch_options = SourceBatchOptions {
        max_results,
        allowed_domains: &allowed_domains,
        blocked_domains: &blocked_domains,
        recency_days: args.recency_days,
        mode: &actual_mode,
    };
    let executions = if actual_mode == SearchMode::Fast {
        let mut attempts = Vec::new();
        for source in &selected_sources {
            let batch = execute_source_batch(
                std::slice::from_ref(source),
                &queries,
                &batch_options,
                config,
            )
            .await;
            let succeeded = batch.iter().any(|execution| {
                execution.output.as_ref().is_some_and(|output| {
                    !output.results.is_empty()
                })
            });
            attempts.extend(batch);
            if succeeded {
                break;
            }
        }
        attempts
    } else {
        execute_source_batch(&selected_sources, &queries, &batch_options, config).await
    };

    let mut status_update_map = BTreeMap::new();
    let mut source_runs = Vec::new();
    let mut provider_outputs = Vec::new();
    let mut attempted_sources = Vec::new();
    let mut attempted_source_ids = HashSet::new();
    for execution in executions {
        if attempted_source_ids.insert(execution.source.id.clone()) {
            attempted_sources.push(execution.source.clone());
        }
        for update in execution.status_updates {
            merge_status_update(&mut status_update_map, update);
        }
        source_runs.push(execution.summary);
        if let Some(output) = execution.output {
            provider_outputs.push((execution.source, execution.query, output));
        }
    }

    let results = aggregate_results(&provider_outputs, &allowed_domains, &blocked_domains);
    let selected_descriptors = attempted_sources
        .iter()
        .map(SearchSourceDescriptor::from_config)
        .collect::<Vec<_>>();
    let output = WebSearchOutput {
        mode: actual_mode,
        queries,
        result_count: results.len(),
        results,
        selected_sources: selected_descriptors,
        source_runs,
        citation_instruction: "Extract and verify the most relevant original pages before citing. Place each Markdown link next to the claim it supports. AI syntheses are unverified and do not count as independent evidence.".to_string(),
    };
    let status_updates = status_update_map.into_values().collect::<Vec<_>>();

    match serde_json::to_string_pretty(&output) {
        Ok(stdout) => WebSearchExecution {
            result: ToolResult::Completed {
                stdout,
                stderr: String::new(),
                exit_code: 0,
                duration_ms: started.elapsed().as_millis() as u64,
                runtime: Some("builtin_web_search".to_string()),
                cwd: None,
                output_cursor: 0,
                structured_content: None,
                is_error: false,
            },
            status_updates,
        },
        Err(error) => WebSearchExecution {
            result: ToolResult::Error {
                message: format!("failed to serialize search results: {error}"),
            },
            status_updates,
        },
    }
}

struct SourceBatchOptions<'a> {
    max_results: u32,
    allowed_domains: &'a [String],
    blocked_domains: &'a [String],
    recency_days: Option<u32>,
    mode: &'a SearchMode,
}

async fn execute_source_batch(
    sources: &[WebSearchSourceConfig],
    queries: &[String],
    options: &SourceBatchOptions<'_>,
    config: &WebSearchToolConfig,
) -> Vec<SourceExecution> {
    let selected_keys = sources
        .iter()
        .map(|source| {
            (
                source.id.clone(),
                select_source_key(source, config, WebSearchKeyOperation::Search),
            )
        })
        .collect::<HashMap<_, _>>();
    let requests = queries
        .iter()
        .flat_map(|query| {
            let selected_keys = &selected_keys;
            let mode = options.mode.clone();
            sources.iter().cloned().map(move |source| {
                let selected_key = selected_keys.get(&source.id).cloned().flatten();
                (
                    source,
                    ProviderSearchRequest {
                        query: query.clone(),
                        max_results: options.max_results,
                        mode: mode.clone(),
                        allowed_domains: options.allowed_domains.to_vec(),
                        blocked_domains: options.blocked_domains.to_vec(),
                        recency_days: options.recency_days,
                    },
                    selected_key,
                )
            })
        })
        .collect::<Vec<_>>();
    join_all(requests.into_iter().map(|(source, request, selected_key)| {
        execute_source_query_with_timeout(source, request, selected_key, config)
    }))
    .await
}

async fn execute_source_query_with_timeout(
    source: WebSearchSourceConfig,
    request: ProviderSearchRequest,
    selected_key: Option<(usize, String)>,
    config: &WebSearchToolConfig,
) -> SourceExecution {
    let deadline = Duration::from_millis(config.timeout_ms());
    let timeout_source = source.clone();
    let timeout_query = request.query.clone();
    let timeout_key = selected_key.clone();
    let started = Instant::now();
    match tokio::time::timeout(
        deadline,
        execute_source_query(source, request, selected_key, config),
    )
    .await
    {
        Ok(execution) => execution,
        Err(_) => {
            let status_updates = timeout_key
                .map(|(key_index, api_key)| {
                    vec![key_status_update(
                        &timeout_source.id,
                        key_index,
                        &api_key,
                        "network_error".to_string(),
                        "source request timed out".to_string(),
                    )]
                })
                .unwrap_or_default();
            source_failure(
                timeout_source,
                timeout_query,
                started,
                "timed_out",
                format!("source request exceeded {} ms", deadline.as_millis()),
                status_updates,
            )
        }
    }
}

pub(crate) fn select_source_key(
    source: &WebSearchSourceConfig,
    config: &WebSearchToolConfig,
    operation: WebSearchKeyOperation,
) -> Option<(usize, String)> {
    let keys = source
        .api_keys
        .iter()
        .enumerate()
        .map(|(key_index, key)| (key_index, key.trim().to_string()))
        .filter(|(_, key)| !key.is_empty() && !source.is_api_key_blocked(key))
        .collect::<Vec<_>>();
    let index = key_attempt_order(&source.id, keys.len(), &config.key_selection)
        .into_iter()
        .min_by_key(|index| key_health_rank(source, keys[*index].0, operation))?;
    keys.get(index).cloned()
}

fn execution_error(message: String) -> WebSearchExecution {
    WebSearchExecution {
        result: ToolResult::Error { message },
        status_updates: Vec::new(),
    }
}

fn normalize_queries(args: &WebSearchArgs) -> Result<Vec<String>, String> {
    let mut raw = Vec::with_capacity(1 + args.additional_queries.len());
    raw.push(args.query.as_str());
    raw.extend(args.additional_queries.iter().map(String::as_str));
    if raw.len() > MAX_WEB_SEARCH_QUERIES {
        return Err(format!(
            "web_search supports at most {MAX_WEB_SEARCH_QUERIES} queries per call"
        ));
    }
    let mut seen = HashSet::new();
    let mut queries = Vec::new();
    for query in raw {
        let query = query.trim();
        if query.is_empty() {
            continue;
        }
        if query.chars().count() > MAX_WEB_SEARCH_QUERY_CHARS {
            return Err(format!(
                "each query must be at most {MAX_WEB_SEARCH_QUERY_CHARS} characters"
            ));
        }
        let key = query.to_lowercase();
        if seen.insert(key) {
            queries.push(query.to_string());
        }
    }
    if queries.is_empty() {
        return Err("query must not be empty".to_string());
    }
    Ok(queries)
}

fn resolve_mode(requested: &SearchMode, queries: &[String]) -> SearchMode {
    if requested != &SearchMode::Auto {
        return requested.clone();
    }
    let query = queries.join(" ").to_lowercase();
    if [
        "deep research",
        "comprehensive",
        "in-depth",
        "深度调研",
        "深入调研",
        "全面调研",
        "系统调研",
    ]
    .iter()
    .any(|marker| query.contains(marker))
    {
        SearchMode::Deep
    } else if [
        "latest", "current", "today", "news", "compare", "最新", "当前", "今天", "新闻", "对比",
        "比较",
    ]
    .iter()
    .any(|marker| query.contains(marker))
    {
        SearchMode::Balanced
    } else {
        SearchMode::Fast
    }
}

fn select_sources(
    config: &WebSearchToolConfig,
    args: &WebSearchArgs,
    mode: &SearchMode,
) -> Result<Vec<WebSearchSourceConfig>, String> {
    if mode == &SearchMode::Custom && args.source_ids.is_empty() && args.source_roles.is_empty() {
        return Err("custom mode requires source_ids or source_roles".to_string());
    }
    let requested_ids = args
        .source_ids
        .iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    let requested_roles = args.source_roles.iter().cloned().collect::<HashSet<_>>();
    let inferred_roles = if requested_roles.is_empty() {
        infer_source_roles(args)
    } else {
        HashSet::new()
    };
    let mut candidates = config
        .sources
        .iter()
        .filter(|source| source.enabled && source.can_search())
        .filter(|source| {
            requested_ids.is_empty() || requested_ids.contains(&source.id.trim().to_lowercase())
        })
        .filter(|source| {
            requested_roles.is_empty()
                || source
                    .roles()
                    .iter()
                    .any(|role| requested_roles.contains(role))
        })
        .cloned()
        .collect::<Vec<_>>();
    if !requested_ids.is_empty() {
        let found = candidates
            .iter()
            .map(|source| source.id.trim().to_lowercase())
            .collect::<HashSet<_>>();
        let missing = requested_ids
            .difference(&found)
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "unknown or disabled web search source_ids: {}",
                missing.join(", ")
            ));
        }
    }
    candidates.sort_by(|left, right| {
        let left_fit = left
            .roles()
            .iter()
            .filter(|role| inferred_roles.contains(role))
            .count();
        let right_fit = right
            .roles()
            .iter()
            .filter(|role| inferred_roles.contains(role))
            .count();
        right_fit
            .cmp(&left_fit)
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.id.cmp(&right.id))
    });

    if !requested_ids.is_empty() || mode == &SearchMode::Custom {
        candidates.truncate(MAX_SELECTED_SOURCES);
        return Ok(candidates);
    }

    let limit = match mode {
        SearchMode::Fast => 3,
        SearchMode::Balanced | SearchMode::Auto => 3,
        SearchMode::Deep => MAX_SELECTED_SOURCES,
        SearchMode::Custom => MAX_SELECTED_SOURCES,
    };
    let mut groups = HashSet::new();
    let mut selected = Vec::new();
    for source in candidates {
        if mode == &SearchMode::Deep && !source.deep_enabled {
            continue;
        }
        if groups.insert(source.independence_group().to_lowercase()) {
            selected.push(source);
        }
        if selected.len() >= limit {
            break;
        }
    }
    Ok(selected)
}

/// Query markers → source roles they imply. Data only; the loop below applies it.
const ROLE_MARKERS: &[(&[&str], &[WebSearchSourceRole])] = &[
    (
        &[
            "latest", "current", "today", "news", "breaking", "最新", "当前", "今天", "新闻",
            "近期",
        ],
        &[WebSearchSourceRole::CurrentNews],
    ),
    (
        &[
            "verify",
            "fact check",
            "true or false",
            "rumor",
            "misinformation",
            "核实",
            "核验",
            "事实核查",
            "真假",
            "谣言",
            "是否属实",
        ],
        &[WebSearchSourceRole::FactChecking],
    ),
    (
        &[
            "docs",
            "documentation",
            "api",
            "sdk",
            "github",
            "official",
            "文档",
            "官方",
            "源码",
        ],
        &[
            WebSearchSourceRole::OfficialDocs,
            WebSearchSourceRole::TechnicalResearch,
            WebSearchSourceRole::SemanticResearch,
        ],
    ),
    (
        &[
            "developer",
            "programming",
            "software",
            "source code",
            "bug",
            "error",
            "技术",
            "开发",
            "编程",
            "代码",
            "源码",
            "报错",
        ],
        &[
            WebSearchSourceRole::TechnicalResearch,
            WebSearchSourceRole::SemanticResearch,
        ],
    ),
    (
        &[
            "paper",
            "academic",
            "journal",
            "doi",
            "arxiv",
            "literature",
            "论文",
            "学术",
            "期刊",
            "文献",
        ],
        &[
            WebSearchSourceRole::AcademicResearch,
            WebSearchSourceRole::SemanticResearch,
        ],
    ),
    (
        &[
            "market",
            "industry",
            "business",
            "company",
            "financial",
            "revenue",
            "market share",
            "市场",
            "行业",
            "商业",
            "公司",
            "财报",
            "营收",
            "份额",
        ],
        &[WebSearchSourceRole::MarketResearch],
    ),
    (
        &[
            "compare",
            "comparison",
            "review",
            "recommend",
            "versus",
            " vs ",
            "price",
            "buy",
            "对比",
            "比较",
            "评测",
            "推荐",
            "价格",
            "购买",
            "哪个好",
        ],
        &[WebSearchSourceRole::ProductResearch],
    ),
    (
        &[
            "multilingual",
            "cross-language",
            "non-english",
            "多语言",
            "跨语言",
            "外文",
            "日文",
            "韩文",
            "德文",
            "法文",
            "西班牙文",
        ],
        &[WebSearchSourceRole::MultilingualSearch],
    ),
    (
        &[
            "related",
            "similar",
            "concept",
            "background",
            "discover",
            "相关",
            "相似",
            "概念",
            "背景",
            "探索",
            "综述",
        ],
        &[WebSearchSourceRole::SemanticResearch],
    ),
];

fn infer_source_roles(args: &WebSearchArgs) -> HashSet<WebSearchSourceRole> {
    let query = std::iter::once(args.query.as_str())
        .chain(args.additional_queries.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut roles = HashSet::new();
    for (markers, implied_roles) in ROLE_MARKERS {
        if markers.iter().any(|marker| query.contains(marker)) {
            roles.extend(implied_roles.iter().cloned());
        }
    }
    if args.recency_days.is_some() {
        roles.insert(WebSearchSourceRole::CurrentNews);
    }
    if roles.is_empty() {
        roles.insert(WebSearchSourceRole::GeneralSearch);
    }
    roles
}

struct SourceExecution {
    source: WebSearchSourceConfig,
    query: String,
    output: Option<ProviderSearchOutput>,
    summary: SourceRunSummary,
    status_updates: Vec<WebSearchKeyStatusUpdate>,
}

async fn execute_source_query(
    source: WebSearchSourceConfig,
    request: ProviderSearchRequest,
    selected_key: Option<(usize, String)>,
    config: &WebSearchToolConfig,
) -> SourceExecution {
    let started = Instant::now();
    let provider = source.provider.as_str().to_string();
    let query = request.query.clone();
    let (key_index, api_key) = match selected_key {
        Some((key_index, api_key)) => (Some(key_index), api_key),
        None if source.provider.supports_anonymous_search() => (None, String::new()),
        None => {
            return source_failure(
                source,
                query,
                started,
                "unavailable",
                "source has no usable credential".to_string(),
                Vec::new(),
            );
        }
    };
    let timeout = Duration::from_millis(config.timeout_ms());
    let control_proxy = source
        .proxy_url
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let control_client = match client_cache::control_client(timeout, control_proxy) {
        Ok(client) => client,
        Err(error) => {
            return source_failure(source, query, started, "failed", error, Vec::new());
        }
    };
    let base_url = source.provider.default_base_url();
    let result = dispatch_search(
        source.provider.clone(),
        &control_client,
        base_url,
        &api_key,
        &request,
    )
    .await;

    match result {
        Ok(output) => {
            let result_count = output.results.len();
            let status = if result_count == 0 {
                "soft_miss"
            } else {
                "success"
            };
            let status_updates = key_index
                .map(|key_index| {
                    vec![key_status_update(
                        &source.id,
                        key_index,
                        &api_key,
                        "normal".to_string(),
                        "request succeeded".to_string(),
                    )]
                })
                .unwrap_or_default();
            SourceExecution {
                summary: SourceRunSummary {
                    source_id: source.id.clone(),
                    provider: provider.clone(),
                    query: query.clone(),
                    status: status.to_string(),
                    result_count,
                    duration_ms: started.elapsed().as_millis() as u64,
                    detail: None,
                },
                source,
                query,
                output: Some(output),
                status_updates,
            }
        }
        Err(error) => {
            let message = sanitize_error_message(error.message(), &api_key);
            let status_updates = key_index
                .map(|key_index| {
                    let status = classify_key_status(source.provider.clone(), &message);
                    vec![key_status_update(
                        &source.id,
                        key_index,
                        &api_key,
                        status,
                        message.clone(),
                    )]
                })
                .unwrap_or_default();
            source_failure(source, query, started, "failed", message, status_updates)
        }
    }
}

fn key_health_rank(
    source: &WebSearchSourceConfig,
    original_key_index: usize,
    operation: WebSearchKeyOperation,
) -> u8 {
    let statuses = match operation {
        WebSearchKeyOperation::Search => &source.key_statuses,
        WebSearchKeyOperation::Extract => &source.extract_key_statuses,
    };
    match statuses
        .get(original_key_index)
        .and_then(Option::as_ref)
        .map(|record| record.status.as_str())
    {
        Some("normal") => 0,
        None => 1,
        Some("rate_limited" | "network_error" | "service_error") => 2,
        Some("quota_limited") => 3,
        Some(
            "quota_exhausted"
            | "permission_denied"
            | "configuration_error"
            | "invalid_key"
            | "unauthorized"
            | "officially_blocked",
        ) => 4,
        Some(_) => 3,
    }
}

fn source_failure(
    source: WebSearchSourceConfig,
    query: String,
    started: Instant,
    status: &str,
    detail: String,
    status_updates: Vec<WebSearchKeyStatusUpdate>,
) -> SourceExecution {
    SourceExecution {
        summary: SourceRunSummary {
            source_id: source.id.clone(),
            provider: source.provider.as_str().to_string(),
            query: query.clone(),
            status: status.to_string(),
            result_count: 0,
            duration_ms: started.elapsed().as_millis() as u64,
            detail: Some(detail),
        },
        source,
        query,
        output: None,
        status_updates,
    }
}

#[derive(Debug)]
struct AggregateEntry {
    score: f64,
    title: String,
    url: String,
    snippet: String,
    content_excerpt: Option<String>,
    kind: SearchHitKind,
    published_at: Option<String>,
    source_ids: HashSet<String>,
    provider_kinds: HashSet<String>,
    independence_groups: HashSet<String>,
    matched_queries: HashSet<String>,
}

fn aggregate_results(
    outputs: &[(WebSearchSourceConfig, String, ProviderSearchOutput)],
    allowed_domains: &[String],
    blocked_domains: &[String],
) -> Vec<AggregatedSearchResult> {
    let mut entries: HashMap<String, AggregateEntry> = HashMap::new();
    for (source, query, output) in outputs {
        for (rank, hit) in output.results.iter().enumerate() {
            let Some(url) = canonicalize_url(&hit.url) else {
                continue;
            };
            if !url_allowed(&url, allowed_domains, blocked_domains) {
                continue;
            }
            let entry = entries
                .entry(url.clone())
                .or_insert_with(|| AggregateEntry {
                    score: 0.0,
                    title: hit.title.clone(),
                    url: url.clone(),
                    snippet: hit.snippet.clone(),
                    content_excerpt: hit.content_excerpt.clone(),
                    kind: hit.kind.clone(),
                    published_at: hit.published_at.clone(),
                    source_ids: HashSet::new(),
                    provider_kinds: HashSet::new(),
                    independence_groups: HashSet::new(),
                    matched_queries: HashSet::new(),
                });
            entry.score += 1.0 / (60.0 + rank as f64 + 1.0);
            if hit.snippet.chars().count() > entry.snippet.chars().count() {
                entry.snippet = hit.snippet.clone();
            }
            if hit.content_excerpt.as_ref().is_some_and(|content| {
                content.chars().count()
                    > entry
                        .content_excerpt
                        .as_ref()
                        .map(|value| value.chars().count())
                        .unwrap_or_default()
            }) {
                entry.content_excerpt = hit.content_excerpt.clone();
            }
            if evidence_priority(&hit.kind) > evidence_priority(&entry.kind) {
                entry.kind = hit.kind.clone();
            }
            entry.source_ids.insert(source.id.clone());
            entry
                .provider_kinds
                .insert(source.provider.provider_kind().to_string());
            entry
                .independence_groups
                .insert(source.independence_group().to_string());
            entry.matched_queries.insert(query.clone());
        }
    }
    let mut ranked = entries.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .independence_groups
            .len()
            .cmp(&left.independence_groups.len())
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.url.cmp(&right.url))
    });
    ranked.truncate(MAX_WEB_SEARCH_RESULTS as usize);
    ranked
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut source_ids = entry.source_ids.into_iter().collect::<Vec<_>>();
            let mut provider_kinds = entry.provider_kinds.into_iter().collect::<Vec<_>>();
            let mut independence_groups = entry.independence_groups.into_iter().collect::<Vec<_>>();
            let mut matched_queries = entry.matched_queries.into_iter().collect::<Vec<_>>();
            source_ids.sort();
            provider_kinds.sort();
            independence_groups.sort();
            matched_queries.sort();
            AggregatedSearchResult {
                rank: index + 1,
                title: entry.title,
                url: entry.url,
                snippet: truncate(&entry.snippet, 500),
                content_excerpt: entry.content_excerpt.map(|value| truncate(&value, 4_000)),
                evidence_kind: entry.kind,
                source_ids,
                provider_kinds,
                corroboration_count: independence_groups.len(),
                independence_groups,
                matched_queries,
                published_at: entry.published_at,
            }
        })
        .collect()
}

fn evidence_priority(kind: &SearchHitKind) -> u8 {
    match kind {
        SearchHitKind::WebPage => 3,
    }
}

pub(crate) fn canonicalize_url(raw: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);
    let kept = url
        .query_pairs()
        .filter(|(key, _)| {
            let key = key.to_ascii_lowercase();
            !key.starts_with("utm_")
                && !matches!(
                    key.as_ref(),
                    "fbclid" | "gclid" | "dclid" | "mc_cid" | "mc_eid" | "ref"
                )
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    Some(url.to_string())
}

fn normalize_domains(values: &[String]) -> Vec<String> {
    let mut domains = values
        .iter()
        .filter_map(|value| {
            let value = value.trim().trim_start_matches("*.").trim_end_matches('.');
            if value.is_empty() {
                None
            } else {
                Some(value.to_ascii_lowercase())
            }
        })
        .collect::<Vec<_>>();
    domains.sort();
    domains.dedup();
    domains
}

fn url_allowed(url: &str, allowed: &[String], blocked: &[String]) -> bool {
    let Some(host) = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    if blocked.iter().any(|domain| domain_matches(&host, domain)) {
        return false;
    }
    allowed.is_empty() || allowed.iter().any(|domain| domain_matches(&host, domain))
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn key_status_update(
    source_id: &str,
    key_index: usize,
    api_key: &str,
    status: String,
    detail: String,
) -> WebSearchKeyStatusUpdate {
    WebSearchKeyStatusUpdate {
        operation: WebSearchKeyOperation::Search,
        source_id: source_id.to_string(),
        key_index,
        api_key: api_key.to_string(),
        record: WebSearchKeyStatusRecord {
            status,
            detail,
            checked_at: chrono::Utc::now().to_rfc3339(),
        },
    }
}

pub(crate) fn classify_key_status(provider: WebSearchProviderKind, message: &str) -> String {
    providers::classify_key_status(provider, message).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(query: &str) -> WebSearchArgs {
        WebSearchArgs {
            query: query.to_string(),
            additional_queries: Vec::new(),
            mode: SearchMode::Auto,
            source_roles: Vec::new(),
            source_ids: Vec::new(),
            max_results_per_source: None,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            recency_days: None,
        }
    }

    #[test]
    fn empty_query_is_rejected() {
        let error = normalize_queries(&args("  ")).unwrap_err();
        assert!(error.contains("must not be empty"));
    }

    #[test]
    fn auto_detects_deep_research() {
        assert_eq!(
            resolve_mode(&SearchMode::Auto, &["请做深度调研".to_string()]),
            SearchMode::Deep
        );
    }

    #[test]
    fn source_role_inference_covers_common_search_intents() {
        let roles = infer_source_roles(&args("核验最新 API 官方文档，并做行业市场和产品价格对比"));
        assert!(roles.contains(&WebSearchSourceRole::CurrentNews));
        assert!(roles.contains(&WebSearchSourceRole::FactChecking));
        assert!(roles.contains(&WebSearchSourceRole::OfficialDocs));
        assert!(roles.contains(&WebSearchSourceRole::TechnicalResearch));
        assert!(roles.contains(&WebSearchSourceRole::MarketResearch));
        assert!(roles.contains(&WebSearchSourceRole::ProductResearch));
    }

    #[test]
    fn generic_queries_fall_back_to_general_search() {
        assert_eq!(
            infer_source_roles(&args("北京天气")),
            HashSet::from([WebSearchSourceRole::GeneralSearch])
        );
    }

    #[test]
    fn canonical_url_removes_tracking_and_fragments() {
        assert_eq!(
            canonicalize_url("https://Example.com/a?utm_source=x&id=2#part").as_deref(),
            Some("https://example.com/a?id=2")
        );
    }

    #[test]
    fn fresh_defaults_route_search_to_anonymous_sources() {
        let config = WebSearchToolConfig::default();
        let selected = select_sources(&config, &args("rust"), &SearchMode::Fast).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>(),
            vec!["keenable", "jina", "firecrawl"]
        );
        assert!(selected[0].api_keys.is_empty());
        assert!(selected[1].api_keys.is_empty());
    }

    #[test]
    fn balanced_selection_uses_independent_groups() {
        let mut config = WebSearchToolConfig {
            enabled: true,
            ..WebSearchToolConfig::default()
        };
        for source in &mut config.sources {
            source.api_keys = vec!["key".to_string()];
            source.enabled = true;
        }
        config.sources[1].independence_group = config.sources[0].independence_group.clone();
        let selected = select_sources(&config, &args("rust"), &SearchMode::Balanced).unwrap();
        let groups = selected
            .iter()
            .map(|source| source.independence_group().to_string())
            .collect::<HashSet<_>>();
        assert_eq!(groups.len(), selected.len());
        assert_eq!(selected.len(), 3);
    }

    #[test]
    fn current_queries_prioritize_complementary_current_sources() {
        let mut config = WebSearchToolConfig {
            enabled: true,
            ..WebSearchToolConfig::default()
        };
        for source in &mut config.sources {
            source.api_keys = vec!["key".to_string()];
            source.enabled = true;
        }
        let selected = select_sources(
            &config,
            &args("latest Rust release news"),
            &SearchMode::Balanced,
        )
        .unwrap();
        assert_eq!(selected[0].id, "keenable");
        assert_eq!(selected[1].id, "tavily");
    }

    #[test]
    fn key_status_merge_keeps_the_worst_parallel_result() {
        let mut updates = BTreeMap::new();
        merge_status_update(
            &mut updates,
            key_status_update("exa", 0, "secret", "normal".to_string(), "ok".to_string()),
        );
        merge_status_update(
            &mut updates,
            key_status_update(
                "exa",
                0,
                "secret",
                "rate_limited".to_string(),
                "limited".to_string(),
            ),
        );
        assert_eq!(
            updates.into_values().next().unwrap().record.status,
            "rate_limited"
        );
    }
}
