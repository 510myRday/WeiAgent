use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::domain::tools::{WebSearchKeyOperation, WebSearchKeyStatusUpdate, mask_api_key};
use crate::tools::web_search::providers::{GrokDispatch, dispatch_probe, grok_support};
use crate::tools::web_search::types::{ProviderSearchRequest, SearchMode};
use crate::{
    app::AppState,
    domain::{WebSearchKeyStatusRecord, WebSearchProviderKind, WebSearchSourceConfig},
    error::{AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct CheckWebSearchKeysRequest {
    pub source_id: String,
    #[serde(default)]
    pub key_indices: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct WebSearchKeyCheckResult {
    pub key_index: usize,
    pub key_hint: String,
    pub key_fingerprint: String,
    pub status: String,
    pub detail: String,
    pub latency_ms: u128,
    pub checked_at: String,
}

#[derive(Debug, Serialize)]
pub struct WebSearchKeyCheckResponse {
    pub source_id: String,
    pub results: Vec<WebSearchKeyCheckResult>,
}

#[derive(Debug, Deserialize)]
pub struct ImportWebSearchKeysRequest {
    pub source_id: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub api_key_labels: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportWebSearchKeysResponse {
    pub source_id: String,
    pub imported: usize,
    pub duplicates: usize,
    pub blocked: usize,
    pub source: WebSearchSourceConfig,
}

#[derive(Debug, Deserialize)]
pub struct DeleteWebSearchKeyRequest {
    pub source_id: String,
    pub key_index: usize,
    /// Stable fingerprint returned to the client. It identifies the key even
    /// when the UI list has been reordered or another key was removed.
    #[serde(default)]
    pub key_fingerprint: Option<String>,
    /// Legacy masked key hint retained for older clients.
    #[serde(default)]
    pub key_hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeleteWebSearchKeyResponse {
    pub source_id: String,
    pub source: WebSearchSourceConfig,
}

pub async fn import_web_search_keys(
    State(state): State<AppState>,
    Json(request): Json<ImportWebSearchKeysRequest>,
) -> AppResult<Json<ImportWebSearchKeysResponse>> {
    let mut config = state.store.read_tool_defaults().await?;
    let source_index = config
        .web_search
        .sources
        .iter()
        .position(|source| source.id == request.source_id)
        .ok_or_else(|| AppError::BadRequest("web search source not found".to_string()))?;

    let source_id = config.web_search.sources[source_index].id.clone();
    let existing_keys = config.web_search.sources[source_index].api_keys.clone();
    let blocked_key_hashes = config.web_search.sources[source_index]
        .blocked_api_key_hashes
        .clone();
    let mut seen = std::collections::HashSet::new();
    let mut imported = 0;
    let mut duplicates = 0;
    let mut blocked = 0;
    let additions = request
        .api_keys
        .into_iter()
        .zip(request.api_key_labels.into_iter().chain(std::iter::repeat(String::new())))
        .map(|(key, label)| (key.trim().to_string(), label.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .filter(|(key, _)| {
            if existing_keys.iter().any(|existing| existing == key) || !seen.insert(key.clone()) {
                duplicates += 1;
                false
            } else if blocked_key_hashes
                .iter()
                .any(|hash| hash == &crate::domain::tools::api_key_fingerprint(key))
            {
                blocked += 1;
                false
            } else {
                imported += 1;
                true
            }
        })
        .collect::<Vec<_>>();

    if imported > 0 {
        let source = &mut config.web_search.sources[source_index];
        source.api_key_labels.resize(source.api_keys.len(), String::new());
        source.api_keys.extend(additions.iter().map(|(key, _)| key.clone()));
        source.api_key_labels.extend(additions.into_iter().map(|(_, label)| label));
        source
            .key_statuses
            .resize_with(source.api_keys.len(), || None);
    }

    let saved = state.store.update_tool_defaults(config).await?;
    let source = saved
        .redact_secrets()
        .web_search
        .sources
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| AppError::BadRequest("web search source not found".to_string()))?;

    Ok(Json(ImportWebSearchKeysResponse {
        source_id,
        imported,
        duplicates,
        blocked,
        source,
    }))
}

pub async fn delete_web_search_key(
    State(state): State<AppState>,
    Json(request): Json<DeleteWebSearchKeyRequest>,
) -> AppResult<Json<DeleteWebSearchKeyResponse>> {
    let mut config = state.store.read_tool_defaults().await?;
    let source_index = config
        .web_search
        .sources
        .iter()
        .position(|source| source.id == request.source_id)
        .ok_or_else(|| AppError::BadRequest("web search source not found".to_string()))?;
    let source = &mut config.web_search.sources[source_index];
    let key_index = resolve_delete_key_index(source, &request)?;
    let _removed_key = remove_web_search_key_at(source, key_index)?;
    let source_id = source.id.clone();

    let saved = state.store.update_tool_defaults(config).await?;
    let source = saved
        .redact_secrets()
        .web_search
        .sources
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| AppError::BadRequest("web search source not found".to_string()))?;

    Ok(Json(DeleteWebSearchKeyResponse { source_id, source }))
}

fn resolve_delete_key_index(
    source: &WebSearchSourceConfig,
    request: &DeleteWebSearchKeyRequest,
) -> AppResult<usize> {
    if let Some(fingerprint) = request
        .key_fingerprint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return source
            .api_keys
            .iter()
            .position(|key| crate::domain::tools::api_key_fingerprint(key) == fingerprint)
            .ok_or_else(|| AppError::Conflict("selected web search key is stale".to_string()));
    }

    let key_index = request
        .key_hint
        .as_deref()
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(|hint| {
            let matches = source
                .api_keys
                .iter()
                .enumerate()
                .filter(|(_, key)| mask_api_key(key) == hint)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [index] => Ok(*index),
                [] => Err(AppError::Conflict(
                    "selected web search key is stale".to_string(),
                )),
                _ => Err(AppError::Conflict(
                    "selected web search key is ambiguous; reload the key list".to_string(),
                )),
            }
        })
        .transpose()?
        .unwrap_or(request.key_index);
    Ok(key_index)
}

fn remove_web_search_key_at(
    source: &mut WebSearchSourceConfig,
    key_index: usize,
) -> AppResult<String> {
    if key_index >= source.api_keys.len() {
        return Err(AppError::BadRequest(
            "web search key index is out of range".to_string(),
        ));
    }

    let removed_key = source.api_keys.remove(key_index);
    if key_index < source.api_key_labels.len() {
        source.api_key_labels.remove(key_index);
    }
    let was_officially_blocked = source
        .key_statuses
        .get(key_index)
        .and_then(Option::as_ref)
        .is_some_and(|record| record.status == "officially_blocked");
    if was_officially_blocked {
        source.remember_blocked_api_key(&removed_key);
    }

    // Both status arrays are position-aligned with api_keys. Removing only the
    // search status would make extraction status belong to the wrong key.
    if key_index < source.key_statuses.len() {
        source.key_statuses.remove(key_index);
    }
    if key_index < source.extract_key_statuses.len() {
        source.extract_key_statuses.remove(key_index);
    }
    source.key_statuses.truncate(source.api_keys.len());
    source.extract_key_statuses.truncate(source.api_keys.len());
    Ok(removed_key)
}

pub async fn check_web_search_keys(
    State(state): State<AppState>,
    Json(request): Json<CheckWebSearchKeysRequest>,
) -> AppResult<Json<WebSearchKeyCheckResponse>> {
    let config = state.store.read_tool_defaults().await?;
    let source = config
        .web_search
        .sources
        .iter()
        .find(|source| source.id == request.source_id)
        .ok_or_else(|| {
            crate::error::AppError::BadRequest("web search source not found".to_string())
        })?;
    let source_id = source.id.clone();
    let api_keys = source.api_keys.clone();
    let provider = source.provider.clone();
    let source_proxy_url = source.proxy_url.clone();

    let indices: Vec<usize> = if request.key_indices.is_empty() {
        (0..api_keys.len()).collect()
    } else {
        let mut values = request.key_indices;
        values.sort_unstable();
        values.dedup();
        values
    };
    let timeout = std::time::Duration::from_millis(config.web_search.timeout_ms());
    let base_url = provider.default_base_url().to_string();
    let effective_proxy_url = source_proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| crate::domain::ip_proxies::proxy_url_with_remote_dns(value).into_owned());
    let proxy_url = effective_proxy_url.as_deref();
    let control_timeout = timeout.max(std::time::Duration::from_secs(90));
    let control_proxy = if provider != WebSearchProviderKind::Grok {
        proxy_url
    } else {
        None
    };
    let control_client =
        crate::tools::web_search::client_cache::control_client(control_timeout, control_proxy)
            .map_err(crate::error::AppError::BadRequest)?;
    let grok_client = if provider == WebSearchProviderKind::Grok {
        Some(grok_support::build_emulated_client(timeout, proxy_url))
    } else {
        None
    };
    let check_request = ProviderSearchRequest {
        query: "WeiAgent key status check".to_string(),
        max_results: 1,
        mode: SearchMode::Fast,
        allowed_domains: Vec::new(),
        blocked_domains: Vec::new(),
        recency_days: None,
    };

    const MAX_CONCURRENT_KEY_CHECKS: usize = 3;
    let mut results = Vec::new();
    for chunk in indices.chunks(MAX_CONCURRENT_KEY_CHECKS) {
        let mut futs = Vec::new();
        for &key_index in chunk {
            let api_key = api_keys.get(key_index).cloned();
            let control_client = control_client.clone();
            let grok_client = grok_client.clone();
            let base_url = base_url.clone();
            let source_id = source_id.clone();
            let provider = provider.clone();
            let check_request = check_request.clone();
            let proxy_url = effective_proxy_url.clone();
            futs.push(async move {
                let Some(api_key) = api_key else {
                    return WebSearchKeyCheckResult {
                        key_index,
                        key_hint: String::new(),
                        key_fingerprint: String::new(),
                        status: "configuration_error".to_string(),
                        detail: "key index is out of range".to_string(),
                        latency_ms: 0,
                        checked_at: chrono::Utc::now().to_rfc3339(),
                    };
                };
                let started = Instant::now();
                let key_hint = mask_api_key(&api_key);
                let search_result = dispatch_probe(
                    provider.clone(),
                    &control_client,
                    grok_client.as_ref(),
                    &base_url,
                    &api_key,
                    &check_request,
                    GrokDispatch {
                        source_id: &source_id,
                        proxy_url: proxy_url.as_deref(),
                        timeout,
                    },
                )
                .await;
                let (status, detail) = match search_result {
                    Ok(_) => ("normal".to_string(), "request succeeded".to_string()),
                    Err(detail) => {
                        let status =
                            crate::tools::web_search::classify_key_status(provider, &detail);
                        (status, detail)
                    }
                };
                WebSearchKeyCheckResult {
                    key_index,
                    key_hint,
                    key_fingerprint: crate::domain::tools::api_key_fingerprint(&api_key),
                    status,
                    detail,
                    latency_ms: started.elapsed().as_millis(),
                    checked_at: chrono::Utc::now().to_rfc3339(),
                }
            });
        }
        results.extend(futures_util::future::join_all(futs).await);
    }

    let status_updates = results
        .iter()
        .filter_map(|result| {
            let api_key = api_keys.get(result.key_index)?;
            Some(WebSearchKeyStatusUpdate {
                operation: WebSearchKeyOperation::Search,
                source_id: source_id.clone(),
                key_index: result.key_index,
                api_key: api_key.clone(),
                record: WebSearchKeyStatusRecord {
                    status: result.status.clone(),
                    detail: result.detail.clone(),
                    checked_at: result.checked_at.clone(),
                },
            })
        })
        .collect::<Vec<_>>();
    state
        .store
        .update_web_search_key_statuses(&status_updates)
        .await?;

    Ok(Json(WebSearchKeyCheckResponse { source_id, results }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: &str, detail: &str) -> Option<WebSearchKeyStatusRecord> {
        Some(WebSearchKeyStatusRecord {
            status: status.to_string(),
            detail: detail.to_string(),
            checked_at: "2026-07-28T00:00:00Z".to_string(),
        })
    }

    #[test]
    fn removing_middle_key_keeps_search_and_extract_statuses_aligned() {
        let mut source = crate::domain::tools::WebSearchToolConfig::default()
            .sources
            .into_iter()
            .find(|source| source.id == "grok")
            .expect("default Grok source");
        source.api_keys = vec![
            "key-a".to_string(),
            "key-b".to_string(),
            "key-c".to_string(),
        ];
        source.api_key_labels = vec![
            "first".to_string(),
            "middle".to_string(),
            "last".to_string(),
        ];
        source.key_statuses = vec![
            status("normal", "search-a"),
            status("anti_bot", "search-b"),
            status("quota_limited", "search-c"),
        ];
        source.extract_key_statuses = vec![
            status("normal", "extract-a"),
            status("error", "extract-b"),
            status("service_error", "extract-c"),
        ];

        assert_eq!(remove_web_search_key_at(&mut source, 1).unwrap(), "key-b");
        assert_eq!(source.api_keys, ["key-a", "key-c"]);
        assert_eq!(source.api_key_labels, ["first", "last"]);
        assert_eq!(
            source
                .key_statuses
                .iter()
                .flatten()
                .map(|record| record.detail.as_str())
                .collect::<Vec<_>>(),
            ["search-a", "search-c"]
        );
        assert_eq!(
            source
                .extract_key_statuses
                .iter()
                .flatten()
                .map(|record| record.detail.as_str())
                .collect::<Vec<_>>(),
            ["extract-a", "extract-c"]
        );
    }

    #[test]
    fn fingerprint_resolves_key_when_masked_suffix_is_duplicated() {
        let mut source = crate::domain::tools::WebSearchToolConfig::default()
            .sources
            .into_iter()
            .find(|source| source.id == "grok")
            .expect("default Grok source");
        source.api_keys = vec!["key-a-same".to_string(), "key-b-same".to_string()];

        let request = DeleteWebSearchKeyRequest {
            source_id: "grok".to_string(),
            key_index: 0,
            key_fingerprint: Some(crate::domain::tools::api_key_fingerprint("key-b-same")),
            key_hint: Some("****same".to_string()),
        };

        assert_eq!(resolve_delete_key_index(&source, &request).unwrap(), 1);
    }
}
