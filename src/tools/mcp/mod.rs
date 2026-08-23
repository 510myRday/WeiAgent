use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, RwLock},
    time::Duration,
};
use uuid::Uuid;

use crate::{
    domain::{
        McpAuthState, McpServer, McpServerStatus, McpTool, McpTransport, ToolResult,
        qualify_tool_name, qualify_tool_name_with_suffix,
    },
    error::{AppError, AppResult},
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::Store,
    tools::runtime::EnvironmentRuntimeManager,
};

mod auth;
mod env;
mod http;
mod protocol;
mod stdio;

use auth::McpAuthRuntime;
use http::{rpc_sse, rpc_streamable_http};
use protocol::{
    MAX_TOOL_NAME_BYTES, MAX_TOOLS_LIST_PAGES, McpCallOutput, McpHandshake, McpRpcResponse,
    STDIO_IDLE_TTL, STDIO_MAX_RETRIES, is_retryable_stdio_error, mcp_timeout_ms,
    normalize_mcp_input_schema, status_name,
};
use stdio::{StdioSession, StdioSessionEntry, rpc_stdio_once, stdio_server_signature};

pub use protocol::is_mcp_tool;

#[derive(Clone)]
pub struct McpManager {
    store: Arc<Store>,
    environment: Arc<EnvironmentRuntimeManager>,
    snapshots: Arc<RwLock<HashMap<Uuid, Vec<McpServer>>>>,
    stdio_sessions: Arc<Mutex<HashMap<Uuid, Arc<StdioSessionEntry>>>>,
    auth: Arc<McpAuthRuntime>,
}

impl McpManager {
    pub fn new(store: Arc<Store>, environment: Arc<EnvironmentRuntimeManager>) -> Self {
        let auth_root = store.data_dir.join("mcp-auth");
        Self {
            store,
            environment,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            stdio_sessions: Arc::new(Mutex::new(HashMap::new())),
            auth: Arc::new(McpAuthRuntime::new(auth_root)),
        }
    }

    pub async fn stop_server(&self, server_id: Uuid) {
        let entry = self.stdio_sessions.lock().await.remove(&server_id);
        if let Some(entry) = entry {
            entry.session.lock().await.stop().await;
        }
    }

