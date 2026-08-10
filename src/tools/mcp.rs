use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Instant,
};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, RwLock},
    task::JoinHandle,
    time::{Duration, timeout},
};
use uuid::Uuid;

use crate::{
    domain::{
        McpAuthState, McpAuthStatus, McpServer, McpServerStatus, McpTool, McpTransport, ToolResult,
        qualify_tool_name, qualify_tool_name_with_suffix,
    },
    error::{AppError, AppResult},
    llm::types::{FunctionDefinition, ToolDefinition},
    storage::Store,
};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_TOOL_NAME_BYTES: usize = 64;

#[derive(Clone)]
struct McpAuthRuntime {
    root: PathBuf,
    states: Arc<Mutex<HashMap<Uuid, McpAuthState>>>,
    browser_opened_at: Arc<Mutex<HashMap<Uuid, Instant>>>,
}

impl McpAuthRuntime {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            states: Arc::new(Mutex::new(HashMap::new())),
            browser_opened_at: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn session_dir(&self, server_id: Uuid) -> PathBuf {
        self.root.join(server_id.to_string())
    }

    fn default_state(&self, server_id: Uuid) -> McpAuthState {
        McpAuthState {
            status: McpAuthStatus::Idle,
            authorize_url: None,
            last_success_at: None,
            last_updated_at: None,
            last_error: None,
            browser_opened: false,
            session_dir: Some(self.session_dir(server_id).display().to_string()),
        }
    }

    async fn state(&self, server_id: Uuid, supported: bool) -> McpAuthState {
        if !supported {
            return McpAuthState::unsupported();
        }
        if let Some(state) = self.states.lock().await.get(&server_id).cloned() {
            return state;
        }
        let path = self.session_dir(server_id).join("state.json");
        let state = match fs::read_to_string(path).await {
            Ok(content) => {
                serde_json::from_str(&content).unwrap_or_else(|_| self.default_state(server_id))
            }
            Err(_) => self.default_state(server_id),
        };
        self.states.lock().await.insert(server_id, state.clone());
        state
    }

    async fn save(&self, server_id: Uuid, state: &McpAuthState) {
        let dir = self.session_dir(server_id);
        if fs::create_dir_all(&dir).await.is_err() {
            return;
        }
        if let Ok(content) = serde_json::to_string_pretty(state) {
            let _ = fs::write(dir.join("state.json"), content).await;
        }
    }

    async fn update<F>(&self, server_id: Uuid, update: F) -> McpAuthState
    where
        F: FnOnce(&mut McpAuthState),
    {
        let mut states = self.states.lock().await;
        let state = states
            .entry(server_id)
            .or_insert_with(|| self.default_state(server_id));
        update(state);
        state.last_updated_at = Some(chrono::Utc::now().to_rfc3339());
        let snapshot = state.clone();
        drop(states);
        self.save(server_id, &snapshot).await;
        snapshot
    }

    async fn mark_starting(&self, server_id: Uuid) {
        self.update(server_id, |state| {
            state.status = McpAuthStatus::Starting;
            state.authorize_url = None;
            state.browser_opened = false;
            state.last_error = None;
        })
        .await;
    }

    async fn mark_connected(&self, server_id: Uuid) {
        self.update(server_id, |state| {
            state.status = McpAuthStatus::Connected;
            state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
            state.last_error = None;
        })
        .await;
    }

    async fn mark_error(&self, server_id: Uuid, error: String) {
        self.update(server_id, |state| {
            state.status = if matches!(
                state.status,
                McpAuthStatus::AuthPending
                    | McpAuthStatus::BrowserOpened
                    | McpAuthStatus::WaitingCallback
                    | McpAuthStatus::Authorized
            ) {
                McpAuthStatus::AuthFailed
            } else {
                McpAuthStatus::InitFailed
            };
            state.last_error = Some(error);
        })
        .await;
    }

