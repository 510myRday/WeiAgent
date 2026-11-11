use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use dashmap::DashMap;
use dom_smoothie::{CandidateSelectMode, Config, Readability, TextMode};
use futures_util::{StreamExt, future::join_all};
use reqwest::StatusCode;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::domain::tools::{
    EvidenceStorageConfig, MAX_WEB_EXTRACT_URLS, WebSearchKeyOperation, WebSearchKeyStatusUpdate,
};
use crate::domain::{
    ToolResult, WebSearchProviderKind, WebSearchSourceConfig, WebSearchToolConfig,
};

use super::providers::dispatch_extract;
use super::types::{
    ProviderError, ProviderExtractDocument, ProviderExtractOutput, ProviderExtractRequest,
    merge_status_update,
};
use super::{canonicalize_url, classify_key_status, select_source_key};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_CHARS_PER_URL: usize = 8_000;
const MAX_CHARS_PER_URL: usize = 20_000;
const HIGH_QUALITY_THRESHOLD: f32 = 0.68;
const MAX_REDIRECTS: usize = 8;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebExtractArgs {
    pub urls: Vec<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub max_chars_per_url: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebExtractEvidenceClass {
    LocalDirect,
    ProviderExtracted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebExtractAttempt {
    pub extractor: String,
    pub status: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebExtractDocument {
    pub document_id: String,
    pub requested_url: String,
    pub final_url: String,
    pub title: String,
    pub content_type: String,
    pub content_format: String,
    pub extractor: String,
    pub evidence_class: WebExtractEvidenceClass,
    pub citation_eligible: bool,
    pub quality_score: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quality_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub relevant_excerpts: Vec<String>,
    pub extracted_chars: usize,
    pub stored: bool,
    pub attempts: Vec<WebExtractAttempt>,
}

#[derive(Debug, Clone, Serialize)]
struct WebExtractFailure {
    requested_url: String,
    error: String,
    attempts: Vec<WebExtractAttempt>,
}

#[derive(Debug, Clone, Serialize)]
struct WebExtractOutput {
    documents: Vec<WebExtractDocument>,
    failures: Vec<WebExtractFailure>,
    citation_instruction: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct StoredEvidence<'a> {
    document_id: &'a str,
    requested_url: &'a str,
    final_url: &'a str,
    title: &'a str,
    content_type: &'a str,
    content_format: &'a str,
    extractor: &'a str,
    evidence_class: &'a WebExtractEvidenceClass,
    quality_score: f32,
    quality_reasons: &'a [String],
    published_at: &'a Option<String>,
    extracted_at: String,
    content: &'a str,
    attempts: &'a [WebExtractAttempt],
}

#[derive(Debug, Clone, Default)]
pub struct WebSearchProvenance {
    pub source_ids: Vec<String>,
    cached_content: Option<CachedSearchContent>,
}

#[derive(Debug, Clone)]
struct CachedSearchContent {
    title: String,
    url: String,
    content: String,
    published_at: Option<String>,
}

pub type WebSearchProvenanceCache = Arc<DashMap<String, WebSearchProvenance>>;

pub(crate) fn record_web_search_provenance(cache: &WebSearchProvenanceCache, result: &ToolResult) {
    let ToolResult::Completed { stdout, .. } = result else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return;
    };
    let Some(results) = value.get("results").and_then(serde_json::Value::as_array) else {
        return;
    };
    for result in results {
        let Some(url) = result.get("url").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(url) = canonicalize_url(url) else {
            continue;
        };
        let source_ids = result
            .get("source_ids")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let cached_content = result
            .get("content_excerpt")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|content| !content.is_empty())
            .map(|content| CachedSearchContent {
                title: result
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&url)
                    .to_string(),
                url: url.clone(),
                content: content.to_string(),
                published_at: result
                    .get("published_at")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            });
        if !source_ids.is_empty() || cached_content.is_some() {
            let mut entry = cache.entry(url).or_default();
            for source_id in source_ids {
                if !entry.source_ids.contains(&source_id) {
                    entry.source_ids.push(source_id);
                }
            }
            entry.source_ids.sort();
            if cached_content.as_ref().is_some_and(|candidate| {
                entry
                    .cached_content
                    .as_ref()
                    .is_none_or(|current| candidate.content.len() > current.content.len())
            }) {
                entry.cached_content = cached_content;
            }
        }
    }
}

pub(crate) struct WebExtractExecution {
    pub result: ToolResult,
    pub status_updates: Vec<WebSearchKeyStatusUpdate>,
}

