use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
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
    tools::{runtime::EnvironmentRuntimeManager, terminal::managed_path_from},
};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_TOOL_NAME_BYTES: usize = 64;
const STDIO_IDLE_TTL: Duration = Duration::from_secs(300);
const STDIO_MAX_RETRIES: u32 = 2;
const STDIO_MAX_RESPONSE_WAIT_ITERATIONS: usize = 100;
const MAX_STDIO_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOLS_LIST_PAGES: usize = 100;
const MAX_MCP_STDERR_ERROR_CHARS: usize = 4_000;

fn mcp_initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "WeiAgent",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

#[derive(Clone)]
struct McpAuthRuntime {
    root: PathBuf,
    states: Arc<Mutex<HashMap<Uuid, McpAuthState>>>,
    browser_opened_at: Arc<Mutex<HashMap<Uuid, Instant>>>,
    oauth_tasks: Arc<Mutex<HashMap<Uuid, JoinHandle<()>>>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct McpOAuthToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
}

impl McpAuthRuntime {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            states: Arc::new(Mutex::new(HashMap::new())),
            browser_opened_at: Arc::new(Mutex::new(HashMap::new())),
            oauth_tasks: Arc::new(Mutex::new(HashMap::new())),
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
        if let Some(task) = self.oauth_tasks.lock().await.remove(&server_id) {
            task.abort();
        }
        self.states.lock().await.remove(&server_id);
        self.browser_opened_at.lock().await.remove(&server_id);
        let _ = fs::remove_dir_all(self.session_dir(server_id)).await;
        self.default_state(server_id)
    }

    async fn oauth_access_token(&self, server: &McpServer) -> Option<String> {
        let oauth = match &server.transport {
            McpTransport::StreamableHttp { oauth, .. } | McpTransport::Sse { oauth, .. } => {
                oauth.as_ref()
            }
            McpTransport::Stdio { .. } => None,
        }?;
        let path = self.session_dir(server.id).join("oauth-token.json");
        let content = fs::read_to_string(&path).await.ok()?;
        let mut token = serde_json::from_str::<McpOAuthToken>(&content).ok()?;
        let now = chrono::Utc::now().timestamp();
        if token
            .expires_at
            .is_some_and(|expires_at| expires_at <= now + 60)
        {
            let refresh_token = token.refresh_token.clone()?;
            let client = reqwest::Client::new();
            let mut form = vec![
                ("grant_type", "refresh_token".to_string()),
                ("refresh_token", refresh_token),
                ("client_id", oauth.client_id.clone()),
            ];
            if let Some(secret) = oauth.client_secret.as_deref() {
                form.push(("client_secret", secret.to_string()));
            }
            let response = client
                .post(&oauth.token_url)
                .form(&form)
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let value = response.json::<Value>().await.ok()?;
            token.access_token = value.get("access_token")?.as_str()?.to_string();
            token.refresh_token = value
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(token.refresh_token);
            token.expires_at = value
                .get("expires_in")
                .and_then(Value::as_i64)
                .map(|seconds| now + seconds);
            if let Ok(serialized) = serde_json::to_string_pretty(&token) {
                let _ = fs::write(path, serialized).await;
            }
        }
        Some(token.access_token)
    }

    async fn start_oauth(&self, server: &McpServer) -> AppResult<McpAuthState> {
        let oauth = match &server.transport {
            McpTransport::StreamableHttp { oauth, .. } | McpTransport::Sse { oauth, .. } => {
                oauth.clone()
            }
            McpTransport::Stdio { .. } => None,
        }
        .ok_or_else(|| AppError::BadRequest("MCP OAuth configuration is missing".to_string()))?;
        if oauth.authorize_url.trim().is_empty()
            || oauth.token_url.trim().is_empty()
            || oauth.client_id.trim().is_empty()
        {
            return Err(AppError::BadRequest(
                "MCP OAuth requires authorize_url, token_url and client_id".to_string(),
            ));
        }
        let port = oauth.redirect_port.unwrap_or(8765);
        let redirect_uri = oauth
            .redirect_uri
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}/callback"));
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|error| {
                AppError::Process(format!("MCP OAuth callback bind failed: {error}"))
            })?;
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = Uuid::new_v4().simple().to_string();
        let mut authorize_url = reqwest::Url::parse(&oauth.authorize_url).map_err(|error| {
            AppError::BadRequest(format!("invalid MCP OAuth authorize URL: {error}"))
        })?;
        {
            let mut query = authorize_url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &oauth.client_id);
            query.append_pair("redirect_uri", &redirect_uri);
            query.append_pair("state", &state);
            query.append_pair("code_challenge", &challenge);
            query.append_pair("code_challenge_method", "S256");
            if !oauth.scopes.is_empty() {
                query.append_pair("scope", &oauth.scopes.join(" "));
            }
        }
        self.mark_starting(server.id).await;
        self.update(server.id, |auth| {
            auth.status = McpAuthStatus::AuthPending;
            auth.authorize_url = Some(authorize_url.to_string());
            auth.last_error = None;
        })
        .await;
        open_browser_url(authorize_url.as_str()).map_err(|error| AppError::Process(error))?;
        let runtime = self.clone();
        let server_id = server.id;
        let task = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let _ = timeout(Duration::from_secs(300), async {
                let mut buffer = [0_u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n")
                        || request.len() >= 8 * 1024
                    {
                        break;
                    }
                }
                Ok::<(), std::io::Error>(())
            })
            .await;
            let request_line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let callback_url = reqwest::Url::parse(&format!("http://127.0.0.1{path}"));
            let mut response_body = "MCP authorization completed. You can return to WeiAgent.";
            let mut response_status = "200 OK";
            if let Ok(callback_url) = callback_url {
                let callback_state = callback_url
                    .query_pairs()
                    .find(|(key, _)| key == "state")
                    .map(|(_, value)| value.to_string());
                let code = callback_url
                    .query_pairs()
                    .find(|(key, _)| key == "code")
                    .map(|(_, value)| value.to_string());
                if callback_state.as_deref() != Some(state.as_str()) || code.is_none() {
                    response_body = "MCP authorization failed.";
                    response_status = "400 Bad Request";
                    runtime
                        .mark_error(
                            server_id,
                            "MCP OAuth callback state or code was invalid".to_string(),
                        )
                        .await;
                } else {
                    let mut form = vec![
                        ("grant_type", "authorization_code".to_string()),
                        ("code", code.unwrap_or_default()),
                        ("redirect_uri", redirect_uri.clone()),
                        ("client_id", oauth.client_id.clone()),
                        ("code_verifier", verifier.clone()),
                    ];
                    if let Some(secret) = oauth.client_secret.as_deref() {
                        form.push(("client_secret", secret.to_string()));
                    }
                    let token_result = match reqwest::Client::new()
                        .post(&oauth.token_url)
                        .form(&form)
                        .send()
                        .await
                    {
                        Ok(response) if response.status().is_success() => {
                            response.json::<Value>().await.ok()
                        }
                        _ => None,
                    };
                    match token_result {
                        Some(value) => {
                            if let Some(access_token) =
                                value.get("access_token").and_then(Value::as_str)
                            {
                                let token = McpOAuthToken {
                                    access_token: access_token.to_string(),
                                    refresh_token: value
                                        .get("refresh_token")
                                        .and_then(Value::as_str)
                                        .map(str::to_string),
                                    token_type: value
                                        .get("token_type")
                                        .and_then(Value::as_str)
                                        .map(str::to_string),
                                    expires_at: value
                                        .get("expires_in")
                                        .and_then(Value::as_i64)
                                        .map(|seconds| chrono::Utc::now().timestamp() + seconds),
                                };
                                let dir = runtime.session_dir(server_id);
                                if fs::create_dir_all(&dir).await.is_ok() {
                                    if let Ok(serialized) = serde_json::to_string_pretty(&token) {
                                        let _ = fs::write(dir.join("oauth-token.json"), serialized)
                                            .await;
                                    }
                                }
                                runtime.mark_connected(server_id).await;
                            } else {
                                response_body = "MCP authorization returned no access token.";
                                response_status = "502 Bad Gateway";
                                runtime
                                    .mark_error(
                                        server_id,
                                        "MCP OAuth token response had no access_token".to_string(),
                                    )
                                    .await;
                            }
                        }
                        None => {
                            response_body = "MCP authorization token exchange failed.";
                            response_status = "502 Bad Gateway";
                            runtime
                                .mark_error(
                                    server_id,
                                    "MCP OAuth token exchange failed".to_string(),
                                )
                                .await;
                        }
                    }
                }
            }
            let reply = format!(
                "HTTP/1.1 {response_status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            let _ = stream.write_all(reply.as_bytes()).await;
        });
        self.oauth_tasks.lock().await.insert(server.id, task);
        Ok(self.state(server.id, true).await)
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
        let McpTransport::Stdio {
            env,
            inherit_env,
            env_allowlist,
            ..
        } = &server.transport
        else {
            return std::collections::BTreeMap::new();
        };
        let mut result = safe_baseline_environment(*inherit_env, env_allowlist);
        result.extend(
            env.iter()
                .map(|(key, value)| (key.clone(), expand_mcp_value(value))),
        );
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