    async fn mark_timeout(&self, server_id: Uuid, error: String) {
        self.update(server_id, |state| {
            state.status = if matches!(
                state.status,
                McpAuthStatus::AuthPending
                    | McpAuthStatus::BrowserOpened
                    | McpAuthStatus::WaitingCallback
                    | McpAuthStatus::Authorized
            ) {
                McpAuthStatus::AuthTimeout
            } else {
                McpAuthStatus::InitFailed
            };
            state.last_error = Some(error);
        })
        .await;
    }

    async fn handle_stderr_line(&self, server_id: Uuid, line: &str) {
        let lower = line.to_ascii_lowercase();
        let authorize_url = extract_authorize_url(line, &lower);
        let authorization_completed = contains_any(
            &lower,
            &[
                "authentication completed",
                "using tokens from disk",
                "token saved",
                "auth completed",
                "auth code received",
                "logged in successfully",
                "proxy established successfully",
            ],
        );
        let waiting_for_authorization = contains_any(
            &lower,
            &[
                "waiting for authorization",
                "waiting for auth code",
                "waiting for authentication",
                "oauth callback",
                "callback server",
            ],
        );
        let error_detected = contains_any(
            &lower,
            &[
                "authentication error",
                "token exchange failed",
                "authorization denied",
                "auth failed",
            ],
        );
        if authorize_url.is_none()
            && !authorization_completed
            && !waiting_for_authorization
            && !error_detected
        {
            return;
        }

        let mut should_open = false;
        self.update(server_id, |state| {
            if let Some(url) = authorize_url.clone() {
                state.authorize_url = Some(url);
            }
            if authorization_completed {
                state.status = McpAuthStatus::Authorized;
                state.last_error = None;
            } else if error_detected {
                state.status = McpAuthStatus::AuthFailed;
                state.last_error = Some(line.to_string());
            } else if waiting_for_authorization || state.authorize_url.is_some() {
                state.status = if state.browser_opened {
                    McpAuthStatus::WaitingCallback
                } else {
                    McpAuthStatus::AuthPending
                };
                should_open = state.authorize_url.is_some() && !state.browser_opened;
            }
        })
        .await;

        if should_open {
            let should_attempt = {
                let mut opened = self.browser_opened_at.lock().await;
                let recent = opened
                    .get(&server_id)
                    .is_some_and(|instant| instant.elapsed() < Duration::from_secs(8));
                if recent {
                    false
                } else {
                    opened.insert(server_id, Instant::now());
                    true
                }
            };
            if should_attempt {
                let url = self
                    .state(server_id, true)
                    .await
                    .authorize_url
                    .unwrap_or_default();
                match open_browser_url(&url) {
                    Ok(()) => {
                        self.update(server_id, |state| {
                            state.browser_opened = true;
                            state.status = McpAuthStatus::BrowserOpened;
                        })
                        .await;
                    }
                    Err(error) => {
                        self.update(server_id, |state| {
                            state.last_error = Some(error);
                            state.status = McpAuthStatus::AuthFailed;
                        })
                        .await;
                    }
                }
            }
        }
    }

    async fn clear(&self, server_id: Uuid) -> McpAuthState {
        self.states.lock().await.remove(&server_id);
        self.browser_opened_at.lock().await.remove(&server_id);
        let _ = fs::remove_dir_all(self.session_dir(server_id)).await;
        self.default_state(server_id)
    }

    async fn cleanup_orphans(&self, active_server_ids: &HashSet<Uuid>) {
        let Ok(mut entries) = fs::read_dir(&self.root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(server_id) = Uuid::parse_str(&name) else {
                continue;
            };
            if !active_server_ids.contains(&server_id) {
                let _ = fs::remove_dir_all(entry.path()).await;
            }
        }
    }