pub async fn execute_web_extract_with_status_updates(
    args: WebExtractArgs,
    data_dir: &Path,
    evidence_storage: &EvidenceStorageConfig,
    web_search: &WebSearchToolConfig,
    provenance: &WebSearchProvenanceCache,
) -> WebExtractExecution {
    let started = Instant::now();
    let urls = normalize_urls(args.urls);
    if urls.is_empty() {
        return extraction_error("urls must contain at least one http or https URL".to_string());
    }
    if urls.len() > MAX_WEB_EXTRACT_URLS {
        return extraction_error(format!(
            "web_extract accepts at most {MAX_WEB_EXTRACT_URLS} URLs"
        ));
    }
    let max_chars = args
        .max_chars_per_url
        .unwrap_or(DEFAULT_MAX_CHARS_PER_URL)
        .clamp(1_000, MAX_CHARS_PER_URL);
    let local_client = match crate::tools::web_search::client_cache::local_extract_client() {
        Ok(client) => client,
        Err(error) => return extraction_error(error),
    };
    let remote_client = match crate::tools::web_search::client_cache::remote_extract_client(
        Duration::from_millis(web_search.timeout_ms()).max(Duration::from_secs(45)),
    ) {
        Ok(client) => client,
        Err(error) => return extraction_error(error),
    };
    let query = args.query.unwrap_or_default();
    let semaphore = Arc::new(Semaphore::new(3));
    let tasks = urls.into_iter().map(|url| {
        let provenance_entry = canonicalize_url(&url)
            .and_then(|key| provenance.get(&key).map(|entry| entry.value().clone()))
            .unwrap_or_default();
        extract_one_routed(
            local_client.clone(),
            remote_client.clone(),
            url,
            query.clone(),
            max_chars,
            data_dir.to_path_buf(),
            evidence_storage.enabled,
            web_search.clone(),
            provenance_entry,
            semaphore.clone(),
        )
    });

    let mut documents = Vec::new();
    let mut failures = Vec::new();
    let mut status_updates = BTreeMap::new();
    for outcome in join_all(tasks).await {
        if let Some(document) = outcome.document {
            documents.push(document);
        }
        if let Some(failure) = outcome.failure {
            failures.push(failure);
        }
        for update in outcome.status_updates {
            merge_status_update(&mut status_updates, update);
        }
    }
    if evidence_storage.enabled
        && let Err(error) = cleanup_evidence_store(data_dir, evidence_storage).await
    {
        tracing::warn!(error = %error, "failed to clean evidence store");
    }
    let output = WebExtractOutput {
        documents,
        failures,
        citation_instruction: "Cite final_url only for documents where citation_eligible is true.",
    };
    match serde_json::to_string_pretty(&output) {
        Ok(stdout) => WebExtractExecution {
            result: ToolResult::Completed {
                stdout,
                stderr: String::new(),
                exit_code: 0,
                duration_ms: started.elapsed().as_millis() as u64,
                runtime: Some("builtin_web_extract".to_string()),
                cwd: None,
                output_cursor: 0,
                structured_content: None,
                is_error: false,
            },
            status_updates: status_updates.into_values().collect(),
        },
        Err(error) => extraction_error(format!("failed to serialize extracted evidence: {error}")),
    }
}

fn extraction_error(message: String) -> WebExtractExecution {
    WebExtractExecution {
        result: ToolResult::Error { message },
        status_updates: Vec::new(),
    }
}