fn safe_baseline_environment(
    inherit_env: bool,
    allowlist: &[String],
) -> std::collections::BTreeMap<String, String> {
    let mut result = std::collections::BTreeMap::new();
    let baseline = [
        "HOME",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SYSTEMROOT",
        "COMSPEC",
        "WINDIR",
        "PATHEXT",
        "HOMEDRIVE",
        "HOMEPATH",
        "ALLUSERSPROFILE",
        "PROGRAMDATA",
        "APPDATA",
        "LOCALAPPDATA",
        // Package managers need the host's network settings even when a server
        // opts out of inheriting the complete process environment.
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "UV_CACHE_DIR",
        "UV_TOOL_DIR",
        "UV_TOOL_BIN_DIR",
        "UV_HTTP_TIMEOUT",
        "UV_PYTHON_INSTALL_MIRROR",
        "NPM_CONFIG_REGISTRY",
    ];
    let inherited = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    for key in baseline
        .iter()
        .copied()
        .chain(allowlist.iter().map(String::as_str))
    {
        if let Some((_, value)) = inherited
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
        {
            result.insert(key.to_string(), value.clone());
        }
    }
    if inherit_env {
        result.extend(inherited);
    }
    result
}

fn expand_mcp_value(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let key = &rest[start + 2..start + 2 + end];
        let replacement = match key {
            "userHome" => std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_default(),
            "pathSeparator" | "/" => std::path::MAIN_SEPARATOR.to_string(),
            _ => std::env::var(key).unwrap_or_else(|_| format!("${{{key}}}")),
        };
        output.push_str(&replacement);
        rest = &rest[start + 3 + end..];
    }
    output.push_str(rest);
    output
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
    environment: Arc<EnvironmentRuntimeManager>,
    snapshots: Arc<RwLock<HashMap<Uuid, Vec<McpServer>>>>,
    stdio_sessions: Arc<Mutex<HashMap<Uuid, Arc<StdioSessionEntry>>>>,
    auth: Arc<McpAuthRuntime>,
}