    fn managed_env(&self, server: &McpServer) -> std::collections::BTreeMap<String, String> {
        let McpTransport::Stdio { env, .. } = &server.transport else {
            return std::collections::BTreeMap::new();
        };
        let mut result = env.clone();
        if is_mcp_remote(server)
            && !result
                .keys()
                .any(|key| key.eq_ignore_ascii_case("MCP_REMOTE_CONFIG_DIR"))
        {
            result.insert(
                "MCP_REMOTE_CONFIG_DIR".to_string(),
                self.session_dir(server.id)
                    .join("mcp-remote")
                    .display()
                    .to_string(),
            );
        }
        result
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn extract_authorize_url(line: &str, lower: &str) -> Option<String> {
    if !contains_any(
        lower,
        &[
            "oauth",
            "authorize",
            "authorization",
            "login",
            "sign in",
            "auth code",
        ],
    ) {
        return None;
    }
    line.split_whitespace().find_map(|token| {
        let value =
            token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '(' | ')' | '[' | ']' | ','));
        (value.starts_with("http://") || value.starts_with("https://")).then(|| value.to_string())
    })
}

fn is_mcp_remote(server: &McpServer) -> bool {
    let McpTransport::Stdio { command, args, .. } = &server.transport else {
        return false;
    };
    let command_name = PathBuf::from(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    command_name == "mcp-remote"
        || args.iter().any(|arg| {
            arg.to_ascii_lowercase()
                .split('@')
                .next()
                .unwrap_or_default()
                == "mcp-remote"
        })
}

fn open_browser_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("MCP authorization URL is not an HTTP URL".to_string());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open browser: {error}"))
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open browser: {error}"))
    }
    #[cfg(target_os = "linux")]
    {
        match std::process::Command::new("xdg-open").arg(url).spawn() {
            Ok(_) => Ok(()),
            Err(xdg_error) => std::process::Command::new("gio")
                .args(["open", url])
                .spawn()
                .map(|_| ())
                .map_err(|gio_error| {
                    format!(
                        "failed to open browser with xdg-open ({xdg_error}) or gio ({gio_error})"
                    )
                }),
        }
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
    {
        Err("automatic browser opening is unsupported on this operating system".to_string())
    }
}

#[derive(Clone)]
pub struct McpManager {
    store: Arc<Store>,
    snapshots: Arc<RwLock<HashMap<Uuid, Vec<McpServer>>>>,
    stdio_sessions: Arc<Mutex<HashMap<Uuid, Arc<Mutex<StdioSession>>>>>,
    auth: Arc<McpAuthRuntime>,
}

impl McpManager {
    pub fn new(store: Arc<Store>) -> Self {
        let auth_root = store.data_dir.join("mcp-auth");
        Self {
            store,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            stdio_sessions: Arc::new(Mutex::new(HashMap::new())),
            auth: Arc::new(McpAuthRuntime::new(auth_root)),
        }
    }