struct UrlExtractionOutcome {
    document: Option<WebExtractDocument>,
    failure: Option<WebExtractFailure>,
    status_updates: Vec<WebSearchKeyStatusUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteHint {
    General,
    DynamicOrBlocked,
    Pdf,
}

#[derive(Debug, Clone)]
struct ExtractCandidate {
    requested_url: String,
    final_url: String,
    title: String,
    content_type: String,
    content_format: String,
    content: String,
    published_at: Option<String>,
    extractor: String,
    evidence_class: WebExtractEvidenceClass,
    quality: QualityAssessment,
}

#[derive(Debug, Clone)]
struct QualityAssessment {
    score: f32,
    reasons: Vec<String>,
    hard_page: bool,
}

#[derive(Debug)]
struct ExtractAttemptError {
    message: String,
    route_hint: RouteHint,
}

#[allow(clippy::too_many_arguments)]
async fn extract_one_routed(
    local_client: reqwest::Client,
    remote_client: reqwest::Client,
    requested_url: String,
    query: String,
    max_chars: usize,
    data_dir: PathBuf,
    store_enabled: bool,
    web_search: WebSearchToolConfig,
    provenance: WebSearchProvenance,
    semaphore: Arc<Semaphore>,
) -> UrlExtractionOutcome {
    let mut attempts = Vec::new();
    let mut status_updates = Vec::new();
    let validation_started = Instant::now();
    let parsed_target = match reqwest::Url::parse(&requested_url) {
        Ok(url) => url,
        Err(error) => {
            attempts.push(WebExtractAttempt {
                extractor: "url_validation".to_string(),
                status: "failed".to_string(),
                duration_ms: validation_started.elapsed().as_millis() as u64,
                quality_score: None,
                detail: Some(format!("invalid URL: {error}")),
            });
            return UrlExtractionOutcome {
                document: None,
                failure: Some(WebExtractFailure {
                    requested_url,
                    error: "URL must use HTTP(S) and target a public host".to_string(),
                    attempts,
                }),
                status_updates,
            };
        }
    };
    if let Err(error) = validate_public_target(&parsed_target).await {
        attempts.push(WebExtractAttempt {
            extractor: "url_validation".to_string(),
            status: "failed".to_string(),
            duration_ms: validation_started.elapsed().as_millis() as u64,
            quality_score: None,
            detail: Some(error.message),
        });
        return UrlExtractionOutcome {
            document: None,
            failure: Some(WebExtractFailure {
                requested_url,
                error: "URL must use HTTP(S) and target a public host".to_string(),
                attempts,
            }),
            status_updates,
        };
    }
    let mut best_direct = None;
    if let Some(cached) = provenance.cached_content {
        let cache_started = Instant::now();
        let content = normalize_content(&cached.content);
        let quality = assess_quality(&cached.title, &content, &query);
        let candidate = ExtractCandidate {
            requested_url: requested_url.clone(),
            final_url: cached.url,
            title: meaningful_title(&cached.title, &content),
            content_type: "text/plain".to_string(),
            content_format: "text".to_string(),
            content,
            published_at: cached.published_at,
            extractor: "web_search_cache".to_string(),
            evidence_class: WebExtractEvidenceClass::ProviderExtracted,
            quality,
        };
        let high_quality = candidate.quality.score >= HIGH_QUALITY_THRESHOLD;
        attempts.push(WebExtractAttempt {
            extractor: "web_search_cache".to_string(),
            status: if high_quality {
                "success"
            } else {
                "low_quality"
            }
            .to_string(),
            duration_ms: cache_started.elapsed().as_millis() as u64,
            quality_score: Some(candidate.quality.score),
            detail: (!high_quality).then(|| candidate.quality.reasons.join(", ")),
        });
        if high_quality {
            let document = finalize_candidate(
                candidate,
                attempts,
                &query,
                max_chars,
                &data_dir,
                store_enabled,
            )
            .await;
            return UrlExtractionOutcome {
                document: Some(document),
                failure: None,
                status_updates,
            };
        }
        best_direct = Some(candidate);
    }
    let local_started = Instant::now();
    let route_hint;

    match extract_local(&local_client, &requested_url, &query).await {
        Ok((candidate, local_hint)) => {
            route_hint = local_hint;
            let high_quality = candidate.quality.score >= HIGH_QUALITY_THRESHOLD;
            attempts.push(WebExtractAttempt {
                extractor: "local".to_string(),
                status: if high_quality {
                    "success"
                } else {
                    "low_quality"
                }
                .to_string(),
                duration_ms: local_started.elapsed().as_millis() as u64,
                quality_score: Some(candidate.quality.score),
                detail: (!high_quality).then(|| candidate.quality.reasons.join(", ")),
            });
            if high_quality {
                let document = finalize_candidate(
                    candidate,
                    attempts,
                    &query,
                    max_chars,
                    &data_dir,
                    store_enabled,
                )
                .await;
                return UrlExtractionOutcome {
                    document: Some(document),
                    failure: None,
                    status_updates,
                };
            }
            if best_direct
                .as_ref()
                .is_none_or(|best| candidate.quality.score > best.quality.score)
            {
                best_direct = Some(candidate);
            }
        }
        Err(error) => {
            route_hint = error.route_hint;
            attempts.push(WebExtractAttempt {
                extractor: "local".to_string(),
                status: "failed".to_string(),
                duration_ms: local_started.elapsed().as_millis() as u64,
                quality_score: None,
                detail: Some(error.message),
            });
        }
    }

    let remote_allowed =
        web_search.extraction.remote_enabled && remote_url_is_shareable(&requested_url);
    if remote_allowed {
        let ordered = ordered_extract_sources(&web_search, &provenance.source_ids, route_hint);
        let mut remote_attempts = 0usize;
        for source in &ordered {
            if remote_attempts >= web_search.max_remote_extract_attempts() {
                break;
            }
            let selected_key =
                select_source_key(source, &web_search, WebSearchKeyOperation::Extract);
            let (key_index, api_key) = match selected_key {
                Some((index, key)) => (Some(index), key),
                None if source.provider.supports_anonymous_extract() => (None, String::new()),
                None => continue,
            };
            remote_attempts += 1;
            let _permit = semaphore.acquire().await.expect("semaphore remains open");
            let attempt_started = Instant::now();
            let request = ProviderExtractRequest {
                url: requested_url.clone(),
                query: query.clone(),
                hard_page: route_hint == RouteHint::DynamicOrBlocked,
                timeout_ms: web_search.timeout_ms().max(45_000),
            };
            let result = execute_provider_extract(source, &remote_client, &api_key, &request).await;
            match result {
                Ok(output) => {
                    if let Some(index) = key_index {
                        status_updates.push(extract_key_status_update(
                            source,
                            index,
                            &api_key,
                            "normal",
                            "extraction request succeeded",
                        ));
                    }
                    if let Some(document) = output.documents.into_iter().next() {
                        let candidate = provider_candidate(document, source, &query);
                        let high_quality = candidate.quality.score >= HIGH_QUALITY_THRESHOLD;
                        attempts.push(WebExtractAttempt {
                            extractor: source.id.clone(),
                            status: if high_quality {
                                "success"
                            } else {
                                "low_quality"
                            }
                            .to_string(),
                            duration_ms: attempt_started.elapsed().as_millis() as u64,
                            quality_score: Some(candidate.quality.score),
                            detail: (!high_quality).then(|| candidate.quality.reasons.join(", ")),
                        });
                        if high_quality {
                            let document = finalize_candidate(
                                candidate,
                                attempts,
                                &query,
                                max_chars,
                                &data_dir,
                                store_enabled,
                            )
                            .await;
                            return UrlExtractionOutcome {
                                document: Some(document),
                                failure: None,
                                status_updates,
                            };
                        }
                        if best_direct
                            .as_ref()
                            .is_none_or(|best| candidate.quality.score > best.quality.score)
                        {
                            best_direct = Some(candidate);
                        }
                    } else {
                        attempts.push(WebExtractAttempt {
                            extractor: source.id.clone(),
                            status: "soft_miss".to_string(),
                            duration_ms: attempt_started.elapsed().as_millis() as u64,
                            quality_score: None,
                            detail: output
                                .failures
                                .first()
                                .map(|failure| failure.message.clone()),
                        });
                    }
                }
                Err(error) => {
                    let message = error.message().to_string();
                    if let Some(index) = key_index {
                        let status = classify_key_status(source.provider.clone(), &message);
                        status_updates.push(extract_key_status_update(
                            source, index, &api_key, &status, &message,
                        ));
                    }
                    attempts.push(WebExtractAttempt {
                        extractor: source.id.clone(),
                        status: "failed".to_string(),
                        duration_ms: attempt_started.elapsed().as_millis() as u64,
                        quality_score: None,
                        detail: Some(message),
                    });
                }
            }
        }
    } else if web_search.extraction.remote_enabled {
        attempts.push(WebExtractAttempt {
            extractor: "remote_router".to_string(),
            status: "skipped".to_string(),
            duration_ms: 0,
            quality_score: None,
            detail: Some("URL contains credentials or sensitive query parameters".to_string()),
        });
    }

    let document = match best_direct {
        Some(candidate) => Some(
            finalize_candidate(
                candidate,
                attempts.clone(),
                &query,
                max_chars,
                &data_dir,
                store_enabled,
            )
            .await,
        ),
        None => None,
    };
    let failure = if document.is_none() {
        Some(WebExtractFailure {
            requested_url,
            error: "all enabled extraction methods failed".to_string(),
            attempts,
        })
    } else {
        None
    };
    UrlExtractionOutcome {
        document,
        failure,
        status_updates,
    }
}

async fn execute_provider_extract(
    source: &WebSearchSourceConfig,
    client: &reqwest::Client,
    api_key: &str,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    let base_url = source.provider.default_extract_base_url();
    let proxied_client = source
        .proxy_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|proxy_url| {
            crate::tools::web_search::client_cache::control_client(
                Duration::from_millis(request.timeout_ms),
                Some(proxy_url),
            )
            .map_err(ProviderError::Retryable)
        })
        .transpose()?;
    let provider_client = proxied_client.as_ref().unwrap_or(client);
    dispatch_extract(
        source.provider.clone(),
        provider_client,
        base_url,
        api_key,
        request,
    )
    .await
}