    pub async fn cleanup_loop(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            self.reap_idle_stdio_sessions().await;
        }
    }

    pub async fn delete_server(&self, server_id: Uuid) -> AppResult<()> {
        let _ = self.clear_auth(server_id).await?;
        self.store.delete_mcp_server(server_id).await?;
        self.snapshots
            .write()
            .await
            .values_mut()
            .for_each(|servers| servers.retain(|server| server.id != server_id));
        Ok(())
    }

    pub async fn auth_state(&self, server_id: Uuid) -> AppResult<McpAuthState> {
        let server = self
            .store
            .list_mcp_servers()
            .await?
            .into_iter()
            .find(|item| item.id == server_id)
            .ok_or(AppError::NotFound)?;
        Ok(self
            .auth
            .state(
                server.id,
                matches!(server.transport, McpTransport::Stdio { .. })
                    || matches!(
                        &server.transport,
                        McpTransport::StreamableHttp { oauth: Some(_), .. }
                            | McpTransport::Sse { oauth: Some(_), .. }
                    ),
            )
            .await)
    }

    pub async fn clear_auth(&self, server_id: Uuid) -> AppResult<McpAuthState> {
        self.stop_server(server_id).await;
        let _ = self
            .store
            .list_mcp_servers()
            .await?
            .into_iter()
            .find(|item| item.id == server_id)
            .ok_or(AppError::NotFound)?;
        Ok(self.auth.clear(server_id).await)
    }

    pub async fn reauthorize(&self, server_id: Uuid) -> AppResult<McpAuthState> {
        let _ = self.clear_auth(server_id).await?;
        let server = self
            .store
            .list_mcp_servers()
            .await?
            .into_iter()
            .find(|server| server.id == server_id)
            .ok_or(AppError::NotFound)?;
        if matches!(
            &server.transport,
            McpTransport::StreamableHttp { oauth: Some(_), .. }
                | McpTransport::Sse { oauth: Some(_), .. }
        ) {
            return self.auth.start_oauth(&server).await;
        }
        let _ = self.refresh_server(server_id).await;
        self.auth_state(server_id).await
    }

    pub async fn load_cached(&self) -> AppResult<()> {
        let servers = self.store.list_mcp_servers().await?;
        let active_server_ids = servers.iter().map(|server| server.id).collect();
        self.auth.cleanup_orphans(&active_server_ids).await;
        let mut snapshots = self.snapshots.write().await;
        snapshots.insert(Uuid::nil(), servers);
        Ok(())
    }

    pub async fn prepare_agent(&self, agent_id: Uuid) -> AppResult<()> {
        let settings = self.store.read_mcp_agent_settings(agent_id).await?;
        let refresh_ids = settings
            .effective
            .iter()
            .filter(|server| {
                server.enabled
                    && (server.tools.is_empty()
                        || matches!(
                            server.status,
                            McpServerStatus::Unknown
                                | McpServerStatus::Offline
                                | McpServerStatus::Error
                        ))
            })
            .map(|server| server.id)
            .collect::<Vec<_>>();
        let _ = futures_util::stream::iter(refresh_ids)
            .map(|server_id| async move { self.refresh_server(server_id).await })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        let updated = self.store.read_mcp_agent_settings(agent_id).await?;
        self.snapshots
            .write()
            .await
            .insert(agent_id, updated.effective);
        Ok(())
    }

    /// Copy the prepared MCP state for one agent so a running agent loop can
    /// keep using the same tool set while settings change in the background.
    pub async fn snapshot(&self, agent_id: Uuid) -> Vec<McpServer> {
        self.snapshots
            .read()
            .await
            .get(&agent_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn list_servers(&self) -> AppResult<Vec<McpServer>> {
        self.store.list_mcp_servers().await
    }

    pub async fn refresh_server(&self, server_id: Uuid) -> AppResult<McpServer> {
        let server = self
            .store
            .list_mcp_servers()
            .await?
            .into_iter()
            .find(|item| item.id == server_id)
            .ok_or(AppError::NotFound)?;
        self.store
            .update_mcp_server_status(server_id, McpServerStatus::Connecting, None, None)
            .await?;
        let started = Instant::now();
        match discover_tools(self, &server).await {
            Ok((tools, handshake)) => {
                self.store.replace_mcp_tools(server_id, &tools).await?;
                let mut diagnostics = server.diagnostics.clone();
                diagnostics.protocol_version = handshake.protocol_version;
                diagnostics.server_name = handshake.server_name;
                diagnostics.server_version = handshake.server_version;
                diagnostics.capabilities = handshake.capabilities;
                diagnostics.last_operation = Some("tools/list".to_string());
                diagnostics.last_operation_at = Some(chrono::Utc::now().to_rfc3339());
                diagnostics.last_duration_ms = Some(started.elapsed().as_millis() as u64);
                diagnostics.last_error_code = None;
                self.store
                    .update_mcp_server_diagnostics(server_id, diagnostics)
                    .await?;
                self.store
                    .update_mcp_server_status(
                        server_id,
                        McpServerStatus::Ready,
                        None,
                        Some(chrono::Utc::now().to_rfc3339()),
                    )
                    .await?;
            }
            Err(error) => {
                let status = if matches!(server.transport, McpTransport::Stdio { .. }) {
                    McpServerStatus::Error
                } else {
                    McpServerStatus::Offline
                };
                let status_label = status_name(status.clone());
                self.store
                    .update_mcp_server_status(server_id, status, Some(error.to_string()), None)
                    .await?;
                let mut diagnostics = server.diagnostics.clone();
                diagnostics.last_operation = Some("tools/list".to_string());
                diagnostics.last_operation_at = Some(chrono::Utc::now().to_rfc3339());
                diagnostics.last_duration_ms = Some(started.elapsed().as_millis() as u64);
                diagnostics.last_error_code = Some(status_label.to_string());
                self.store
                    .update_mcp_server_diagnostics(server_id, diagnostics)
                    .await?;
                return Err(error);
            }
        }
        self.store
            .list_mcp_servers()
            .await?
            .into_iter()
            .find(|item| item.id == server_id)
            .ok_or(AppError::NotFound)
    }

    pub fn definitions_from_snapshot(servers: &[McpServer]) -> Option<Vec<ToolDefinition>> {
        let mut definitions = Vec::new();
        for server in servers.iter().filter(|server| server.enabled) {
            for tool in server.tools.iter().filter(|tool| tool.enabled) {
                definitions.push(ToolDefinition {
                    kind: "function".to_string(),
                    function: FunctionDefinition {
                        name: tool.qualified_name.clone(),
                        description: format!("{} (MCP: {})", tool.description, server.name),
                        parameters: tool.input_schema.clone(),
                    },
                });
            }
        }
        definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));
        (!definitions.is_empty()).then_some(definitions)
    }

    pub fn supports_parallel_tool_from_snapshot(servers: &[McpServer], name: &str) -> bool {
        servers.iter().any(|server| {
            server.supports_parallel_tool_calls
                && server.enabled
                && server.tools.iter().any(|tool| {
                    tool.enabled && tool.qualified_name == name && tool.tool_id != Uuid::nil()
                })
        })
    }

    pub async fn execute_tool_from_snapshot(
        &self,
        servers: &[McpServer],
        name: &str,
        arguments: Value,
    ) -> ToolResult {
        let Some((server_id, tool_id)) = servers.iter().find_map(|server| {
            server.tools.iter().find_map(|tool| {
                (server.enabled && tool.enabled && tool.qualified_name == name)
                    .then_some((server.id, tool.tool_id))
            })
        }) else {
            return ToolResult::Blocked {
                reason: format!("MCP tool '{name}' is disabled or unavailable"),
                code: Some("McpToolUnavailable".to_string()),
                details: None,
            };
        };
        let Some(server) = servers
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
        else {
            return ToolResult::Error {
                message: format!("Unknown MCP server for tool: {name}"),
            };
        };
        let Some(tool) = server.tools.iter().find(|tool| tool.tool_id == tool_id) else {
            return ToolResult::Error {
                message: format!("Unknown MCP tool: {name}"),
            };
        };
        let started = Instant::now();
        let result = match call_tool(self, &server, tool, arguments).await {
            Ok(McpCallOutput {
                text,
                content,
                is_error,
            }) => ToolResult::Completed {
                stdout: text,
                stderr: String::new(),
                exit_code: if is_error { 1 } else { 0 },
                duration_ms: started.elapsed().as_millis() as u64,
                runtime: Some("mcp".to_string()),
                cwd: None,
                output_cursor: 0,
                structured_content: Some(content),
                is_error,
            },
            Err(error) => ToolResult::Error {
                message: error.to_string(),
            },
        };
        let mut diagnostics = server.diagnostics.clone();
        diagnostics.last_operation = Some("tools/call".to_string());
        diagnostics.last_operation_at = Some(chrono::Utc::now().to_rfc3339());
        diagnostics.last_duration_ms = Some(started.elapsed().as_millis() as u64);
        diagnostics.last_error_code =
            matches!(result, ToolResult::Error { .. }).then(|| "tool_call_failed".to_string());
        let _ = self
            .store
            .update_mcp_server_diagnostics(server.id, diagnostics)
            .await;
        result
    }
}

