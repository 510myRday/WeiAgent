use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings, ApprovalDecision,
        ApprovalRequest, ApprovalResponse, CodexRuntimeStatus, ProcessInfo, RuntimeComponentKind,
        WebSearchKeyStatusRecord, WebSearchProviderKind, WebSearchSourceConfig,
    },
    error::{AppError, AppResult},
    tools::approval::ApprovalResolution,
};

use crate::domain::tools::{WebSearchKeyStatusUpdate, mask_api_key};
use crate::tools::web_search::providers::{exa, grok, grok_support, jina, tavily};
use crate::tools::web_search::types::ProviderSearchRequest;

#[derive(Debug, Deserialize)]
pub struct CheckWebSearchKeysRequest {
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub key_indices: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct WebSearchKeyCheckResult {
    pub key_index: usize,
    pub key_hint: String,
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
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub api_keys: Vec<String>,
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
    #[serde(default)]
    pub source_id: Option<String>,
    pub key_index: usize,
    /// Masked key returned to the client. It lets the server resolve the
    /// intended key when a stale UI index no longer points at the same slot.
    #[serde(default)]
    pub key_hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeleteWebSearchKeyResponse {
    pub source_id: String,
    pub source: WebSearchSourceConfig,
}

pub async fn read_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentToolsSettings>> {
    let settings = state.store.read_agent_tools_settings(agent_id).await?;
    Ok(Json(settings.redact_secrets()))
}

pub async fn update_agent_tools_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(overrides): Json<AgentToolsOverrides>,
) -> AppResult<Json<AgentToolsSettings>> {
    let settings = state
        .store
        .update_agent_tools_overrides(agent_id, overrides)
        .await?;
    Ok(Json(settings.redact_secrets()))
}

pub async fn read_tool_defaults(
    State(state): State<AppState>,
) -> AppResult<Json<AgentToolsConfig>> {
    let config = state.store.read_tool_defaults().await?;
    Ok(Json(config.redact_secrets()))
}

pub async fn update_tool_defaults(
    State(state): State<AppState>,
    Json(config): Json<AgentToolsConfig>,
) -> AppResult<Json<AgentToolsConfig>> {
    let config = state.store.update_tool_defaults(config).await?;
    if config.evidence_storage.enabled {
        if let Err(error) = crate::tools::web_search::evidence::cleanup_evidence_store(
            &state.store.data_dir,
            &config.evidence_storage,
        )
        .await
        {
            tracing::warn!(error = %error, "failed to clean evidence store after settings update");
        }
    }
    Ok(Json(config.redact_secrets()))
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
        .position(|source| {
            request
                .source_id
                .as_deref()
                .map(|id| id == source.id)
                .unwrap_or(source.provider == WebSearchProviderKind::Grok)
        })
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
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .filter(|key| {
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
        source.api_keys.extend(additions);
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
        .position(|source| {
            request
                .source_id
                .as_deref()
                .map(|id| id == source.id)
                .unwrap_or(source.provider == WebSearchProviderKind::Grok)
        })
        .ok_or_else(|| AppError::BadRequest("web search source not found".to_string()))?;
    let source = &mut config.web_search.sources[source_index];
    let key_index = request
        .key_hint
        .as_deref()
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(|hint| {
            if let Some(key) = source.api_keys.get(request.key_index)
                && mask_api_key(key) == hint
            {
                return Ok(request.key_index);
            }

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
    if key_index >= source.api_keys.len() {
        return Err(AppError::BadRequest(
            "web search key index is out of range".to_string(),
        ));
    }
    let removed_key = source.api_keys[key_index].clone();
    let was_officially_blocked = source
        .key_statuses
        .get(key_index)
        .and_then(Option::as_ref)
        .is_some_and(|record| record.status == "officially_blocked");
    if was_officially_blocked {
        source.remember_blocked_api_key(&removed_key);
    }
    source.api_keys.remove(key_index);
    if key_index < source.key_statuses.len() {
        source.key_statuses.remove(key_index);
    }
    source.key_statuses.truncate(source.api_keys.len());
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

pub async fn check_web_search_keys(
    State(state): State<AppState>,
    Json(request): Json<CheckWebSearchKeysRequest>,
) -> AppResult<Json<WebSearchKeyCheckResponse>> {
    let config = state.store.read_tool_defaults().await?;
    let source = config
        .web_search
        .sources
        .iter()
        .find(|source| {
            request
                .source_id
                .as_deref()
                .map(|id| id == source.id)
                .unwrap_or(source.provider == WebSearchProviderKind::Grok)
        })
        .ok_or_else(|| {
            crate::error::AppError::BadRequest("web search source not found".to_string())
        })?;
    let source_id = source.id.clone();
    let api_keys = source.api_keys.clone();
    let provider = source.provider.clone();
    let source_base_url = source.base_url.clone();
    let source_proxy_url = source.proxy_url.clone();
    let source_flaresolverr_url = source.flaresolverr_url.clone();
    let source_flaresolverr_proxy_url = source.flaresolverr_proxy_url.clone();

    let indices: Vec<usize> = if request.key_indices.is_empty() {
        (0..api_keys.len()).collect()
    } else {
        let mut values = request.key_indices;
        values.sort_unstable();
        values.dedup();
        values
    };
    let timeout = std::time::Duration::from_millis(config.web_search.timeout_ms());
    let base_url = source_base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| provider.default_base_url())
        .to_string();
    let proxy_url = source_proxy_url.as_deref();
    let flaresolverr_url = source_flaresolverr_url.as_deref();
    let flaresolverr_proxy_url = source_flaresolverr_proxy_url.as_deref();
    let control_client = reqwest::Client::builder()
        .timeout(timeout.max(std::time::Duration::from_secs(90)))
        .build()
        .map_err(|error| crate::error::AppError::BadRequest(error.to_string()))?;
    let grok_client = if provider == WebSearchProviderKind::Grok {
        Some(grok_support::build_emulated_client(timeout, proxy_url))
    } else {
        None
    };
    let mut results = Vec::new();
    let check_request = ProviderSearchRequest {
        query: "WeiAgent key status check".to_string(),
        max_results: 1,
        allowed_domains: Vec::new(),
        blocked_domains: Vec::new(),
        recency_days: None,
    };

    for key_index in indices {
        let Some(api_key) = api_keys.get(key_index) else {
            results.push(WebSearchKeyCheckResult {
                key_index,
                key_hint: String::new(),
                status: "configuration_error".to_string(),
                detail: "key index is out of range".to_string(),
                latency_ms: 0,
                checked_at: chrono::Utc::now().to_rfc3339(),
            });
            continue;
        };
        let started = Instant::now();
        let key_hint = mask_api_key(api_key);
        let search_result = match &provider {
            WebSearchProviderKind::Tavily => {
                tavily::search(&control_client, &base_url, api_key, &check_request)
                    .await
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Exa => {
                exa::search(&control_client, &base_url, api_key, &check_request)
                    .await
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Jina => {
                jina::search(&control_client, &base_url, api_key, &check_request)
                    .await
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Grok => match grok_client.as_ref() {
                Some(Ok(client)) => grok::search(
                    client,
                    &control_client,
                    &base_url,
                    api_key,
                    &check_request,
                    grok::GrokSearchOptions {
                        source_id: &source_id,
                        flaresolverr_url,
                        flaresolverr_proxy_url,
                        proxy_url,
                    },
                )
                .await
                .map_err(|error| error.message().to_string()),
                Some(Err(error)) => Err(error.clone()),
                None => Err("Grok emulated client missing".to_string()),
            },
        };
        let (status, detail) = match search_result {
            Ok(_) => ("normal".to_string(), "request succeeded".to_string()),
            Err(detail) => {
                let status =
                    crate::tools::web_search::classify_key_status(provider.clone(), &detail);
                (status, detail)
            }
        };
        results.push(WebSearchKeyCheckResult {
            key_index,
            key_hint,
            status,
            detail,
            latency_ms: started.elapsed().as_millis(),
            checked_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    let status_updates = results
        .iter()
        .filter_map(|result| {
            let api_key = api_keys.get(result.key_index)?;
            Some(WebSearchKeyStatusUpdate {
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

pub async fn read_codex_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(state.codex_runtime.status().await))
}

pub async fn refresh_codex_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(state.codex_runtime.refresh().await?))
}

pub async fn install_codex_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(
        state
            .codex_runtime
            .start_install(RuntimeComponentKind::Codex)
            .await?,
    ))
}

pub async fn install_runtime_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(state.codex_runtime.start_install(component).await?))
}

pub async fn refresh_runtime_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<CodexRuntimeStatus>> {
    Ok(Json(
        state.codex_runtime.refresh_component(component).await?,
    ))
}

pub async fn approve_tool_call(
    State(state): State<AppState>,
    Json(request): Json<ApprovalRequest>,
) -> AppResult<Json<ApprovalResponse>> {
    let resolution = state.approval_manager.resolve(
        &request.approval_id,
        request.agent_id,
        request.decision.clone(),
    );
    match resolution {
        ApprovalResolution::Resolved(entry) => match request.decision {
            ApprovalDecision::Approve => Ok(Json(ApprovalResponse {
                status: "approved".to_string(),
                message: Some(approval_summary("Command approved", &entry)),
            })),
            ApprovalDecision::Reject => Ok(Json(ApprovalResponse {
                status: "rejected".to_string(),
                message: Some(approval_summary("Command rejected by user", &entry)),
            })),
            ApprovalDecision::AlwaysApprove => Ok(Json(ApprovalResponse {
                status: "always_approved".to_string(),
                message: Some(approval_summary(
                    "Command approved and a 24-hour context-bound grant will be stored",
                    &entry,
                )),
            })),
        },
        ApprovalResolution::AlreadyResolved(decision) => Ok(Json(ApprovalResponse {
            status: approval_status(&decision).to_string(),
            message: Some("Approval was already processed".to_string()),
        })),
        ApprovalResolution::AgentMismatch => Err(AppError::BadRequest(
            "Approval belongs to another agent".to_string(),
        )),
        ApprovalResolution::NotFound => Err(AppError::BadRequest(format!(
            "Approval '{}' not found or expired",
            request.approval_id
        ))),
    }
}

fn approval_status(decision: &ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approve => "approved",
        ApprovalDecision::Reject => "rejected",
        ApprovalDecision::AlwaysApprove => "always_approved",
    }
}

fn approval_summary(prefix: &str, entry: &crate::domain::PendingApproval) -> String {
    format!(
        "{prefix}: {} (agent={}, conversation={}, call={}, cwd={}, runtime={}, permission={}, justification={}, background={})",
        entry.command,
        entry.agent_id,
        entry.conversation_id,
        entry.tool_call_id,
        entry.cwd.as_deref().unwrap_or("<default>"),
        entry.runtime,
        entry.permission,
        entry.justification.as_deref().unwrap_or("<none>"),
        entry.background,
    )
}

#[derive(Debug, Deserialize)]
pub struct ProcessListQuery {
    pub agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ProcessOwnerQuery {
    pub agent_id: Uuid,
}

pub async fn list_tool_processes(
    State(state): State<AppState>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<Vec<ProcessInfo>>> {
    Ok(Json(state.terminal_manager.list(query.agent_id).await))
}

pub async fn get_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(info))
}

pub async fn stop_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    state
        .terminal_manager
        .terminate(&process_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}