struct StdioSessionEntry {
    signature: String,
    session: Arc<Mutex<StdioSession>>,
    last_used: Mutex<Instant>,
}

#[derive(Debug, Clone, Default)]
struct McpHandshake {
    protocol_version: Option<String>,
    server_name: Option<String>,
    server_version: Option<String>,
    capabilities: Value,
}

#[derive(Debug)]
struct McpRpcResponse {
    value: Value,
    handshake: Option<McpHandshake>,
}

fn parse_mcp_handshake(result: &Value) -> McpHandshake {
    let server_info = result.get("serverInfo");
    McpHandshake {
        protocol_version: result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .map(str::to_string),
        server_name: server_info
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        server_version: server_info
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
        capabilities: result.get("capabilities").cloned().unwrap_or(Value::Null),
    }
}

impl StdioSessionEntry {
    fn new(signature: String, session: Arc<Mutex<StdioSession>>) -> Self {
        Self {
            signature,
            session,
            last_used: Mutex::new(Instant::now()),
        }
    }

    async fn touch(&self) {
        *self.last_used.lock().await = Instant::now();
    }
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

    pub async fn supports_parallel_tool(&self, agent_id: Uuid, name: &str) -> bool {
        let snapshots = self.snapshots.read().await;
        snapshots.get(&agent_id).is_some_and(|servers| {
            servers.iter().any(|server| {
                server.supports_parallel_tool_calls
                    && server.enabled
                    && server.tools.iter().any(|tool| {
                        tool.enabled && tool.qualified_name == name && tool.tool_id != Uuid::nil()
                    })
            })
        })
    }