fn ordered_extract_sources(
    config: &WebSearchToolConfig,
    provenance_ids: &[String],
    route_hint: RouteHint,
) -> Vec<WebSearchSourceConfig> {
    let provenance = provenance_ids.iter().collect::<HashSet<_>>();
    let mut sources = config
        .sources
        .iter()
        .filter(|source| source.extract_enabled && source.can_extract())
        .cloned()
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        let left_origin = provenance.contains(&left.id);
        let right_origin = provenance.contains(&right.id);
        right_origin
            .cmp(&left_origin)
            .then_with(|| {
                provider_route_rank(&left.provider, route_hint)
                    .cmp(&provider_route_rank(&right.provider, route_hint))
            })
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.id.cmp(&right.id))
    });
    sources
}

fn provider_route_rank(provider: &WebSearchProviderKind, hint: RouteHint) -> u8 {
    match hint {
        RouteHint::DynamicOrBlocked => match provider {
            WebSearchProviderKind::Keenable => 0,
            WebSearchProviderKind::Firecrawl => 1,
            WebSearchProviderKind::Jina => 2,
            WebSearchProviderKind::Exa => 3,
            WebSearchProviderKind::Tavily => 4,
        },
        RouteHint::Pdf => match provider {
            WebSearchProviderKind::Exa => 0,
            WebSearchProviderKind::Jina => 1,
            WebSearchProviderKind::Firecrawl => 2,
            WebSearchProviderKind::Tavily => 3,
            WebSearchProviderKind::Keenable => 4,
        },
        RouteHint::General => match provider {
            WebSearchProviderKind::Keenable => 0,
            WebSearchProviderKind::Jina => 1,
            WebSearchProviderKind::Firecrawl => 2,
            WebSearchProviderKind::Exa => 3,
            WebSearchProviderKind::Tavily => 4,
        },
    }
}

fn provider_candidate(
    document: ProviderExtractDocument,
    source: &WebSearchSourceConfig,
    query: &str,
) -> ExtractCandidate {
    let content = normalize_content(&document.content);
    let quality = assess_quality(&document.title, &content, query);
    ExtractCandidate {
        requested_url: document.requested_url,
        final_url: document.final_url,
        title: meaningful_title(&document.title, &content),
        content_type: if document.content_format == "markdown" {
            "text/markdown".to_string()
        } else {
            "text/plain".to_string()
        },
        content_format: document.content_format,
        content,
        published_at: document.published_at,
        extractor: source.id.clone(),
        evidence_class: WebExtractEvidenceClass::ProviderExtracted,
        quality,
    }
}

async fn finalize_candidate(
    candidate: ExtractCandidate,
    attempts: Vec<WebExtractAttempt>,
    query: &str,
    max_chars: usize,
    data_dir: &Path,
    store_enabled: bool,
) -> WebExtractDocument {
    let document_id = evidence_id(&candidate.final_url, candidate.content.as_bytes());
    let relevant_excerpts = select_excerpts(&candidate.content, query, max_chars);
    let citation_eligible = candidate.quality.score >= HIGH_QUALITY_THRESHOLD;
    let store_path = data_dir
        .join("evidence")
        .join(format!("{document_id}.json"));
    let stored = if store_enabled && citation_eligible {
        match serde_json::to_string_pretty(&StoredEvidence {
            document_id: &document_id,
            requested_url: &candidate.requested_url,
            final_url: &candidate.final_url,
            title: &candidate.title,
            content_type: &candidate.content_type,
            content_format: &candidate.content_format,
            extractor: &candidate.extractor,
            evidence_class: &candidate.evidence_class,
            quality_score: candidate.quality.score,
            quality_reasons: &candidate.quality.reasons,
            published_at: &candidate.published_at,
            extracted_at: chrono::Utc::now().to_rfc3339(),
            content: &candidate.content,
            attempts: &attempts,
        }) {
            Ok(json) => {
                if let Some(parent) = store_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                crate::fsutil::write_atomic(&store_path, json).await.is_ok()
            }
            Err(_) => false,
        }
    } else {
        false
    };
    WebExtractDocument {
        document_id,
        requested_url: candidate.requested_url,
        final_url: candidate.final_url,
        title: candidate.title,
        content_type: candidate.content_type,
        content_format: candidate.content_format,
        extractor: candidate.extractor,
        evidence_class: candidate.evidence_class,
        citation_eligible,
        quality_score: candidate.quality.score,
        quality_reasons: candidate.quality.reasons,
        published_at: candidate.published_at,
        relevant_excerpts,
        extracted_chars: candidate.content.chars().count(),
        stored,
        attempts,
    }
}