async fn discover_tools(
    manager: &McpManager,
    server: &McpServer,
) -> AppResult<(Vec<McpTool>, McpHandshake)> {
    let mut tools = Vec::new();
    let mut handshake = McpHandshake::default();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_TOOLS_LIST_PAGES {
        let params = cursor
            .as_deref()
            .map(|value| json!({"cursor": value}))
            .unwrap_or_else(|| json!({}));
        let response = manager
            .rpc_session(server, Some(("tools/list", params)))
            .await?;
        let result = response.value;
        if let Some(value) = response.handshake {
            handshake = value;
        }
        if let Some(page) = result.get("tools").and_then(Value::as_array) {
            tools.extend(page.iter().cloned());
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.is_empty());
        if cursor.is_none() {
            break;
        }
    }
    if cursor.is_some() {
        return Err(AppError::Process(
            "MCP tools/list exceeded the maximum page count".to_string(),
        ));
    }
    let mut used_names = HashSet::new();
    Ok((
        tools
            .into_iter()
            .filter_map(|item| {
                let name = item.get("name")?.as_str()?.to_string();
                let mut occurrence = 1;
                let qualified_name = loop {
                    let candidate = if occurrence == 1 {
                        qualify_tool_name(&server.name, &name)
                    } else {
                        qualify_tool_name_with_suffix(&server.name, &name, occurrence)
                    };
                    if used_names.insert(candidate.clone()) {
                        break candidate;
                    }
                    occurrence += 1;
                };
                Some(McpTool {
                    tool_id: Uuid::new_v4(),
                    name,
                    qualified_name: qualified_name.chars().take(MAX_TOOL_NAME_BYTES).collect(),
                    title: item
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    description: item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    input_schema: normalize_mcp_input_schema(item.get("inputSchema")),
                    enabled: true,
                    updated_at: Some(chrono::Utc::now().to_rfc3339()),
                })
            })
            .collect(),
        handshake,
    ))
}

