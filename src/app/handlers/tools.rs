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
        ApprovalRequest, ApprovalResponse, EnvironmentRuntimeStatus, McpAgentOverrides,
        McpAgentSettings, McpAuthState, McpServer, McpServerDraft, McpServerStatus, ProcessInfo,
        RuntimeComponentKind, WebSearchKeyStatusRecord, WebSearchProviderKind,
        WebSearchSourceConfig,
    },
    error::{AppError, AppResult},
    tools::approval::ApprovalResolution,
};

use crate::domain::tools::{WebSearchKeyOperation, WebSearchKeyStatusUpdate, mask_api_key};
use crate::tools::web_search::providers::{exa, firecrawl, grok, grok_support, jina, tavily};
use crate::tools::web_search::types::{ProviderSearchRequest, SearchMode};

#[derive(Debug, Deserialize)]
pub struct McpToolEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct McpToolsEnabledRequest {
    pub tool_names: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct McpServerEnabledRequest {
    pub enabled: bool,
}

#[cfg(windows)]
use {
    raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle,
        WindowHandle,
    },
    std::num::NonZeroIsize,
    windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow,
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

#[derive(Debug, Deserialize)]
pub struct PickDirectoryRequest {
    #[serde(default)]
    pub initial_directory: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PickDirectoryResponse {
    pub path: Option<String>,
}

#[cfg(windows)]
struct DialogParentWindow {
    hwnd: NonZeroIsize,
}

#[cfg(windows)]
impl DialogParentWindow {
    fn foreground() -> Option<Self> {
        let hwnd = unsafe { GetForegroundWindow() };
        NonZeroIsize::new(hwnd as isize).map(|hwnd| Self { hwnd })
    }
}

#[cfg(windows)]
impl HasWindowHandle for DialogParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(self.hwnd);
        // The HWND is borrowed only while rfd copies it into the dialog options.
        Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
    }
}

#[cfg(windows)]
impl HasDisplayHandle for DialogParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
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
    if config.evidence_storage.enabled
        && let Err(error) = crate::tools::web_search::evidence::cleanup_evidence_store(
            &state.store.data_dir,
            &config.evidence_storage,
        )
        .await
    {
        tracing::warn!(error = %error, "failed to clean evidence store after settings update");
    }
    Ok(Json(config.redact_secrets()))
}

pub async fn list_mcp_servers(State(state): State<AppState>) -> AppResult<Json<Vec<McpServer>>> {
    let servers = state
        .mcp_manager
        .list_servers()
        .await?
        .into_iter()
        .map(|server| server.redacted())
        .collect();
    Ok(Json(servers))
}

pub async fn read_mcp_server_secrets(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<crate::domain::McpTransport>> {
    let transport = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|server| server.id == server_id)
        .map(|server| server.transport)
        .ok_or(AppError::NotFound)?;
    Ok(Json(transport))
}

pub async fn create_mcp_server(
    State(state): State<AppState>,
    Json(draft): Json<McpServerDraft>,
) -> AppResult<Json<McpServer>> {
    let mut server = state.store.create_mcp_server(draft).await?;
    let server_id = server.id;
    if server.enabled {
        let _ = state
            .store
            .update_mcp_server_status(server_id, McpServerStatus::Connecting, None, None)
            .await;
        server.status = McpServerStatus::Connecting;
        server.last_error = None;
    }
    let mcp_manager = state.mcp_manager.clone();
    tokio::spawn(async move {
        refresh_mcp_server_in_background(mcp_manager, server_id).await;
    });
    Ok(Json(server.redacted()))
}

pub async fn update_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(mut draft): Json<McpServerDraft>,
) -> AppResult<Json<McpServer>> {
    draft.id = Some(server_id);
    let mut server = state.store.update_mcp_server(draft).await?;
    if server.enabled {
        let _ = state
            .store
            .update_mcp_server_status(server_id, McpServerStatus::Connecting, None, None)
            .await;
        server.status = McpServerStatus::Connecting;
        server.last_error = None;
    }
    let mcp_manager = state.mcp_manager.clone();
    tokio::spawn(async move {
        refresh_mcp_server_in_background(mcp_manager, server_id).await;
    });
    Ok(Json(server.redacted()))
}