async fn extract_local(
    client: &reqwest::Client,
    requested_url: &str,
    query: &str,
) -> Result<(ExtractCandidate, RouteHint), ExtractAttemptError> {
    let mut current = reqwest::Url::parse(requested_url).map_err(|error| ExtractAttemptError {
        message: format!("invalid URL: {error}"),
        route_hint: RouteHint::General,
    })?;
    let mut response = None;
    for _ in 0..=MAX_REDIRECTS {
        validate_public_target(&current).await?;
        let next = client
            .get(current.clone())
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/pdf,text/plain,application/json",
            )
            .send()
            .await
            .map_err(|error| ExtractAttemptError {
                message: format!("request failed: {error}"),
                route_hint: RouteHint::General,
            })?;
        if next.status().is_redirection() {
            let Some(location) = next.headers().get(reqwest::header::LOCATION) else {
                return Err(ExtractAttemptError {
                    message: format!("HTTP {} redirect omitted Location", next.status()),
                    route_hint: RouteHint::General,
                });
            };
            let location = location.to_str().map_err(|_| ExtractAttemptError {
                message: "redirect Location is not valid text".to_string(),
                route_hint: RouteHint::General,
            })?;
            current = current
                .join(location)
                .map_err(|error| ExtractAttemptError {
                    message: format!("invalid redirect target: {error}"),
                    route_hint: RouteHint::General,
                })?;
            continue;
        }
        response = Some(next);
        break;
    }
    let response = response.ok_or_else(|| ExtractAttemptError {
        message: "too many redirects".to_string(),
        route_hint: RouteHint::General,
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ExtractAttemptError {
            message: format!("HTTP {status}"),
            route_hint: if matches!(
                status,
                StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS | StatusCode::UNAUTHORIZED
            ) {
                RouteHint::DynamicOrBlocked
            } else {
                RouteHint::General
            },
        });
    }
    let final_url = response.url().to_string();
    let content_type_header = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let content_type = content_type_header
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ExtractAttemptError {
            message: format!("response exceeds {MAX_RESPONSE_BYTES} bytes"),
            route_hint: RouteHint::General,
        });
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| ExtractAttemptError {
            message: format!("response body failed: {error}"),
            route_hint: RouteHint::General,
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(ExtractAttemptError {
                message: format!("response exceeds {MAX_RESPONSE_BYTES} bytes"),
                route_hint: RouteHint::General,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    let extracted = extract_content(&bytes, &content_type, &content_type_header, &final_url)
        .map_err(|message| ExtractAttemptError {
            route_hint: if content_type == "application/pdf" || bytes.starts_with(b"%PDF-") {
                RouteHint::Pdf
            } else {
                RouteHint::General
            },
            message,
        })?;
    let content = normalize_content(&extracted.content);
    if content.is_empty() {
        return Err(ExtractAttemptError {
            message: "page contained no extractable text".to_string(),
            route_hint: RouteHint::DynamicOrBlocked,
        });
    }
    let quality = assess_quality(&extracted.title, &content, query);
    let route_hint = if extracted.is_pdf {
        RouteHint::Pdf
    } else if quality.hard_page {
        RouteHint::DynamicOrBlocked
    } else {
        RouteHint::General
    };
    Ok((
        ExtractCandidate {
            requested_url: requested_url.to_string(),
            final_url,
            title: meaningful_title(&extracted.title, &content),
            content_type,
            content_format: extracted.content_format,
            content,
            published_at: extracted.published_at,
            extractor: "local".to_string(),
            evidence_class: WebExtractEvidenceClass::LocalDirect,
            quality,
        },
        route_hint,
    ))
}

async fn validate_public_target(url: &reqwest::Url) -> Result<(), ExtractAttemptError> {
    if !is_public_http_url(url) {
        return Err(ExtractAttemptError {
            message: "URL must use HTTP(S) and target a public host".to_string(),
            route_hint: RouteHint::General,
        });
    }
    let Some(host) = url.host_str() else {
        return Err(ExtractAttemptError {
            message: "URL has no host".to_string(),
            route_hint: RouteHint::General,
        });
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| ExtractAttemptError {
            message: format!("DNS lookup failed: {error}"),
            route_hint: RouteHint::General,
        })?
        .map(|address| address.ip())
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(*address)) {
        return Err(ExtractAttemptError {
            message: "URL resolved to a non-public address".to_string(),
            route_hint: RouteHint::General,
        });
    }
    Ok(())
}

struct ExtractedContent {
    title: String,
    published_at: Option<String>,
    content: String,
    content_format: String,
    is_pdf: bool,
}

fn extract_content(
    bytes: &[u8],
    content_type: &str,
    content_type_header: &str,
    final_url: &str,
) -> Result<ExtractedContent, String> {
    if content_type == "application/pdf" || bytes.starts_with(b"%PDF-") {
        let content = pdf_extract::extract_text_from_mem(bytes)
            .map_err(|error| format!("PDF extraction failed: {error}"))?;
        return Ok(ExtractedContent {
            title: "PDF document".to_string(),
            published_at: None,
            content,
            content_format: "text".to_string(),
            is_pdf: true,
        });
    }
    let text = decode_text(bytes, content_type_header);
    if content_type.contains("html") || text.trim_start().starts_with('<') {
        return Ok(extract_html(&text, final_url));
    }
    Ok(ExtractedContent {
        title: "Web document".to_string(),
        published_at: None,
        content: text,
        content_format: "text".to_string(),
        is_pdf: false,
    })
}

fn decode_text(bytes: &[u8], content_type_header: &str) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _, _) = encoding.decode(&bytes[bom_len..]);
        return text.into_owned();
    }
    let charset = content_type_header.split(';').find_map(|part| {
        part.trim()
            .strip_prefix("charset=")
            .or_else(|| part.trim().strip_prefix("Charset="))
            .map(str::trim)
            .map(|value| value.trim_matches(['\'', '"']))
    });
    let encoding = charset
        .and_then(|value| encoding_rs::Encoding::for_label(value.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

fn extract_html(html: &str, final_url: &str) -> ExtractedContent {
    let config = Config {
        max_elements_to_parse: 100_000,
        candidate_select_mode: CandidateSelectMode::DomSmoothie,
        text_mode: TextMode::Markdown,
        ..Config::default()
    };
    if let Ok(mut readability) = Readability::new(html, Some(final_url), Some(config))
        && let Ok(article) = readability.parse()
    {
        let content = article.text_content.to_string();
        if !content.trim().is_empty() {
            return ExtractedContent {
                title: article.title,
                published_at: article.published_time,
                content,
                content_format: "markdown".to_string(),
                is_pdf: false,
            };
        }
    }
    extract_html_fallback(html)
}

fn extract_html_fallback(html: &str) -> ExtractedContent {
    let document = Html::parse_document(html);
    let title = first_text(&document, "meta[property='og:title']", Some("content"))
        .or_else(|| first_text(&document, "title", None))
        .unwrap_or_else(|| "Web page".to_string());
    let published_at = first_text(
        &document,
        "meta[property='article:published_time']",
        Some("content"),
    )
    .or_else(|| first_text(&document, "meta[name='date']", Some("content")))
    .or_else(|| first_text(&document, "time[datetime]", Some("datetime")));
    let root_selector = Selector::parse("article, main, body").expect("static selector");
    let content = document
        .select(&root_selector)
        .next()
        .map(|element| {
            element
                .text()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default();
    ExtractedContent {
        title,
        published_at,
        content,
        content_format: "text".to_string(),
        is_pdf: false,
    }
}

fn first_text(document: &Html, selector: &str, attribute: Option<&str>) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    let element = document.select(&selector).next()?;
    let value = if let Some(attribute) = attribute {
        element.value().attr(attribute)?.to_string()
    } else {
        element.text().collect::<Vec<_>>().join(" ")
    };
    let value = normalize_inline_whitespace(&value);
    (!value.is_empty()).then_some(value)
}

fn assess_quality(title: &str, content: &str, query: &str) -> QualityAssessment {
    let lower = format!("{title}\n{content}").to_lowercase();
    let challenge_markers = [
        "just a moment",
        "checking your browser",
        "verify you are human",
        "attention required",
        "enable javascript",
        "javascript is required",
        "access denied",
        "cf-chl-",
        "captcha",
        "人机验证",
        "请启用javascript",
        "访问被拒绝",
    ];
    let access_wall_markers = [
        "sign in to continue",
        "log in to continue",
        "login to continue",
        "please sign in",
        "please log in",
        "authentication required",
        "subscription required",
        "subscribe to continue reading",
        "登录后继续",
        "请先登录",
        "登录以继续",
        "订阅后继续阅读",
    ];
    let challenge_page = challenge_markers
        .iter()
        .any(|marker| lower.contains(marker));
    let access_wall = access_wall_markers
        .iter()
        .any(|marker| lower.contains(marker));
    let hard_page = challenge_page || access_wall;
    let characters = content
        .chars()
        .filter(|value| !value.is_whitespace())
        .count();
    let mut score: f32 = match characters {
        0..=79 => 0.05,
        80..=199 => 0.2,
        200..=499 => 0.45,
        500..=999 => 0.62,
        1_000..=2_499 => 0.75,
        _ => 0.85,
    };
    let mut reasons = Vec::new();
    if characters < 500 {
        reasons.push("thin_content".to_string());
    }
    let paragraph_count = content
        .split('\n')
        .filter(|paragraph| paragraph.trim().chars().count() >= 40)
        .count();
    if paragraph_count >= 3 {
        score += 0.05;
    } else {
        reasons.push("few_paragraphs".to_string());
    }
    if !matches!(
        title.trim(),
        "" | "Web page" | "Web document" | "PDF document"
    ) {
        score += 0.04;
    }
    if lower.contains("<html") || lower.contains("<script") {
        score -= 0.25;
        reasons.push("raw_html_leak".to_string());
    }
    let lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() >= 12 {
        let short_lines = lines
            .iter()
            .filter(|line| line.chars().count() <= 64)
            .count();
        if short_lines * 10 >= lines.len() * 7 {
            score -= 0.3;
            reasons.push("navigation_or_menu_noise".to_string());
        }
    }
    if lines.len() >= 8 {
        let link_lines = lines
            .iter()
            .filter(|line| line.contains("](") || line.starts_with("http"))
            .count();
        if link_lines * 2 >= lines.len() {
            score -= 0.2;
            reasons.push("link_directory_noise".to_string());
        }
    }
    if hard_page {
        score = score.min(0.12);
        reasons.push(if access_wall {
            "login_or_subscription_wall".to_string()
        } else {
            "challenge_or_javascript_shell".to_string()
        });
    }
    let query_terms = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 2)
        .collect::<Vec<_>>();
    if !query_terms.is_empty() && query_terms.iter().any(|term| lower.contains(term)) {
        score += 0.04;
    }
    if score < HIGH_QUALITY_THRESHOLD && reasons.is_empty() {
        reasons.push("quality_below_threshold".to_string());
    }
    QualityAssessment {
        score: score.clamp(0.0, 1.0),
        reasons,
        hard_page,
    }
}

fn meaningful_title(title: &str, content: &str) -> String {
    let title = title.trim();
    if !title.is_empty() && !matches!(title, "Web page" | "Web document") {
        return title.to_string();
    }
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim())
        .filter(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
        .unwrap_or_else(|| "Web document".to_string())
}

fn normalize_content(value: &str) -> String {
    let mut output = Vec::new();
    let mut blank = false;
    for line in value.lines() {
        let line = normalize_inline_whitespace(line);
        if line.is_empty() {
            if !blank && !output.is_empty() {
                output.push(String::new());
            }
            blank = true;
        } else {
            output.push(line);
            blank = false;
        }
    }
    while output.last().is_some_and(String::is_empty) {
        output.pop();
    }
    if output.is_empty() {
        normalize_inline_whitespace(value)
    } else {
        output.join("\n")
    }
}

fn normalize_inline_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn select_excerpts(content: &str, query: &str, max_chars: usize) -> Vec<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 2)
        .collect::<Vec<_>>();
    let mut paragraphs = content
        .split(['\n', '。', '！', '？'])
        .map(str::trim)
        .filter(|paragraph| paragraph.chars().count() >= 40)
        .map(|paragraph| {
            let lower = paragraph.to_lowercase();
            let score = terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            (score, paragraph.to_string())
        })
        .collect::<Vec<_>>();
    paragraphs.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.chars().count().cmp(&left.1.chars().count()))
    });
    let mut remaining = max_chars;
    let mut excerpts = Vec::new();
    for (_, paragraph) in paragraphs {
        if remaining == 0 || excerpts.len() >= 8 {
            break;
        }
        let excerpt = paragraph
            .chars()
            .take(remaining.min(2_000))
            .collect::<String>();
        remaining = remaining.saturating_sub(excerpt.chars().count());
        excerpts.push(excerpt);
    }
    if excerpts.is_empty() {
        excerpts.push(content.chars().take(max_chars).collect());
    }
    excerpts
}