async fn call_tool(
    manager: &McpManager,
    server: &McpServer,
    tool: &McpTool,
    arguments: Value,
) -> AppResult<McpCallOutput> {
    let response = manager
        .rpc_session(
            server,
            Some((
                "tools/call",
                json!({"name": tool.name, "arguments": arguments}),
            )),
        )
        .await?;
    let result = response.value;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let content = result.get("content").cloned().unwrap_or(Value::Null);
        return Ok(McpCallOutput {
            text: serde_json::to_string_pretty(&content).unwrap_or_else(|_| content.to_string()),
            content,
            is_error: true,
        });
    }
    let content = result
        .get("content")
        .cloned()
        .unwrap_or_else(|| result.clone());
    Ok(McpCallOutput {
        text: serde_json::to_string_pretty(&content)?,
        content,
        is_error: false,
    })
}

impl McpManager {
    async fn rpc_session(
        &self,
        server: &McpServer,
        request: Option<(&str, Value)>,
    ) -> AppResult<McpRpcResponse> {
        match &server.transport {
            McpTransport::Stdio {
                long_running: true, ..
            } => self.rpc_stdio_pooled(server, request).await,
            McpTransport::Stdio { .. } => {
                rpc_stdio_once(&self.auth, &self.environment, server, request).await
            }
            McpTransport::StreamableHttp { .. } => {
                rpc_streamable_http(&self.auth, server, request).await
            }
            McpTransport::Sse { .. } => rpc_sse(&self.auth, server, request).await,
        }
    }