async fn refresh_mcp_server_in_background(
    mcp_manager: std::sync::Arc<crate::tools::mcp::McpManager>,
    server_id: Uuid,
) {
    mcp_manager.stop_server(server_id).await;
    let enabled = match mcp_manager.list_servers().await {
        Ok(servers) => servers
            .into_iter()
            .find(|server| server.id == server_id)
            .is_some_and(|server| server.enabled),
        Err(error) => {
            tracing::debug!(%error, %server_id, "MCP background refresh could not read server state");
            return;
        }
    };
    if !enabled {
        return;
    }
    if let Err(error) = mcp_manager.refresh_server(server_id).await {
        tracing::debug!(%error, %server_id, "MCP background refresh failed");
    }
}

pub async fn update_mcp_server_enabled(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(request): Json<McpServerEnabledRequest>,
) -> AppResult<Json<McpServer>> {
    if !request.enabled {
        state.mcp_manager.stop_server(server_id).await;
    }
    let server = state
        .store
        .update_mcp_server_enabled(server_id, request.enabled)
        .await?;
    Ok(Json(server.redacted()))
}

pub async fn delete_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<()> {
    state.mcp_manager.delete_server(server_id).await
}

pub async fn refresh_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpServer>> {
    Ok(Json(
        state
            .mcp_manager
            .refresh_server(server_id)
            .await?
            .redacted(),
    ))
}

pub async fn test_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpServer>> {
    Ok(Json(
        state
            .mcp_manager
            .refresh_server(server_id)
            .await?
            .redacted(),
    ))
}

pub async fn get_mcp_auth_state(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpAuthState>> {
    Ok(Json(state.mcp_manager.auth_state(server_id).await?))
}

pub async fn reauthorize_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpAuthState>> {
    Ok(Json(state.mcp_manager.reauthorize(server_id).await?))
}

pub async fn clear_mcp_auth(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
) -> AppResult<Json<McpAuthState>> {
    Ok(Json(state.mcp_manager.clear_auth(server_id).await?))
}

pub async fn update_mcp_tool_enabled(
    State(state): State<AppState>,
    Path((server_id, tool_name)): Path<(Uuid, String)>,
    Json(request): Json<McpToolEnabledRequest>,
) -> AppResult<Json<McpServer>> {
    state
        .store
        .update_mcp_tool_enabled(server_id, &tool_name, request.enabled)
        .await?;
    let server = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|item| item.id == server_id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(server.redacted()))
}

pub async fn update_mcp_tools_enabled(
    State(state): State<AppState>,
    Path(server_id): Path<Uuid>,
    Json(request): Json<McpToolsEnabledRequest>,
) -> AppResult<Json<McpServer>> {
    let names = request
        .tool_names
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err(AppError::BadRequest(
            "at least one MCP tool is required".to_string(),
        ));
    }
    state
        .store
        .update_mcp_tools_enabled(server_id, &names, request.enabled)
        .await?;
    let server = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|item| item.id == server_id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(server.redacted()))
}

pub async fn read_agent_mcp_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<McpAgentSettings>> {
    state.store.get_agent(agent_id).await?;
    let settings = state.store.read_mcp_agent_settings(agent_id).await?;
    Ok(Json(McpAgentSettings {
        defaults: settings
            .defaults
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
        overrides: settings.overrides,
        effective: settings
            .effective
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
    }))
}

pub async fn update_agent_mcp_config(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Json(overrides): Json<McpAgentOverrides>,
) -> AppResult<Json<McpAgentSettings>> {
    state.store.get_agent(agent_id).await?;
    let settings = state
        .store
        .update_mcp_agent_overrides(agent_id, overrides)
        .await?;
    Ok(Json(McpAgentSettings {
        defaults: settings
            .defaults
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
        overrides: settings.overrides,
        effective: settings
            .effective
            .into_iter()
            .map(|server| server.redacted())
            .collect(),
    }))
}