fn normalize_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    urls.into_iter()
        .filter_map(|url| {
            let mut parsed = reqwest::Url::parse(url.trim()).ok()?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return None;
            }
            parsed.set_fragment(None);
            let normalized = parsed.to_string();
            seen.insert(normalized.clone()).then_some(normalized)
        })
        .collect()
}

fn remote_url_is_shareable(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !is_public_http_url(&url) {
        return false;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let sensitive = [
        "access_token",
        "api_key",
        "apikey",
        "auth",
        "authorization",
        "key",
        "password",
        "session",
        "sig",
        "signature",
        "token",
    ];
    !url.query_pairs()
        .any(|(key, _)| sensitive.contains(&key.to_ascii_lowercase().as_str()))
}

fn evidence_id(final_url: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(final_url.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn is_public_http_url(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return false;
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map_or(true, is_public_ip)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99)
                || (a == 192 && b == 168)
                || (a == 198 && matches!(b, 18 | 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224
                || (a == 255 && b == 255 && c == 255 && d == 255))
        }
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(ipv4));
            }
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[0] == 0x2001 && segments[1] == 0x0002)
                || (segments[0] == 0x0100
                    && segments[1] == 0
                    && segments[2] == 0
                    && segments[3] == 0))
        }
    }
}

fn extract_key_status_update(
    source: &WebSearchSourceConfig,
    key_index: usize,
    api_key: &str,
    status: &str,
    detail: &str,
) -> WebSearchKeyStatusUpdate {
    WebSearchKeyStatusUpdate {
        operation: WebSearchKeyOperation::Extract,
        source_id: source.id.clone(),
        key_index,
        api_key: api_key.to_string(),
        record: crate::domain::WebSearchKeyStatusRecord {
            status: status.to_string(),
            detail: detail.to_string(),
            checked_at: chrono::Utc::now().to_rfc3339(),
        },
    }
}

