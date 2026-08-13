// Internal MCP transport/auth helpers split out of the manager module.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
    task::JoinHandle,
    time::{Duration, timeout},
};
use uuid::Uuid;

use super::env::{expand_mcp_value, safe_baseline_environment};
use super::protocol::is_mcp_remote;
use crate::{
    domain::{McpAuthState, McpAuthStatus, McpServer, McpTransport},
    error::{AppError, AppResult},
};

#[derive(Clone)]
pub(super) struct McpAuthRuntime {
    pub(super) root: PathBuf,
    pub(super) states: Arc<Mutex<HashMap<Uuid, McpAuthState>>>,
    pub(super) browser_opened_at: Arc<Mutex<HashMap<Uuid, Instant>>>,
    pub(super) oauth_tasks: Arc<Mutex<HashMap<Uuid, JoinHandle<()>>>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct McpOAuthToken {
    pub(super) access_token: String,
    #[serde(default)]
    pub(super) refresh_token: Option<String>,
    #[serde(default)]
    pub(super) token_type: Option<String>,
    #[serde(default)]
    pub(super) expires_at: Option<i64>,
}

impl McpAuthRuntime {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            states: Arc::new(Mutex::new(HashMap::new())),
            browser_opened_at: Arc::new(Mutex::new(HashMap::new())),
            oauth_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn session_dir(&self, server_id: Uuid) -> PathBuf {
        self.root.join(server_id.to_string())
    }

    pub(super) fn default_state(&self, server_id: Uuid) -> McpAuthState {
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

    pub(super) async fn state(&self, server_id: Uuid, supported: bool) -> McpAuthState {
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

    pub(super) async fn save(&self, server_id: Uuid, state: &McpAuthState) {
        let dir = self.session_dir(server_id);
        if fs::create_dir_all(&dir).await.is_err() {
            return;
        }
        if let Ok(content) = serde_json::to_string_pretty(state) {
            let _ = fs::write(dir.join("state.json"), content).await;
        }
    }

    pub(super) async fn update<F>(&self, server_id: Uuid, update: F) -> McpAuthState
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

    pub(super) async fn mark_starting(&self, server_id: Uuid) {
        self.update(server_id, |state| {
            state.status = McpAuthStatus::Starting;
            state.authorize_url = None;
            state.browser_opened = false;
            state.last_error = None;
        })
        .await;
    }

    pub(super) async fn mark_connected(&self, server_id: Uuid) {
        self.update(server_id, |state| {
            state.status = McpAuthStatus::Connected;
            state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
            state.last_error = None;
        })
        .await;
    }

    pub(super) async fn mark_error(&self, server_id: Uuid, error: String) {
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

    pub(super) async fn mark_timeout(&self, server_id: Uuid, error: String) {
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

    pub(super) async fn handle_stderr_line(&self, server_id: Uuid, line: &str) {
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

    pub(super) async fn clear(&self, server_id: Uuid) -> McpAuthState {
        if let Some(task) = self.oauth_tasks.lock().await.remove(&server_id) {
            task.abort();
        }
        self.states.lock().await.remove(&server_id);
        self.browser_opened_at.lock().await.remove(&server_id);
        let _ = fs::remove_dir_all(self.session_dir(server_id)).await;
        self.default_state(server_id)
    }

    pub(super) async fn oauth_access_token(&self, server: &McpServer) -> Option<String> {
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

    pub(super) async fn start_oauth(&self, server: &McpServer) -> AppResult<McpAuthState> {
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

    pub(super) async fn cleanup_orphans(&self, active_server_ids: &HashSet<Uuid>) {
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

    pub(super) fn managed_env(&self, server: &McpServer) -> std::collections::BTreeMap<String, String> {
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

pub(super) fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

pub(super) fn extract_authorize_url(line: &str, lower: &str) -> Option<String> {
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

pub(super) fn open_browser_url(url: &str) -> Result<(), String> {
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