pub async fn pick_directory(
    Json(request): Json<PickDirectoryRequest>,
) -> AppResult<Json<PickDirectoryResponse>> {
    let mut dialog = rfd::AsyncFileDialog::new();
    #[cfg(windows)]
    let parent_window = DialogParentWindow::foreground();
    #[cfg(windows)]
    if let Some(parent_window) = parent_window.as_ref() {
        dialog = dialog.set_parent(parent_window);
    }
    if let Some(initial_directory) = request
        .initial_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let initial_directory = std::path::Path::new(initial_directory);
        if initial_directory.is_dir() {
            dialog = dialog.set_directory(initial_directory);
        }
    }

    let path = dialog
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().into_owned());
    Ok(Json(PickDirectoryResponse { path }))
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
        .position(|source| source.id == request.source_id)
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
    let mut control_client_builder =
        reqwest::Client::builder().timeout(timeout.max(std::time::Duration::from_secs(90)));
    if provider != WebSearchProviderKind::Grok
        && let Some(proxy_url) = proxy_url.filter(|value| !value.trim().is_empty())
    {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|error| crate::error::AppError::BadRequest(error.to_string()))?;
        control_client_builder = control_client_builder.proxy(proxy);
    }
    let control_client = control_client_builder
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
        mode: SearchMode::Fast,
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
                    .map(|_| ())
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Exa => {
                exa::search(&control_client, &base_url, api_key, &check_request)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Jina => {
                jina::search(&control_client, &base_url, api_key, &check_request)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Firecrawl => {
                firecrawl::search(&control_client, &base_url, api_key, &check_request)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.message().to_string())
            }
            WebSearchProviderKind::Grok => match grok_client.as_ref() {
                Some(Ok(client)) => grok::probe(
                    client,
                    &control_client,
                    &base_url,
                    api_key,
                    grok::GrokSearchOptions {
                        source_id: &source_id,
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

pub async fn read_environment_runtime(
    State(state): State<AppState>,
) -> AppResult<Json<EnvironmentRuntimeStatus>> {
    Ok(Json(state.environment_runtime.status().await))
}

pub async fn install_environment_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<EnvironmentRuntimeStatus>> {
    Ok(Json(
        state.environment_runtime.start_install(component).await?,
    ))
}

pub async fn refresh_environment_component(
    State(state): State<AppState>,
    Path(component): Path<RuntimeComponentKind>,
) -> AppResult<Json<EnvironmentRuntimeStatus>> {
    Ok(Json(
        state
            .environment_runtime
            .refresh_component(component)
            .await?,
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

#[derive(Debug, Deserialize)]
pub struct TerminalInputRequest {
    pub data: String,
    #[serde(default)]
    pub close_stdin: bool,
}

#[derive(Debug, Deserialize)]
pub struct TerminalResizeRequest {
    pub rows: u16,
    pub cols: u16,
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

pub async fn list_terminal_sessions(
    State(state): State<AppState>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<Vec<ProcessInfo>>> {
    list_tool_processes(State(state), Query(query)).await
}

pub async fn get_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    get_tool_process(State(state), Path(process_id), Query(query)).await
}

pub async fn write_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
    Json(request): Json<TerminalInputRequest>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        state
            .terminal_manager
            .write(&process_id, request.data.as_bytes(), request.close_stdin)
            .await?,
    ))
}

pub async fn resize_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
    Json(request): Json<TerminalResizeRequest>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        state
            .terminal_manager
            .resize(&process_id, request.rows, request.cols)
            .await?,
    ))
}

pub async fn interrupt_terminal_session(
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
    Ok(Json(state.terminal_manager.interrupt(&process_id).await?))
}

pub async fn terminate_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    stop_tool_process(State(state), Path(process_id), Query(query)).await
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
}