    pub async fn stop_server(&self, server_id: Uuid) {
        let session = self.stdio_sessions.lock().await.remove(&server_id);
        if let Some(session) = session {
            session.lock().await.stop().await;
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
                matches!(server.transport, McpTransport::Stdio { .. }),
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
        for server in settings.effective.iter().filter(|server| server.enabled) {
            if server.tools.is_empty()
                || matches!(
                    server.status,
                    McpServerStatus::Unknown | McpServerStatus::Offline | McpServerStatus::Error
                )
            {
                let _ = self.refresh_server(server.id).await;
            }
        }
        let updated = self.store.read_mcp_agent_settings(agent_id).await?;
        self.snapshots
            .write()
            .await
            .insert(agent_id, updated.effective);
        Ok(())
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
        match discover_tools(self, &server).await {
            Ok(tools) => {
                self.store.replace_mcp_tools(server_id, &tools).await?;
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
                self.store
                    .update_mcp_server_status(server_id, status, Some(error.to_string()), None)
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

    pub async fn definitions(&self, agent_id: Uuid) -> Option<Vec<ToolDefinition>> {
        let snapshots = self.snapshots.read().await;
        let servers = snapshots.get(&agent_id)?;
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

    pub async fn execute_tool(&self, agent_id: Uuid, name: &str, arguments: Value) -> ToolResult {
        let snapshots = self.snapshots.read().await;
        let Some(server) = snapshots
            .get(&agent_id)
            .and_then(|servers| {
                servers.iter().find(|server| {
                    server
                        .tools
                        .iter()
                        .any(|tool| tool.qualified_name == name && tool.enabled)
                })
            })
            .cloned()
        else {
            return ToolResult::Blocked {
                reason: format!("MCP tool '{name}' is disabled or unavailable"),
                code: Some("McpToolUnavailable".to_string()),
                details: None,
            };
        };
        let Some(tool) = server.tools.iter().find(|tool| tool.qualified_name == name) else {
            return ToolResult::Error {
                message: format!("Unknown MCP tool: {name}"),
            };
        };
        let started = Instant::now();
        match call_tool(self, &server, tool, arguments).await {
            Ok(output) => ToolResult::Completed {
                stdout: output,
                stderr: String::new(),
                exit_code: 0,
                duration_ms: started.elapsed().as_millis() as u64,
                runtime: Some("mcp".to_string()),
                output_cursor: 0,
            },
            Err(error) => ToolResult::Error {
                message: error.to_string(),
            },
        }
    }
}

pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp__")
}

async fn discover_tools(manager: &McpManager, server: &McpServer) -> AppResult<Vec<McpTool>> {
    let result = manager
        .rpc_session(server, Some(("tools/list", json!({}))))
        .await?;
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut used_names = HashSet::new();
    Ok(tools
        .into_iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let mut occurrence = 1;
            let qualified_name = loop {
                let candidate = if occurrence == 1 {
                    qualify_tool_name(server.id, &name)
                } else {
                    qualify_tool_name_with_suffix(server.id, &name, occurrence)
                };
                if used_names.insert(candidate.clone()) {
                    break candidate;
                }
                occurrence += 1;
            };
            Some(McpTool {
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
                input_schema: item
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"})),
                enabled: true,
                updated_at: Some(chrono::Utc::now().to_rfc3339()),
            })
        })
        .collect())
}

async fn call_tool(
    manager: &McpManager,
    server: &McpServer,
    tool: &McpTool,
    arguments: Value,
) -> AppResult<String> {
    let result = manager
        .rpc_session(
            server,
            Some((
                "tools/call",
                json!({"name": tool.name, "arguments": arguments}),
            )),
        )
        .await?;
    Ok(result
        .get("content")
        .map(|value| serde_json::to_string_pretty(value))
        .transpose()?
        .unwrap_or_else(|| serde_json::to_string_pretty(&result).unwrap_or_default()))
}

impl McpManager {
    async fn rpc_session(
        &self,
        server: &McpServer,
        request: Option<(&str, Value)>,
    ) -> AppResult<Value> {
        match &server.transport {
            McpTransport::Stdio {
                long_running: true, ..
            } => self.rpc_stdio_long_running(server, request).await,
            McpTransport::Stdio { .. } => rpc_stdio_once(&self.auth, server, request).await,
            McpTransport::StreamableHttp { .. } => rpc_streamable_http(server, request).await,
            McpTransport::Sse { .. } => rpc_sse(server, request).await,
        }
    }

    async fn rpc_stdio_long_running(
        &self,
        server: &McpServer,
        request: Option<(&str, Value)>,
    ) -> AppResult<Value> {
        let timeout_ms = mcp_timeout_ms(&server.transport);
        let timeout_duration = Duration::from_millis(timeout_ms.clamp(1_000, 120_000));
        let session = self.get_stdio_session(server, timeout_duration).await?;
        let result = if let Some((method, params)) = request {
            let mut session_guard = session.lock().await;
            session_guard
                .request(method, params, timeout_duration)
                .await
        } else {
            Ok(json!({}))
        };
        if result.is_err() {
            self.invalidate_stdio_session(server.id, &session).await;
        }
        result
    }

    async fn get_stdio_session(
        &self,
        server: &McpServer,
        timeout_duration: Duration,
    ) -> AppResult<Arc<Mutex<StdioSession>>> {
        let mut sessions = self.stdio_sessions.lock().await;
        if let Some(session) = sessions.get(&server.id) {
            return Ok(session.clone());
        }
        let session = Arc::new(Mutex::new(
            StdioSession::start(server, timeout_duration, self.auth.clone()).await?,
        ));
        sessions.insert(server.id, session.clone());
        Ok(session)
    }

    async fn invalidate_stdio_session(&self, server_id: Uuid, session: &Arc<Mutex<StdioSession>>) {
        let removed = {
            let mut sessions = self.stdio_sessions.lock().await;
            if sessions
                .get(&server_id)
                .is_some_and(|current| Arc::ptr_eq(current, session))
            {
                sessions.remove(&server_id);
                true
            } else {
                false
            }
        };
        if removed {
            session.lock().await.stop().await;
        }
    }
}

async fn rpc_stdio_once(
    auth: &Arc<McpAuthRuntime>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<Value> {
    let timeout_duration =
        Duration::from_millis(mcp_timeout_ms(&server.transport).clamp(1_000, 120_000));
    let mut session = StdioSession::start(server, timeout_duration, auth.clone()).await?;
    let result = if let Some((method, params)) = request {
        session.request(method, params, timeout_duration).await
    } else {
        Ok(json!({}))
    };
    session.stop().await;
    result
}

struct StdioSession {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    reader: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    next_id: i64,
    stderr_task: Option<JoinHandle<()>>,
    stderr_output: Arc<Mutex<Vec<String>>>,
    auth: Arc<McpAuthRuntime>,
    server_id: Uuid,
}

impl StdioSession {
    async fn start(
        server: &McpServer,
        timeout_duration: Duration,
        auth: Arc<McpAuthRuntime>,
    ) -> AppResult<Self> {
        auth.mark_starting(server.id).await;
        let mut session = match Self::spawn(server, auth.clone()) {
            Ok(session) => session,
            Err(error) => {
                auth.mark_error(server.id, error.to_string()).await;
                return Err(error);
            }
        };
        match timeout(timeout_duration, session.initialize()).await {
            Ok(Ok(())) => {
                auth.mark_connected(server.id).await;
                Ok(session)
            }
            Ok(Err(error)) => {
                auth.mark_error(server.id, error.to_string()).await;
                session.stop().await;
                Err(error)
            }
            Err(_) => {
                auth.mark_timeout(server.id, mcp_timeout_error(timeout_duration).to_string())
                    .await;
                session.stop().await;
                Err(mcp_timeout_error(timeout_duration))
            }
        }
    }

    fn spawn(server: &McpServer, auth: Arc<McpAuthRuntime>) -> AppResult<Self> {
        let McpTransport::Stdio {
            command,
            args,
            cwd,
            env: _,
            ..
        } = &server.transport
        else {
            unreachable!()
        };
        if command.trim().is_empty() {
            return Err(AppError::BadRequest(
                "MCP stdio command is required".to_string(),
            ));
        }
        let mut process = Command::new(command);
        process
            .args(normalize_stdio_args(args))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd.as_deref().filter(|value| !value.trim().is_empty()) {
            process.current_dir(cwd);
        }
        process.envs(auth.managed_env(server));
        let mut child = process
            .spawn()
            .map_err(|error| AppError::Process(format!("MCP process failed to start: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Process("MCP stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Process("MCP stdout unavailable".to_string()))?;
        let stderr_output = Arc::new(Mutex::new(Vec::new()));
        let stderr_task = child.stderr.take().map(|stderr| {
            let auth = auth.clone();
            let server_id = server.id;
            let stderr_output = stderr_output.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = line.trim();
                    if !line.is_empty() {
                        let mut output = stderr_output.lock().await;
                        output.push(line.to_string());
                        if output.len() > 20 {
                            output.remove(0);
                        }
                        drop(output);
                        auth.handle_stderr_line(server_id, line).await;
                    }
                }
            })
        });
        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout).lines(),
            next_id: 2,
            stderr_task,
            stderr_output,
            auth,
            server_id: server.id,
        })
    }

    async fn read_response(&mut self, expected_id: i64) -> AppResult<Value> {
        match read_response_lines(&mut self.reader, expected_id).await {
            Ok(value) => Ok(value),
            Err(AppError::Process(message)) if message == "MCP process closed before response" => {
                let exit_detail = self.child.try_wait().ok().flatten().map(|status| {
                    status
                        .code()
                        .map(|code| format!("exit code {code}"))
                        .unwrap_or_else(|| "terminated by signal".to_string())
                });
                let stderr_detail = self.stderr_output.lock().await.last().cloned();
                let details = [
                    exit_detail,
                    stderr_detail.map(|line| format!("stderr: {line}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                if details.is_empty() {
                    Err(AppError::Process(message))
                } else {
                    Err(AppError::Process(format!(
                        "{message} ({})",
                        details.join("; ")
                    )))
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn initialize(&mut self) -> AppResult<()> {
        send_line(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"WeiAgent","version":"0.1.0"}}}),
        )
        .await?;
        let _ = self.read_response(1).await?;
        send_line(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )
        .await
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_duration: Duration,
    ) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let result = timeout(timeout_duration, async {
            send_line(
                &mut self.stdin,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await?;
            self.read_response(id).await
        })
        .await;
        match result {
            Ok(Ok(value)) => {
                self.auth.mark_connected(self.server_id).await;
                Ok(value)
            }
            Ok(Err(error)) => {
                self.auth
                    .mark_error(self.server_id, error.to_string())
                    .await;
                Err(error)
            }
            Err(_) => {
                let error = mcp_timeout_error(timeout_duration);
                self.auth
                    .mark_timeout(self.server_id, error.to_string())
                    .await;
                Err(error)
            }
        }
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn normalize_stdio_args(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| {
            let trimmed = arg.trim();
            let Some(separator) = trimmed.find(char::is_whitespace) else {
                return vec![arg.clone()];
            };
            let option = &trimmed[..separator];
            let value = trimmed[separator..].trim();
            if !option.starts_with('-') || value.is_empty() {
                return vec![arg.clone()];
            }
            vec![option.to_string(), value.to_string()]
        })
        .collect()
}

fn mcp_timeout_ms(transport: &McpTransport) -> u64 {
    match transport {
        McpTransport::Stdio { timeout_ms, .. }
        | McpTransport::StreamableHttp { timeout_ms, .. }
        | McpTransport::Sse { timeout_ms, .. } => *timeout_ms,
    }
}

fn mcp_timeout_error(timeout_duration: Duration) -> AppError {
    AppError::Process(format!(
        "MCP stdio request timed out after {}ms",
        timeout_duration.as_millis()
    ))
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    use crate::domain::McpServerDraft;
    use std::sync::Arc;

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
            provider: None,
            provider_url: None,
            logo_url: None,
            tags: Vec::new(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "mcp-remote@latest".to_string()],
                cwd: None,
                env: Default::default(),
                long_running: true,
                timeout_ms: 60_000,
            },
            tools: Vec::new(),
            status: McpServerStatus::Unknown,
            last_synced_at: None,
            last_error: None,
        };
        assert!(is_mcp_remote(&server));
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
                    long_running: false,
                    timeout_ms: 1_000,
                },
                ..Default::default()
            })
            .await
            .unwrap();
        let manager = McpManager::new(store.clone());
        let auth_dir = manager.auth.session_dir(server.id);
        fs::create_dir_all(&auth_dir).await.unwrap();
        fs::write(auth_dir.join("state.json"), "{}").await.unwrap();

        manager.delete_server(server.id).await.unwrap();

        assert!(!auth_dir.exists());
        assert!(store.list_mcp_servers().await.unwrap().is_empty());
    }
}