    async fn rpc_stdio_pooled(
        &self,
        server: &McpServer,
        request: Option<(&str, Value)>,
    ) -> AppResult<McpRpcResponse> {
        self.reap_idle_stdio_sessions().await;
        let timeout_ms = mcp_timeout_ms(&server.transport);
        let timeout_duration = Duration::from_millis(timeout_ms.clamp(1_000, 120_000));
        let mut last_error = None;

        for attempt in 0..=STDIO_MAX_RETRIES {
            let entry = self.get_stdio_session(server, timeout_duration).await?;
            entry.touch().await;
            let result = if let Some((method, params)) = request.clone() {
                let mut session_guard = entry.session.lock().await;
                session_guard
                    .request(method, params, timeout_duration)
                    .await
            } else {
                Ok(json!({}))
            };
            match result {
                Ok(value) => {
                    entry.touch().await;
                    let handshake = entry.session.lock().await.handshake.clone();
                    return Ok(McpRpcResponse {
                        value,
                        handshake: Some(handshake),
                    });
                }
                Err(error) => {
                    if !is_retryable_stdio_error(&error) {
                        return Err(error);
                    }
                    last_error = Some(error);
                    self.invalidate_stdio_session(server.id, &entry).await;
                    if attempt < STDIO_MAX_RETRIES {
                        tokio::time::sleep(Duration::from_millis(100 * (attempt as u64 + 1))).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            AppError::Process("MCP request failed without an explicit error".to_string())
        }))
    }

    async fn get_stdio_session(
        &self,
        server: &McpServer,
        timeout_duration: Duration,
    ) -> AppResult<Arc<StdioSessionEntry>> {
        let signature = stdio_server_signature(server);
        let stale = {
            let mut sessions = self.stdio_sessions.lock().await;
            if let Some(entry) = sessions.get(&server.id) {
                if entry.signature == signature {
                    return Ok(entry.clone());
                }
            }
            sessions.remove(&server.id)
        };
        if let Some(entry) = stale {
            entry.session.lock().await.stop().await;
        }

        let session = Arc::new(Mutex::new(
            StdioSession::start(
                server,
                timeout_duration,
                self.auth.clone(),
                self.environment.clone(),
            )
            .await?,
        ));
        let entry = Arc::new(StdioSessionEntry::new(signature, session));
        let mut sessions = self.stdio_sessions.lock().await;
        if let Some(existing) = sessions.get(&server.id) {
            if existing.signature == entry.signature {
                let _ = entry.session.lock().await.stop().await;
                return Ok(existing.clone());
            }
        }
        sessions.insert(server.id, entry.clone());
        Ok(entry)
    }

    async fn invalidate_stdio_session(&self, server_id: Uuid, entry: &Arc<StdioSessionEntry>) {
        let removed = {
            let mut sessions = self.stdio_sessions.lock().await;
            if sessions
                .get(&server_id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
            {
                sessions.remove(&server_id);
                true
            } else {
                false
            }
        };
        if removed {
            entry.session.lock().await.stop().await;
        }
    }

    async fn reap_idle_stdio_sessions(&self) {
        let candidates = {
            let sessions = self.stdio_sessions.lock().await;
            sessions
                .iter()
                .map(|(server_id, entry)| (*server_id, entry.clone()))
                .collect::<Vec<_>>()
        };
        for (server_id, entry) in candidates {
            if entry.last_used.lock().await.elapsed() < STDIO_IDLE_TTL {
                continue;
            }
            let removed = {
                let mut sessions = self.stdio_sessions.lock().await;
                if sessions
                    .get(&server_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &entry))
                {
                    sessions.remove(&server_id);
                    true
                } else {
                    false
                }
            };
            if removed {
                entry.session.lock().await.stop().await;
            }
        }
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use super::{
        auth::{contains_any, extract_authorize_url},
        http::{parse_http_mcp_value, resolve_sse_message_endpoint},
        protocol::{format_mcp_stderr, is_mcp_remote},
        stdio::{normalize_stdio_args, normalize_stdio_launch, parse_content_length},
    };

    use crate::domain::McpServerDraft;
    use std::sync::Arc;
    use tokio::fs;

    #[test]
    fn normalizes_legacy_compound_stdio_arguments() {
        let args = vec![
            "--from d:/Code_Save/Py/发邮件的mcp wecom-mail-mcp".to_string(),
            "--verbose".to_string(),
            "value with spaces".to_string(),
        ];

        assert_eq!(
            normalize_stdio_args(&args),
            vec![
                "--from",
                "d:/Code_Save/Py/发邮件的mcp wecom-mail-mcp",
                "--verbose",
                "value with spaces",
            ]
        );
    }

    #[test]
    fn normalizes_compound_stdio_commands_and_quoted_program_paths() {
        let (program, args) = normalize_stdio_launch(
            "uv tool run arxiv-mcp-server",
            &["--storage-path D:\\data\\arxiv".to_string()],
        );
        assert_eq!(program, "uv");
        assert_eq!(
            args,
            vec![
                "tool",
                "run",
                "arxiv-mcp-server",
                "--storage-path",
                "D:\\data\\arxiv",
            ]
        );

        let (program, args) =
            normalize_stdio_launch(r#""C:\Program Files\TARGET\server.exe" --mode stdio"#, &[]);
        assert_eq!(program, r#"C:\Program Files\TARGET\server.exe"#);
        assert_eq!(args, vec!["--mode", "stdio"]);
    }

    #[test]
    fn detects_authorization_url_and_waiting_state() {
        let line = "Please authorize this client: https://example.test/authorize?client_id=1";
        let lower = line.to_ascii_lowercase();
        assert_eq!(
            extract_authorize_url(line, &lower).as_deref(),
            Some("https://example.test/authorize?client_id=1")
        );
        assert!(contains_any(
            "waiting for authorization",
            &["waiting for authorization"]
        ));
    }

    #[test]
    fn recognizes_mcp_remote_commands() {
        let server = McpServer {
            id: Uuid::new_v4(),
            name: "Remote".to_string(),
            description: String::new(),
            enabled: true,
            supports_parallel_tool_calls: false,
            provider: None,
            provider_url: None,
            logo_url: None,
            tags: Vec::new(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "mcp-remote@latest".to_string()],
                cwd: None,
                env: Default::default(),
                inherit_env: false,
                env_allowlist: Vec::new(),
                long_running: true,
                timeout_ms: 60_000,
            },
            tools: Vec::new(),
            status: McpServerStatus::Unknown,
            last_synced_at: None,
            last_error: None,
            diagnostics: Default::default(),
        };
        assert!(is_mcp_remote(&server));
    }

    #[test]
    fn parses_content_length_frames() {
        assert_eq!(
            parse_content_length("Content-Length: 42").unwrap(),
            Some(42)
        );
        assert_eq!(parse_content_length("content-length: 7").unwrap(), Some(7));
        assert_eq!(parse_content_length("startup notice").unwrap(), None);
    }

    #[test]
    fn retries_only_transport_failures() {
        assert!(is_retryable_stdio_error(&AppError::Process(
            "MCP process closed before response".to_string()
        )));
        assert!(!is_retryable_stdio_error(&AppError::BadRequest(
            "tool failed".to_string()
        )));
    }

    #[test]
    fn preserves_mcp_stderr_details_for_connection_errors() {
        let lines = vec![
            "MCP Server starting with 35 tools".to_string(),
            "Fatal error in main(): ReplicateError: The REPLICATE_API_TOKEN environment variable is missing or empty".to_string(),
        ];
        let output = format_mcp_stderr(&lines).unwrap();
        assert!(output.contains("ReplicateError"));
        assert!(output.contains("REPLICATE_API_TOKEN"));
    }

    #[test]
    fn sse_ready_events_post_back_to_the_stream_url() {
        let base = reqwest::Url::parse("http://127.0.0.1:8765/sse/__builtin_skills__").unwrap();
        let endpoint =
            resolve_sse_message_endpoint(&base, "ready", r#"{"message":"connected","ok":true}"#)
                .unwrap();
        assert_eq!(endpoint, base);
    }

    #[test]
    fn legacy_sse_endpoint_events_still_resolve_relative_urls() {
        let base = reqwest::Url::parse("https://example.test/sse/server").unwrap();
        let endpoint =
            resolve_sse_message_endpoint(&base, "endpoint", "/message?session=1").unwrap();
        assert_eq!(endpoint.as_str(), "https://example.test/message?session=1");
    }

    #[test]
    fn streamable_http_accepts_json_even_when_content_type_is_sse() {
        let value = parse_http_mcp_value(
            r#"{"id":1,"jsonrpc":"2.0","result":{"protocolVersion":"2024-11-05"}}"#,
            "text/event-stream",
            Some(1),
        )
        .unwrap();
        assert_eq!(value["id"], 1);
    }

    #[tokio::test]
    async fn deleting_server_removes_managed_auth_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(temp_dir.path()).await.unwrap());
        let server = store
            .create_mcp_server(McpServerDraft {
                name: "cache cleanup test".to_string(),
                transport: McpTransport::Stdio {
                    command: "fake-mcp".to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                    inherit_env: false,
                    env_allowlist: Vec::new(),
                    long_running: false,
                    timeout_ms: 1_000,
                },
                ..Default::default()
            })
            .await
            .unwrap();
        let environment = Arc::new(EnvironmentRuntimeManager::new(temp_dir.path()).unwrap());
        let manager = McpManager::new(store.clone(), environment);
        let auth_dir = manager.auth.session_dir(server.id);
        fs::create_dir_all(&auth_dir).await.unwrap();
        fs::write(auth_dir.join("state.json"), "{}").await.unwrap();

        manager.delete_server(server.id).await.unwrap();

        assert!(!auth_dir.exists());
        assert!(store.list_mcp_servers().await.unwrap().is_empty());
    }
}