#[derive(Debug)]
struct EvidenceFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EvidenceCleanupStats {
    pub removed_files: usize,
    pub removed_bytes: u64,
    pub remaining_bytes: u64,
}

pub async fn cleanup_evidence_store(
    data_dir: &Path,
    config: &EvidenceStorageConfig,
) -> std::io::Result<EvidenceCleanupStats> {
    if !config.enabled {
        return Ok(EvidenceCleanupStats::default());
    }

    let evidence_dir = data_dir.join("evidence");
    let mut directory = match tokio::fs::read_dir(&evidence_dir).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EvidenceCleanupStats::default());
        }
        Err(error) => return Err(error),
    };
    let now = SystemTime::now();
    let retention = Duration::from_secs(u64::from(config.retention_days).saturating_mul(86_400));
    let cutoff = now.checked_sub(retention).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut files = Vec::new();
    let mut stats = EvidenceCleanupStats::default();

    while let Some(entry) = directory.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let metadata = match entry.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = metadata.len();
        if modified < cutoff {
            if remove_evidence_file(&path).await? {
                stats.removed_files += 1;
                stats.removed_bytes = stats.removed_bytes.saturating_add(size);
            }
            continue;
        }
        files.push(EvidenceFile {
            path,
            size,
            modified,
        });
    }

    files.sort_by_key(|file| file.modified);
    let mut total_bytes = files
        .iter()
        .fold(0_u64, |total, file| total.saturating_add(file.size));
    let max_bytes = config.max_total_mb.saturating_mul(1024 * 1024);
    for file in files {
        if total_bytes <= max_bytes {
            break;
        }
        if remove_evidence_file(&file.path).await? {
            total_bytes = total_bytes.saturating_sub(file.size);
            stats.removed_files += 1;
            stats.removed_bytes = stats.removed_bytes.saturating_add(file.size);
        }
    }
    stats.remaining_bytes = total_bytes;
    Ok(stats)
}

