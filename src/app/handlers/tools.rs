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

use crate::domain::tools::mask_api_key;
use crate::tools::web_search::providers::{grok, grok_support};

#[derive(Debug, Deserialize)]
pub struct CheckGrokKeysRequest {
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub key_indices: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct GrokKeyCheckResult {
    pub key_index: usize,
    pub key_hint: String,
    pub status: String,
    pub detail: String,
    pub latency_ms: u128,
    pub checked_at: String,
}

#[derive(Debug, Serialize)]
pub struct GrokKeyCheckResponse {
    pub source_id: String,
    pub results: Vec<GrokKeyCheckResult>,
}

#[derive(Debug, Deserialize)]
pub struct ImportGrokKeysRequest {
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub api_keys: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportGrokKeysResponse {
    pub source_id: String,
    pub imported: usize,
    pub duplicates: usize,
    pub source: WebSearchSourceConfig,
}

#[derive(Debug, Deserialize)]
pub struct DeleteGrokKeyRequest {
    #[serde(default)]
    pub source_id: Option<String>,
    pub key_index: usize,
}

#[derive(Debug, Serialize)]
pub struct DeleteGrokKeyResponse {
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
    Ok(Json(config.redact_secrets()))
}

pub async fn import_grok_keys(
    State(state): State<AppState>,
    Json(request): Json<ImportGrokKeysRequest>,
) -> AppResult<Json<ImportGrokKeysResponse>> {
    let mut config = state.store.read_tool_defaults().await?;
    let source_index = config
        .web_search
        .sources
        .iter()
        .position(|source| {
            source.provider == WebSearchProviderKind::Grok
                && request
                    .source_id
                    .as_deref()
                    .map(|id| id == source.id)
                    .unwrap_or(true)
        })
        .ok_or_else(|| AppError::BadRequest("Grok source not found".to_string()))?;

    let source_id = config.web_search.sources[source_index].id.clone();
    let existing_keys = config.web_search.sources[source_index].api_keys.clone();
    let mut seen = std::collections::HashSet::new();
    let mut imported = 0;
    let mut duplicates = 0;
    let additions = request
        .api_keys
        .into_iter()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .filter(|key| {
            if existing_keys.iter().any(|existing| existing == key) || !seen.insert(key.clone()) {
                duplicates += 1;
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
        .ok_or_else(|| AppError::BadRequest("Grok source not found".to_string()))?;

    Ok(Json(ImportGrokKeysResponse {
        source_id,
        imported,
        duplicates,
        source,
    }))
}

pub async fn delete_grok_key(
    State(state): State<AppState>,
    Json(request): Json<DeleteGrokKeyRequest>,
) -> AppResult<Json<DeleteGrokKeyResponse>> {
    let mut config = state.store.read_tool_defaults().await?;
    let source_index = config
        .web_search
        .sources
        .iter()
        .position(|source| {
            source.provider == WebSearchProviderKind::Grok
                && request
                    .source_id
                    .as_deref()
                    .map(|id| id == source.id)
                    .unwrap_or(true)
        })
        .ok_or_else(|| AppError::BadRequest("Grok source not found".to_string()))?;
    let source = &mut config.web_search.sources[source_index];
    if request.key_index >= source.api_keys.len() {
        return Err(AppError::BadRequest("Grok key index is out of range".to_string()));
    }
    source.api_keys.remove(request.key_index);
    if request.key_index < source.key_statuses.len() {
        source.key_statuses.remove(request.key_index);
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
        .ok_or_else(|| AppError::BadRequest("Grok source not found".to_string()))?;

    Ok(Json(DeleteGrokKeyResponse { source_id, source }))
}

pub async fn check_grok_keys(
    State(state): State<AppState>,
    Json(request): Json<CheckGrokKeysRequest>,
) -> AppResult<Json<GrokKeyCheckResponse>> {
    let mut config = state.store.read_tool_defaults().await?;
    let source = config
        .web_search
        .sources
        .iter()
        .find(|source| {
            source.provider == WebSearchProviderKind::Grok
                && request
                    .source_id
                    .as_deref()
                    .map(|id| id == source.id)
                    .unwrap_or(true)
        })
        .ok_or_else(|| crate::error::AppError::BadRequest("Grok source not found".to_string()))?;
    let source_id = source.id.clone();
    let source_index = config
        .web_search
        .sources
        .iter()
        .position(|candidate| candidate.id == source_id)
        .ok_or_else(|| crate::error::AppError::BadRequest("Grok source not found".to_string()))?;
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
    let mut results = Vec::new();

    for key_index in indices {
        let Some(api_key) = api_keys.get(key_index) else {
            results.push(GrokKeyCheckResult {
                key_index,
                key_hint: String::new(),
                status: "error".to_string(),
                detail: "key index is out of range".to_string(),
                latency_ms: 0,
                checked_at: chrono::Utc::now().to_rfc3339(),
            });
            continue;
        };
        let started = Instant::now();
        let key_hint = mask_api_key(api_key);
        let (status, detail) = match grok_support::build_emulated_client(timeout, proxy_url) {
            Ok(client) => match grok::search(
                &client,
                &control_client,
                &base_url,
                api_key,
                "WeiAgent Grok account status check",
                1,
                flaresolverr_url,
                flaresolverr_proxy_url,
                proxy_url,
            )
            .await
            {
                Ok(_) => ("normal".to_string(), "request succeeded".to_string()),
                Err(error) => {
                    let detail = error.message().to_string();
                    (grok::classify_key_status(&detail).to_string(), detail)
                }
            },
            Err(error) => ("network_error".to_string(), error),
        };
        results.push(GrokKeyCheckResult {
            key_index,
            key_hint,
            status,
            detail,
            latency_ms: started.elapsed().as_millis(),
            checked_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    let mut key_statuses = config.web_search.sources[source_index].key_statuses.clone();
    key_statuses.resize_with(api_keys.len(), || None);
    for result in &results {
        if result.key_index < key_statuses.len() {
            key_statuses[result.key_index] = Some(WebSearchKeyStatusRecord {
                status: result.status.clone(),
                detail: result.detail.clone(),
                checked_at: result.checked_at.clone(),
            });
        }
    }
    config.web_search.sources[source_index].key_statuses = key_statuses;
    state.store.update_tool_defaults(config).await?;

    Ok(Json(GrokKeyCheckResponse {
        source_id,
        results,
    }))
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