async fn rpc_streamable_http(
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<Value> {
    let (url, bearer_token, headers, timeout_ms) = match &server.transport {
        McpTransport::StreamableHttp {
            url,
            bearer_token,
            headers,
            timeout_ms,
        } => (url, bearer_token.as_deref(), headers, *timeout_ms),
        _ => unreachable!(),
    };
    if url.trim().is_empty() {
        return Err(AppError::BadRequest("MCP HTTP URL is required".to_string()));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms.clamp(1_000, 120_000)))
        .build()
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let mut session: Option<String> = None;
    let init = http_request(&client, url, bearer_token, headers, session.as_deref(), json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"WeiAgent","version":"0.1.0"}}})).await?;
    session = init.1;
    let _ = http_request(
        &client,
        url,
        bearer_token,
        headers,
        session.as_deref(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    )
    .await?;
    if let Some((method, params)) = request {
        let (value, _) = http_request(
            &client,
            url,
            bearer_token,
            headers,
            session.as_deref(),
            json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),
        )
        .await?;
        Ok(value)
    } else {
        Ok(json!({}))
    }
}

async fn rpc_sse(server: &McpServer, request: Option<(&str, Value)>) -> AppResult<Value> {
    let McpTransport::Sse {
        url,
        bearer_token,
        headers,
        timeout_ms,
    } = &server.transport
    else {
        unreachable!()
    };
    if url.trim().is_empty() {
        return Err(AppError::BadRequest("MCP SSE URL is required".to_string()));
    }
    let timeout_duration = Duration::from_millis((*timeout_ms).clamp(1_000, 120_000));
    let client = reqwest::Client::builder()
        .timeout(timeout_duration)
        .build()
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let mut stream_response = client
        .get(url)
        .headers(build_headers(
            bearer_token.as_deref(),
            headers,
            Some("text/event-stream"),
        )?)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    if !stream_response.status().is_success() {
        return Err(AppError::Llm(format!(
            "MCP SSE endpoint returned {}",
            stream_response.status()
        )));
    }
    let endpoint_data = read_sse_event(&mut stream_response, timeout_duration).await?;
    let endpoint = endpoint_data.1.trim().to_string();
    let base_url =
        reqwest::Url::parse(url).map_err(|error| AppError::BadRequest(error.to_string()))?;
    let endpoint_url = base_url
        .join(&endpoint)
        .or_else(|_| reqwest::Url::parse(&endpoint))
        .map_err(|error| {
            AppError::BadRequest(format!("Invalid MCP SSE message endpoint: {error}"))
        })?;

    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"WeiAgent","version":"0.1.0"}}});
    let initialize_response = post_sse_message(
        &client,
        &endpoint_url,
        bearer_token.as_deref(),
        headers,
        initialize,
    )
    .await?;
    let initialize_result = if initialize_response.trim().is_empty() {
        read_sse_response(&mut stream_response, timeout_duration, 1).await?
    } else {
        parse_rpc_response(&initialize_response)?
    };
    let _ = initialize_result;

    let _ = post_sse_message(
        &client,
        &endpoint_url,
        bearer_token.as_deref(),
        headers,
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    )
    .await?;
    if let Some((method, params)) = request {
        let response = post_sse_message(
            &client,
            &endpoint_url,
            bearer_token.as_deref(),
            headers,
            json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),
        )
        .await?;
        if response.trim().is_empty() {
            read_sse_response(&mut stream_response, timeout_duration, 2).await
        } else {
            parse_rpc_response(&response)
        }
    } else {
        Ok(json!({}))
    }
}