async fn remove_evidence_file(path: &Path) -> std::io::Result<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_local_urls() {
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://127.0.0.1/x").unwrap()
        ));
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://localhost/x").unwrap()
        ));
        assert!(is_public_http_url(
            &reqwest::Url::parse("https://example.com/x").unwrap()
        ));
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://100.64.0.1/x").unwrap()
        ));
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://198.18.0.1/x").unwrap()
        ));
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://[::ffff:127.0.0.1]/x").unwrap()
        ));
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://[2001:db8::1]/x").unwrap()
        ));
    }

    #[test]
    fn html_extracts_title_date_and_body() {
        let extracted = extract_html(
            "<html><head><title>A</title><meta property='article:published_time' content='2026-01-01'></head><body><main><p>Hello world with enough article text to extract correctly.</p></main></body></html>",
            "https://example.com/article",
        );
        assert_eq!(extracted.title, "A");
        assert_eq!(extracted.published_at.as_deref(), Some("2026-01-01"));
        assert!(extracted.content.contains("Hello world"));
    }

    #[test]
    fn challenge_page_is_low_quality() {
        let quality = assess_quality(
            "Just a moment",
            "Checking your browser before accessing the site. Enable JavaScript.",
            "",
        );
        assert!(quality.hard_page);
        assert!(quality.score < HIGH_QUALITY_THRESHOLD);
    }

    #[test]
    fn login_wall_and_navigation_directory_are_low_quality() {
        let login = assess_quality(
            "Sign in",
            "Please sign in to continue reading this article.",
            "",
        );
        assert!(login.hard_page);
        assert!(
            login
                .reasons
                .iter()
                .any(|reason| reason == "login_or_subscription_wall")
        );

        let navigation = (0..40)
            .map(|index| format!("[Menu item {index}](https://example.com/{index})"))
            .collect::<Vec<_>>()
            .join("\n");
        let quality = assess_quality("Directory", &navigation, "");
        assert!(quality.score < HIGH_QUALITY_THRESHOLD);
        assert!(
            quality
                .reasons
                .iter()
                .any(|reason| reason == "navigation_or_menu_noise")
        );
    }

    #[test]
    fn sensitive_urls_stay_local() {
        assert!(!remote_url_is_shareable(
            "https://example.com/file?access_token=secret"
        ));
        assert!(remote_url_is_shareable("https://example.com/article?id=42"));
        assert!(!remote_url_is_shareable("http://127.0.0.1/private"));
    }

    #[tokio::test]
    async fn private_targets_stop_before_local_or_remote_fetches() {
        let data_dir = tempfile::tempdir().unwrap();
        let execution = execute_web_extract_with_status_updates(
            WebExtractArgs {
                urls: vec!["http://127.0.0.1/private".to_string()],
                query: None,
                max_chars_per_url: None,
            },
            data_dir.path(),
            &EvidenceStorageConfig::default(),
            &WebSearchToolConfig::default(),
            &WebSearchProvenanceCache::default(),
        )
        .await;
        let ToolResult::Completed { stdout, .. } = execution.result else {
            panic!("private target should produce a structured extraction result");
        };
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let attempts = value["failures"][0]["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0]["extractor"], "url_validation");
    }

    #[tokio::test]
    async fn low_quality_candidates_are_not_stored_or_citation_eligible() {
        let data_dir = tempfile::tempdir().unwrap();
        let candidate = ExtractCandidate {
            requested_url: "https://example.com/article".to_string(),
            final_url: "https://example.com/article".to_string(),
            title: "Sign in".to_string(),
            content_type: "text/plain".to_string(),
            content_format: "text".to_string(),
            content: "Please sign in to continue reading this article.".to_string(),
            published_at: None,
            extractor: "local".to_string(),
            evidence_class: WebExtractEvidenceClass::LocalDirect,
            quality: assess_quality(
                "Sign in",
                "Please sign in to continue reading this article.",
                "",
            ),
        };
        let document = finalize_candidate(
            candidate,
            Vec::new(),
            "",
            DEFAULT_MAX_CHARS_PER_URL,
            data_dir.path(),
            true,
        )
        .await;
        assert!(!document.citation_eligible);
        assert!(!document.stored);
        assert!(!data_dir.path().join("evidence").exists());
    }

    #[test]
    fn records_search_source_provenance() {
        let cache = WebSearchProvenanceCache::default();
        record_web_search_provenance(
            &cache,
            &ToolResult::Completed {
                stdout: serde_json::json!({
                    "results": [{
                        "title": "Cached article",
                        "url": "https://example.com/a?utm_source=x",
                        "source_ids": ["exa", "jina"],
                        "content_excerpt": "Cached direct page content from the search provider."
                    }]
                })
                .to_string(),
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 0,
                runtime: Some("builtin_web_search".to_string()),
                cwd: None,
                output_cursor: 0,
                structured_content: None,
                is_error: false,
            },
        );
        assert_eq!(
            cache.get("https://example.com/a").unwrap().source_ids,
            vec!["exa", "jina"]
        );
        assert_eq!(
            cache
                .get("https://example.com/a")
                .unwrap()
                .cached_content
                .as_ref()
                .unwrap()
                .title,
            "Cached article"
        );
    }

    #[tokio::test]
    async fn cleanup_removes_expired_files_and_enforces_capacity() {
        let data_dir = tempfile::tempdir().unwrap();
        let evidence_dir = data_dir.path().join("evidence");
        tokio::fs::create_dir_all(&evidence_dir).await.unwrap();

        let expired_path = evidence_dir.join("expired.json");
        tokio::fs::write(&expired_path, vec![b'x'; 32])
            .await
            .unwrap();
        let expired_file = std::fs::OpenOptions::new()
            .write(true)
            .open(&expired_path)
            .unwrap();
        expired_file
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH)
                    .set_accessed(SystemTime::UNIX_EPOCH),
            )
            .unwrap();

        for index in 0..3 {
            tokio::fs::write(
                evidence_dir.join(format!("current-{index}.json")),
                vec![b'x'; 700 * 1024],
            )
            .await
            .unwrap();
        }
        tokio::fs::write(evidence_dir.join("keep.txt"), b"unrelated")
            .await
            .unwrap();

        let config = EvidenceStorageConfig {
            enabled: true,
            retention_days: 30,
            max_total_mb: 1,
        };
        let stats = cleanup_evidence_store(data_dir.path(), &config)
            .await
            .unwrap();

        assert!(!expired_path.exists());
        assert!(evidence_dir.join("keep.txt").exists());
        assert!(stats.removed_files >= 3);
        assert!(stats.remaining_bytes <= 1024 * 1024);
    }
}