    pub async fn execute_tool(&self, agent_id: Uuid, name: &str, arguments: Value) -> ToolResult {
        let snapshots = self.snapshots.read().await;
        let Some((server_id, tool_id)) = snapshots.get(&agent_id).and_then(|servers| {
            servers.iter().find_map(|server| {
                server.tools.iter().find_map(|tool| {
                    (tool.qualified_name == name && tool.enabled)
                        .then_some((server.id, tool.tool_id))
                })
            })
        }) else {
            return ToolResult::Blocked {
                reason: format!("MCP tool '{name}' is disabled or unavailable"),
                code: Some("McpToolUnavailable".to_string()),
                details: None,
            };
        };
        let Some(server) = snapshots
            .get(&agent_id)
            .and_then(|servers| servers.iter().find(|server| server.id == server_id))
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

fn status_name(status: McpServerStatus) -> &'static str {
    match status {
        McpServerStatus::Unknown => "unknown",
        McpServerStatus::Connecting => "connecting",
        McpServerStatus::Ready => "ready",
        McpServerStatus::Offline => "offline",
        McpServerStatus::Error => "error",
    }
}

pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp_") || name.starts_with("mcp__")
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

struct McpCallOutput {
    text: String,
    content: Value,
    is_error: bool,
}

fn normalize_mcp_input_schema(value: Option<&Value>) -> Value {
    match value {
        Some(schema) if schema.is_object() => schema.clone(),
        _ => json!({"type": "object", "properties": {}, "additionalProperties": true}),
    }
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

fn stdio_server_signature(server: &McpServer) -> String {
    serde_json::to_string(&server.transport)
        .unwrap_or_else(|_| format!("{}:{}", server.id, server.name))
}

async fn rpc_stdio_once(
    auth: &Arc<McpAuthRuntime>,
    environment: &Arc<EnvironmentRuntimeManager>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<McpRpcResponse> {
    let timeout_duration =
        Duration::from_millis(mcp_timeout_ms(&server.transport).clamp(1_000, 120_000));
    let mut session =
        StdioSession::start(server, timeout_duration, auth.clone(), environment.clone()).await?;
    let result = if let Some((method, params)) = request {
        session.request(method, params, timeout_duration).await
    } else {
        Ok(json!({}))
    };
    let handshake = session.handshake.clone();
    session.stop().await;
    result.map(|value| McpRpcResponse {
        value,
        handshake: Some(handshake),
    })
}

struct StdioSession {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    protocol: StdioProtocol,
    next_id: i64,
    stderr_task: Option<JoinHandle<()>>,
    stderr_output: Arc<Mutex<Vec<String>>>,
    auth: Arc<McpAuthRuntime>,
    server_id: Uuid,
    handshake: McpHandshake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StdioProtocol {
    ContentLength,
    JsonLines,
}

impl StdioSession {
    async fn start(
        server: &McpServer,
        timeout_duration: Duration,
        auth: Arc<McpAuthRuntime>,
        environment: Arc<EnvironmentRuntimeManager>,
    ) -> AppResult<Self> {
        auth.mark_starting(server.id).await;
        let primary = Self::start_with_protocol(
            server,
            timeout_duration,
            auth.clone(),
            environment.clone(),
            StdioProtocol::JsonLines,
        )
        .await;
        match primary {
            Ok(session) => {
                auth.mark_connected(server.id).await;
                Ok(session)
            }
            Err(primary_error) if should_try_stdio_protocol_fallback(&primary_error) => {
                let fallback = Self::start_with_protocol(
                    server,
                    timeout_duration,
                    auth.clone(),
                    environment,
                    StdioProtocol::ContentLength,
                )
                .await;
                match fallback {
                    Ok(session) => {
                        auth.mark_connected(server.id).await;
                        Ok(session)
                    }
                    Err(fallback_error) => {
                        let error = AppError::Process(format!(
                            "stdio protocol negotiation failed (content-length: {primary_error}; json-lines: {fallback_error})"
                        ));
                        auth.mark_error(server.id, error.to_string()).await;
                        Err(error)
                    }
                }
            }
            Err(error) => {
                auth.mark_error(server.id, error.to_string()).await;
                Err(error)
            }
        }
    }

    async fn start_with_protocol(
        server: &McpServer,
        timeout_duration: Duration,
        auth: Arc<McpAuthRuntime>,
        environment: Arc<EnvironmentRuntimeManager>,
        protocol: StdioProtocol,
    ) -> AppResult<Self> {
        let mut session = Self::spawn(server, auth.clone(), environment, protocol).await?;
        match timeout(timeout_duration, session.initialize()).await {
            Ok(Ok(handshake)) => {
                session.handshake = handshake;
                Ok(session)
            }
            Ok(Err(error)) => {
                session.stop().await;
                Err(error)
            }
            Err(_) => {
                session.stop().await;
                Err(mcp_timeout_error(timeout_duration))
            }
        }
    }

    async fn spawn(
        server: &McpServer,
        auth: Arc<McpAuthRuntime>,
        environment: Arc<EnvironmentRuntimeManager>,
        protocol: StdioProtocol,
    ) -> AppResult<Self> {
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
        let expanded_command = expand_mcp_value(command);
        let expanded_args = args
            .iter()
            .map(|arg| expand_mcp_value(arg))
            .collect::<Vec<_>>();
        let (program, launch_args) = normalize_stdio_launch(&expanded_command, &expanded_args);
        let managed_env = auth.managed_env(server);
        let configured_path = managed_env.get("PATH").map(OsStr::new);
        let inherited_path = std::env::var_os("PATH");
        let base_path = configured_path.or(inherited_path.as_deref());
        let managed_path = managed_path_from(environment.as_ref(), base_path).await;
        let command_path = managed_path
            .as_deref()
            .or(configured_path)
            .or(inherited_path.as_deref());
        let mut process = build_stdio_command(&program, &launch_args, command_path);
        process.env_clear();
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd
            .as_deref()
            .map(expand_mcp_value)
            .filter(|value| !value.trim().is_empty())
        {
            process.current_dir(cwd);
        }
        process.envs(&managed_env);
        if let Some(path) = managed_path.as_ref() {
            process.env("PATH", path);
        }
        configure_mcp_spawn_command(&mut process);
        let mut child = process
            .spawn()
            .map_err(|error| {
                AppError::Process(format!(
                    "MCP process failed to start: {error}; command={program:?}; args={launch_args:?}; {}",
                    stdio_launch_debug(&program, command_path)
                ))
            })?;
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
            reader: BufReader::new(stdout),
            protocol,
            next_id: 2,
            stderr_task,
            stderr_output,
            auth,
            server_id: server.id,
            handshake: McpHandshake::default(),
        })
    }

    async fn read_response(&mut self, expected_id: i64) -> AppResult<Value> {
        match read_stdio_response(&mut self.reader, self.protocol, expected_id).await {
            Ok(value) => Ok(value),
            Err(AppError::Process(message)) if message == "MCP process closed before response" => {
                self.drain_stderr_after_exit().await;
                let exit_detail = self.child.try_wait().ok().flatten().map(|status| {
                    status
                        .code()
                        .map(|code| format!("exit code {code}"))
                        .unwrap_or_else(|| "terminated by signal".to_string())
                });
                let stderr_detail = format_mcp_stderr(&self.stderr_output.lock().await);
                let details = [
                    exit_detail,
                    stderr_detail.map(|output| format!("stderr:\n{output}")),
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

    async fn drain_stderr_after_exit(&mut self) {
        let exited = match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => timeout(Duration::from_millis(250), self.child.wait())
                .await
                .is_ok(),
            Err(_) => false,
        };
        if exited {
            if let Some(task) = self.stderr_task.take() {
                let _ = task.await;
            }
        }
    }

    async fn initialize(&mut self) -> AppResult<McpHandshake> {
        send_stdio_message(&mut self.stdin, mcp_initialize_request(), self.protocol).await?;
        let result = self.read_response(1).await?;
        send_stdio_message(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            self.protocol,
        )
        .await?;
        Ok(parse_mcp_handshake(&result))
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
            send_stdio_message(
                &mut self.stdin,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                self.protocol,
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
        if let Some(pid) = self.child.id() {
            let _ = crate::tools::process_job::terminate_process_tree(pid).await;
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

impl Drop for StdioSession {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            let _ = self.child.start_kill();
            tracing::debug!(pid, "stopping dropped MCP stdio process");
        }
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

fn normalize_stdio_launch(command: &str, args: &[String]) -> (String, Vec<String>) {
    let mut command_parts = split_stdio_command(command);
    let program = command_parts
        .first()
        .cloned()
        .unwrap_or_else(|| command.trim().to_string());
    if !command_parts.is_empty() {
        command_parts.remove(0);
    }
    command_parts.extend(normalize_stdio_args(args));
    (program, command_parts)
}

fn split_stdio_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for character in command.chars() {
        match character {
            '"' | '\'' if quote.is_none() => quote = Some(character),
            '"' | '\'' if quote == Some(character) => quote = None,
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn build_stdio_command(program: &str, args: &[String], path: Option<&OsStr>) -> Command {
    #[cfg(windows)]
    if (program.eq_ignore_ascii_case("npx") || program.eq_ignore_ascii_case("npm"))
        && let Some(command) = build_node_package_manager_command_for_program(program, args, path)
    {
        return command;
    }

    #[cfg(windows)]
    if let Some(script) = resolve_windows_script(program, path) {
        let mut command = Command::new(windows_command_interpreter());
        command
            .args(["/D", "/S", "/C"])
            .arg(build_windows_script(&script, args));
        return command;
    }

    let mut command = Command::new(program);
    command.args(args);
    command
}

#[cfg(windows)]
fn build_node_package_manager_command(
    script: &Path,
    args: &[String],
    path: Option<&OsStr>,
) -> Option<Command> {
    let stem = script.file_stem()?.to_string_lossy();
    if !stem.eq_ignore_ascii_case("npx") && !stem.eq_ignore_ascii_case("npm") {
        return None;
    }
    let node = resolve_windows_executable("node", path)?;
    let cli = script
        .parent()?
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join(format!("{stem}-cli.js"));
    if !cli.is_file() {
        return None;
    }
    let mut command = Command::new(node);
    command.arg(cli).args(args);
    Some(command)
}

#[cfg(windows)]
fn build_node_package_manager_command_for_program(
    program: &str,
    args: &[String],
    path: Option<&OsStr>,
) -> Option<Command> {
    let Some(script) = resolve_windows_script(program, path) else {
        tracing::warn!(program, "MCP package manager shim was not found");
        return None;
    };
    let command = build_node_package_manager_command(&script, args, path);
    if command.is_none() {
        tracing::warn!(program, script = %script.display(), "MCP package manager shim could not be converted to a direct Node command");
    }
    command
}

#[cfg(windows)]
fn resolve_windows_executable(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let names = [
        PathBuf::from(program),
        PathBuf::from(format!("{program}.exe")),
    ];
    let mut directories = Vec::new();
    if let Some(path) = path {
        directories.extend(std::env::split_paths(path));
    }
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    directories.into_iter().find_map(|directory| {
        names.iter().find_map(|name| {
            let candidate = directory.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(windows)]
fn windows_command_interpreter() -> PathBuf {
    let candidates = [
        std::env::var_os("ComSpec").map(PathBuf::from),
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("cmd.exe")),
        Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

#[cfg(windows)]
fn resolve_windows_script(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let raw = PathBuf::from(program);
    let has_path = raw.components().count() > 1 || raw.extension().is_some();
    let names = if has_path {
        vec![raw]
    } else {
        vec![
            PathBuf::from(program),
            PathBuf::from(format!("{program}.exe")),
            PathBuf::from(format!("{program}.cmd")),
            PathBuf::from(format!("{program}.bat")),
        ]
    };

    let is_script = |candidate: &Path| {
        candidate.is_file()
            && candidate.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            })
    };
    if let Some(candidate) = names.iter().find(|candidate| is_script(candidate)) {
        return Some(candidate.clone());
    }

    let inherited_path = std::env::var_os("PATH");
    let mut directories = Vec::new();
    if let Some(path) = path {
        directories.extend(std::env::split_paths(path));
    }
    if let Some(path) = inherited_path.as_deref() {
        directories.extend(std::env::split_paths(path));
    }
    for variable in [
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
    ] {
        if let Some(root) = std::env::var_os(variable).map(PathBuf::from) {
            directories.push(root.join("npm"));
            directories.push(root.join("nodejs"));
        }
    }
    if let Some(user_profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        directories.push(user_profile.join(r"AppData\Roaming\npm"));
        directories.push(user_profile.join(r"AppData\Local\npm"));
    }
    for directory in directories {
        for name in &names {
            let candidate = directory.join(name);
            if is_script(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn stdio_launch_debug(program: &str, path: Option<&OsStr>) -> String {
    let resolved = resolve_windows_script(program, path)
        .map(|value| value.display().to_string())
        .unwrap_or_else(|| "<not resolved>".to_string());
    format!(
        "resolved_script={resolved:?}; comspec={:?}",
        windows_command_interpreter()
    )
}

#[cfg(not(windows))]
fn stdio_launch_debug(_program: &str, _path: Option<&OsStr>) -> String {
    "resolved_script=<not applicable>".to_string()
}

#[cfg(windows)]
fn build_windows_script(program: &Path, args: &[String]) -> String {
    let mut command = format!("call {}", quote_windows_cmd_arg(&program.to_string_lossy()));
    for arg in args {
        command.push(' ');
        command.push_str(&quote_windows_cmd_arg(arg));
    }
    command
}

#[cfg(windows)]
fn quote_windows_cmd_arg(value: &str) -> String {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn mcp_timeout_ms(transport: &McpTransport) -> u64 {
    match transport {
        McpTransport::Stdio {
            command,
            timeout_ms,
            ..
        } => {
            // The first run of npx/uv may install a package before the MCP
            // process starts speaking JSON-RPC.
            let package_manager = command
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(command)
                .trim_end_matches(".cmd")
                .trim_end_matches(".exe")
                .to_ascii_lowercase();
            if matches!(package_manager.as_str(), "npx" | "npm" | "uv" | "uvx") {
                (*timeout_ms).max(120_000)
            } else {
                *timeout_ms
            }
        }
        McpTransport::StreamableHttp { timeout_ms, .. } | McpTransport::Sse { timeout_ms, .. } => {
            *timeout_ms
        }
    }
}

fn mcp_timeout_error(timeout_duration: Duration) -> AppError {
    AppError::Process(format!(
        "MCP stdio request timed out after {}ms",
        timeout_duration.as_millis()
    ))
}

fn should_try_stdio_protocol_fallback(error: &AppError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("timed out")
        || message.contains("closed before response")
        || message.contains("content-length")
        || message.contains("stdio frame")
}

fn is_retryable_stdio_error(error: &AppError) -> bool {
    matches!(error, AppError::Process(_) | AppError::Json(_))
}

#[cfg(windows)]
fn configure_mcp_spawn_command(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_mcp_spawn_command(_command: &mut Command) {}

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

async fn rpc_streamable_http(
    auth: &Arc<McpAuthRuntime>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<McpRpcResponse> {
    let (url, configured_token, headers, client_cert, client_key, identity_header, timeout_ms) =
        match &server.transport {
            McpTransport::StreamableHttp {
                url,
                bearer_token,
                headers,
                client_cert,
                client_key,
                identity_header,
                timeout_ms,
                ..
            } => (
                url,
                bearer_token.as_deref(),
                headers,
                client_cert.as_deref(),
                client_key.as_deref(),
                identity_header.as_ref(),
                *timeout_ms,
            ),
            _ => unreachable!(),
        };
    if url.trim().is_empty() {
        return Err(AppError::BadRequest("MCP HTTP URL is required".to_string()));
    }
    let client = build_http_client(timeout_ms, client_cert, client_key).await?;
    let oauth_token = auth.oauth_access_token(server).await;
    let bearer_token = configured_token.or(oauth_token.as_deref());
    let mut session: Option<String> = None;
    let mut protocol_version = MCP_PROTOCOL_VERSION.to_string();
    let mut handshake = McpHandshake::default();
    let result = async {
        let init = http_request(
            &client,
            url,
            bearer_token,
            headers,
            session.as_deref(),
            mcp_initialize_request(),
            &protocol_version,
            identity_header,
            Some(1),
        )
        .await?;
        session = init.1;
        handshake = parse_mcp_handshake(&init.0);
        if let Some(version) = init.0.get("protocolVersion").and_then(Value::as_str) {
            protocol_version = version.to_string();
        }
        let _ = http_request(
            &client,
            url,
            bearer_token,
            headers,
            session.as_deref(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            &protocol_version,
            identity_header,
            None,
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
                &protocol_version,
                identity_header,
                Some(2),
            )
            .await?;
            Ok(value)
        } else {
            Ok(json!({}))
        }
    }
    .await;
    if let Some(session_id) = session.as_deref() {
        let _ = client
            .delete(url)
            .headers(build_headers(bearer_token, headers, identity_header, None)?)
            .header("MCP-Session-Id", session_id)
            .header("MCP-Protocol-Version", &protocol_version)
            .send()
            .await;
    }
    result.map(|value| McpRpcResponse {
        value,
        handshake: Some(handshake),
    })
}

async fn rpc_sse(
    auth: &Arc<McpAuthRuntime>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<McpRpcResponse> {
    let McpTransport::Sse {
        url,
        bearer_token,
        headers,
        client_cert,
        client_key,
        identity_header,
        timeout_ms,
        ..
    } = &server.transport
    else {
        unreachable!()
    };
    if url.trim().is_empty() {
        return Err(AppError::BadRequest("MCP SSE URL is required".to_string()));
    }
    let oauth_token = auth.oauth_access_token(server).await;
    let bearer_token = bearer_token.as_deref().or(oauth_token.as_deref());
    let timeout_duration = Duration::from_millis((*timeout_ms).clamp(1_000, 120_000));
    let client =
        build_http_client(*timeout_ms, client_cert.as_deref(), client_key.as_deref()).await?;
    let mut stream_response = client
        .get(url)
        .headers(build_headers(
            bearer_token.as_deref(),
            headers,
            identity_header.as_ref(),
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
    let base_url =
        reqwest::Url::parse(url).map_err(|error| AppError::BadRequest(error.to_string()))?;
    let endpoint_url = resolve_sse_message_endpoint(&base_url, &endpoint_data.0, &endpoint_data.1)?;

    let mut protocol_version = MCP_PROTOCOL_VERSION.to_string();
    let initialize = mcp_initialize_request();
    let initialize_response = post_sse_message(
        &client,
        &endpoint_url,
        bearer_token.as_deref(),
        headers,
        identity_header.as_ref(),
        initialize,
        &protocol_version,
    )
    .await?;
    let initialize_result = if initialize_response.trim().is_empty() {
        read_sse_response(&mut stream_response, timeout_duration, 1).await?
    } else {
        parse_rpc_response(&initialize_response)?
    };
    let handshake = parse_mcp_handshake(&initialize_result);
    if let Some(version) = initialize_result
        .get("protocolVersion")
        .and_then(Value::as_str)
    {
        protocol_version = version.to_string();
    }

    let _ = post_sse_message(
        &client,
        &endpoint_url,
        bearer_token.as_deref(),
        headers,
        identity_header.as_ref(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        &protocol_version,
    )
    .await?;
    let result = if let Some((method, params)) = request {
        let response = post_sse_message(
            &client,
            &endpoint_url,
            bearer_token.as_deref(),
            headers,
            identity_header.as_ref(),
            json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),
            &protocol_version,
        )
        .await?;
        if response.trim().is_empty() {
            read_sse_response(&mut stream_response, timeout_duration, 2).await
        } else {
            parse_rpc_response(&response)
        }
    } else {
        Ok(json!({}))
    };
    result.map(|value| McpRpcResponse {
        value,
        handshake: Some(handshake),
    })
}

async fn http_request(
    client: &reqwest::Client,
    url: &str,
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    session: Option<&str>,
    body: Value,
    protocol_version: &str,
    identity_header: Option<&crate::domain::McpIdentityHeader>,
    expected_id: Option<i64>,
) -> AppResult<(Value, Option<String>)> {
    let mut header_map = build_headers(
        bearer_token,
        headers,
        identity_header,
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
        .header("MCP-Protocol-Version", protocol_version)
        .json(&body)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = read_limited_body(response, MAX_HTTP_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(AppError::Llm(format!(
            "MCP HTTP endpoint returned {status}: {}",
            truncate_error_body(&text)
        )));
    }
    if text.trim().is_empty() {
        return Ok((json!({}), session));
    }
    let value = parse_http_mcp_value(&text, &content_type, expected_id)?;
    if let Some(expected_id) = expected_id {
        if value.get("id").and_then(Value::as_i64) != Some(expected_id)
            && value.get("result").is_none()
        {
            return Err(AppError::Llm(format!(
                "MCP response id mismatch: expected {expected_id}"
            )));
        }
    }
    Ok((parse_rpc_value(value)?, session))
}

async fn build_http_client(
    timeout_ms: u64,
    client_cert: Option<&str>,
    client_key: Option<&str>,
) -> AppResult<reqwest::Client> {
    let mut builder =
        reqwest::Client::builder().timeout(Duration::from_millis(timeout_ms.clamp(1_000, 120_000)));
    if let Some(cert_path) = client_cert.map(expand_mcp_value) {
        let cert = fs::read(PathBuf::from(cert_path)).await.map_err(|error| {
            AppError::BadRequest(format!("failed to read MCP client certificate: {error}"))
        })?;
        let identity_bytes = if let Some(key_path) = client_key.map(expand_mcp_value) {
            let key = fs::read(PathBuf::from(key_path)).await.map_err(|error| {
                AppError::BadRequest(format!("failed to read MCP client key: {error}"))
            })?;
            let mut combined = cert;
            combined.extend_from_slice(b"\n");
            combined.extend_from_slice(&key);
            combined
        } else {
            cert
        };
        let identity = reqwest::Identity::from_pem(&identity_bytes).map_err(|error| {
            AppError::BadRequest(format!("invalid MCP client certificate/key: {error}"))
        })?;
        builder = builder.identity(identity);
    } else if client_key.is_some() {
        return Err(AppError::BadRequest(
            "MCP client_key requires client_cert".to_string(),
        ));
    }
    builder
        .build()
        .map_err(|error| AppError::Llm(error.to_string()))
}

async fn read_limited_body(response: reqwest::Response, limit: usize) -> AppResult<String> {
    if let Some(length) = response.content_length() {
        if length > limit as u64 {
            return Err(AppError::Llm(format!("MCP response exceeds {limit} bytes")));
        }
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| AppError::Llm(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(AppError::Llm(format!("MCP response exceeds {limit} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|error| AppError::Llm(error.to_string()))
}

fn truncate_error_body(body: &str) -> String {
    body.chars().take(512).collect()
}

fn format_mcp_stderr(lines: &[String]) -> Option<String> {
    let output = lines.join("\n");
    if output.is_empty() {
        return None;
    }
    Some(output.chars().take(MAX_MCP_STDERR_ERROR_CHARS).collect())
}

fn parse_sse_response(text: &str, expected_id: Option<i64>) -> AppResult<Value> {
    for event in text.split("\n\n").chain(text.split("\r\n\r\n")) {
        if let Some((_, data)) = parse_sse_event(event) {
            if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                if expected_id.is_none() || value.get("id").and_then(Value::as_i64) == expected_id {
                    return Ok(value);
                }
            }
        }
    }
    Err(AppError::Llm(
        "MCP SSE response did not contain a matching JSON-RPC event".to_string(),
    ))
}

fn parse_http_mcp_value(
    text: &str,
    content_type: &str,
    expected_id: Option<i64>,
) -> AppResult<Value> {
    let looks_like_sse = content_type.contains("text/event-stream")
        || (expected_id.is_some() && text.contains("data:"));
    if looks_like_sse {
        if let Ok(value) = parse_sse_response(text, expected_id) {
            return Ok(value);
        }
    }
    serde_json::from_str::<Value>(text)
        .or_else(|_| parse_sse_json(text))
        .map_err(AppError::Json)
}

fn resolve_sse_message_endpoint(
    base_url: &reqwest::Url,
    event: &str,
    data: &str,
) -> AppResult<reqwest::Url> {
    let data = data.trim();
    if event.eq_ignore_ascii_case("ready") {
        return Ok(base_url.clone());
    }

    if let Ok(value) = serde_json::from_str::<Value>(data) {
        if let Some(endpoint) = value.get("endpoint").and_then(Value::as_str) {
            return resolve_sse_endpoint_url(base_url, endpoint);
        }
        if value.is_object() {
            return Ok(base_url.clone());
        }
        if let Some(endpoint) = value.as_str() {
            return resolve_sse_endpoint_url(base_url, endpoint);
        }
    }

    resolve_sse_endpoint_url(base_url, data)
}

fn resolve_sse_endpoint_url(base_url: &reqwest::Url, endpoint: &str) -> AppResult<reqwest::Url> {
    base_url
        .join(endpoint.trim())
        .or_else(|_| reqwest::Url::parse(endpoint.trim()))
        .map_err(|error| AppError::BadRequest(format!("Invalid MCP SSE message endpoint: {error}")))
}

fn build_headers(
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    identity_header: Option<&crate::domain::McpIdentityHeader>,
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
    let has_authorization = headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"));
    if !has_authorization
        && let Some(token) =
            bearer_token.filter(|token| !token.is_empty() && *token != "__preserve__")
    {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP bearer token: {error}")))?;
        header_map.insert("authorization", value);
    }
    if let Some(identity) = identity_header.filter(|item| !item.name.trim().is_empty()) {
        let name = HeaderName::from_bytes(identity.name.trim().as_bytes()).map_err(|error| {
            AppError::BadRequest(format!("Invalid MCP identity header name: {error}"))
        })?;
        let value = HeaderValue::from_str(&expand_mcp_value(&identity.value)).map_err(|error| {
            AppError::BadRequest(format!("Invalid MCP identity header value: {error}"))
        })?;
        if !header_map.keys().any(|key| key == &name) {
            header_map.insert(name, value);
        }
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
    identity_header: Option<&crate::domain::McpIdentityHeader>,
    body: Value,
    protocol_version: &str,
) -> AppResult<String> {
    let response = client
        .post(endpoint.clone())
        .headers(build_headers(
            bearer_token,
            headers,
            identity_header,
            Some("application/json, text/event-stream"),
        )?)
        .header("MCP-Protocol-Version", protocol_version)
        .json(&body)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    if body.len() > MAX_HTTP_BODY_BYTES {
        return Err(AppError::Llm(format!(
            "MCP SSE response exceeds {MAX_HTTP_BODY_BYTES} bytes"
        )));
    }
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
        if let Some((index, boundary_len)) = find_sse_boundary(&buffer) {
            let event = buffer.drain(..index).collect::<Vec<_>>();
            buffer.drain(..boundary_len);
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
        if buffer.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
            return Err(AppError::Llm(format!(
                "MCP SSE event exceeds {MAX_SSE_EVENT_BYTES} bytes"
            )));
        }
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

fn find_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    match (crlf, lf) {
        (Some(crlf), Some(lf)) if crlf < lf => Some((crlf, 4)),
        (Some(crlf), _) => Some((crlf, 4)),
        (_, Some(lf)) => Some((lf, 2)),
        _ => None,
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

async fn send_stdio_message(
    stdin: &mut tokio::process::ChildStdin,
    value: Value,
    protocol: StdioProtocol,
) -> AppResult<()> {
    let body = serde_json::to_vec(&value)?;
    match protocol {
        StdioProtocol::ContentLength => {
            let header = format!("Content-Length: {}\r\n\r\n", body.len());
            stdin.write_all(header.as_bytes()).await?;
            stdin.write_all(&body).await?;
        }
        StdioProtocol::JsonLines => {
            stdin.write_all(&body).await?;
            stdin.write_all(b"\n").await?;
        }
    }
    stdin.flush().await?;
    Ok(())
}

async fn read_stdio_response(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    protocol: StdioProtocol,
    expected_id: i64,
) -> AppResult<Value> {
    for _ in 0..STDIO_MAX_RESPONSE_WAIT_ITERATIONS {
        let value = read_stdio_message(reader, protocol).await?;
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(AppError::BadRequest(error.to_string()));
        }
        return Ok(value.get("result").cloned().unwrap_or(value));
    }
    Err(AppError::Process(
        "MCP response wait exceeded the maximum number of frames".to_string(),
    ))
}

async fn read_stdio_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    protocol: StdioProtocol,
) -> AppResult<Value> {
    match protocol {
        StdioProtocol::ContentLength => read_content_length_message(reader).await,
        StdioProtocol::JsonLines => read_json_lines_message(reader).await,
    }
}

async fn read_content_length_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> AppResult<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(AppError::Process(
                "MCP process closed before response".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if let Some(length) = content_length {
                return read_stdio_payload(reader, length).await;
            }
            continue;
        }
        if let Some(length) = parse_content_length(trimmed)? {
            content_length = Some(length);
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(value);
        }
    }
}

async fn read_json_lines_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> AppResult<Value> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(AppError::Process(
                "MCP process closed before response".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(value);
        }
        if let Some(length) = parse_content_length(trimmed)? {
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).await? == 0 {
                    return Err(AppError::Process(
                        "MCP process closed while reading stdio frame headers".to_string(),
                    ));
                }
                if header.trim().is_empty() {
                    return read_stdio_payload(reader, length).await;
                }
            }
        }
    }
}

fn parse_content_length(line: &str) -> AppResult<Option<usize>> {
    let lower = line.to_ascii_lowercase();
    let Some(raw) = lower.strip_prefix("content-length:") else {
        return Ok(None);
    };
    let length = raw
        .trim()
        .parse::<usize>()
        .map_err(|_| AppError::Process(format!("invalid Content-Length header: {line}")))?;
    if length > MAX_STDIO_FRAME_BYTES {
        return Err(AppError::Process(format!(
            "stdio frame exceeds {} bytes",
            MAX_STDIO_FRAME_BYTES
        )));
    }
    Ok(Some(length))
}

async fn read_stdio_payload(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    length: usize,
) -> AppResult<Value> {
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(AppError::from)
}