async fn http_request(
    client: &reqwest::Client,
    url: &str,
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    session: Option<&str>,
    body: Value,
) -> AppResult<(Value, Option<String>)> {
    let mut header_map = build_headers(
        bearer_token,
        headers,
        Some("application/json, text/event-stream"),
    )?;
    if let Some(session) = session {
        if let Ok(value) = HeaderValue::from_str(session) {
            header_map.insert("mcp-session-id", value);
        }
    }
    let response = client
        .post(url)
        .headers(header_map)
        .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = response
        .text()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    if text.trim().is_empty() {
        return Ok((json!({}), session));
    }
    let value = serde_json::from_str::<Value>(&text)
        .or_else(|_| parse_sse_json(&text))
        .map_err(|error| AppError::Json(error))?;
    if let Some(error) = value.get("error") {
        return Err(AppError::BadRequest(error.to_string()));
    }
    Ok((value.get("result").cloned().unwrap_or(value), session))
}

fn build_headers(
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    accept: Option<&str>,
) -> AppResult<HeaderMap> {
    let mut header_map = HeaderMap::new();
    for (key, value) in headers {
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP header name: {error}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP header value: {error}")))?;
        header_map.insert(name, value);
    }
    if let Some(token) = bearer_token.filter(|token| !token.is_empty() && *token != "__preserve__")
    {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP bearer token: {error}")))?;
        header_map.insert("authorization", value);
    }
    if let Some(accept) = accept {
        let value = HeaderValue::from_str(accept)
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP accept header: {error}")))?;
        header_map.insert("accept", value);
    }
    Ok(header_map)
}

async fn post_sse_message(
    client: &reqwest::Client,
    endpoint: &reqwest::Url,
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    body: Value,
) -> AppResult<String> {
    let response = client
        .post(endpoint.clone())
        .headers(build_headers(
            bearer_token,
            headers,
            Some("application/json, text/event-stream"),
        )?)
        .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    if !status.is_success() {
        return Err(AppError::Llm(format!(
            "MCP SSE message endpoint returned {status}: {body}"
        )));
    }
    Ok(body)
}

async fn read_sse_event(
    response: &mut reqwest::Response,
    timeout_duration: Duration,
) -> AppResult<(String, String)> {
    let mut buffer = Vec::new();
    loop {
        if let Some(index) = find_sse_boundary(&buffer) {
            let event = buffer.drain(..index).collect::<Vec<_>>();
            buffer.drain(..boundary_length(&buffer));
            if let Some(parsed) = parse_sse_event(&String::from_utf8_lossy(&event)) {
                return Ok(parsed);
            }
            continue;
        }
        let chunk = timeout(timeout_duration, response.chunk())
            .await
            .map_err(|_| AppError::Llm("MCP SSE response timed out".to_string()))?
            .map_err(|error| AppError::Llm(error.to_string()))?;
        let Some(chunk) = chunk else {
            return Err(AppError::Llm(
                "MCP SSE stream closed before a response".to_string(),
            ));
        };
        buffer.extend_from_slice(&chunk);
    }
}

async fn read_sse_response(
    response: &mut reqwest::Response,
    timeout_duration: Duration,
    expected_id: i64,
) -> AppResult<Value> {
    loop {
        let (_, data) = read_sse_event(response, timeout_duration).await?;
        let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        return parse_rpc_value(value);
    }
}

fn parse_rpc_response(text: &str) -> AppResult<Value> {
    parse_rpc_value(serde_json::from_str(text)?)
}

fn parse_rpc_value(value: Value) -> AppResult<Value> {
    if let Some(error) = value.get("error") {
        return Err(AppError::BadRequest(error.to_string()));
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}

fn find_sse_boundary(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .or_else(|| buffer.windows(4).position(|window| window == b"\r\n\r\n"))
}

fn boundary_length(buffer: &[u8]) -> usize {
    if buffer.starts_with(b"\r\n\r\n") {
        4
    } else {
        2
    }
}

fn parse_sse_event(text: &str) -> Option<(String, String)> {
    let mut event = String::new();
    let mut data = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_string());
        }
    }
    (!data.is_empty()).then(|| (event, data.join("\n")))
}

fn parse_sse_json(text: &str) -> Result<Value, serde_json::Error> {
    for line in text.lines().filter_map(|line| line.strip_prefix("data:")) {
        if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
            return Ok(value);
        }
    }
    serde_json::from_str(text)
}

async fn send_line(stdin: &mut tokio::process::ChildStdin, value: Value) -> AppResult<()> {
    stdin
        .write_all(serde_json::to_string(&value)?.as_bytes())
        .await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_response_lines(
    reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    expected_id: i64,
) -> AppResult<Value> {
    while let Some(line) = reader.next_line().await? {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            // Some launchers print startup notices to stdout. They are not
            // JSON-RPC messages, so keep scanning for the response instead
            // of turning a valid MCP session into a JSON parse failure.
            continue;
        };
        if value.get("id").and_then(Value::as_i64) == Some(expected_id) {
            if let Some(error) = value.get("error") {
                return Err(AppError::BadRequest(error.to_string()));
            }
            return Ok(value.get("result").cloned().unwrap_or(value));
        }
    }
    Err(AppError::Process(
        "MCP process closed before response".to_string(),
    ))
}
