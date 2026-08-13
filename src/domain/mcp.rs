use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum McpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        inherit_env: bool,
        #[serde(default)]
        env_allowlist: Vec<String>,
        #[serde(default = "default_long_running")]
        long_running: bool,
        #[serde(default = "default_mcp_timeout_ms")]
        timeout_ms: u64,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        bearer_token: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        client_cert: Option<String>,
        #[serde(default)]
        client_key: Option<String>,
        #[serde(default)]
        identity_header: Option<McpIdentityHeader>,
        #[serde(default)]
        oauth: Option<McpOAuthConfig>,
        #[serde(default = "default_mcp_timeout_ms")]
        timeout_ms: u64,
    },
    Sse {
        url: String,
        #[serde(default)]
        bearer_token: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        client_cert: Option<String>,
        #[serde(default)]
        client_key: Option<String>,
        #[serde(default)]
        identity_header: Option<McpIdentityHeader>,
        #[serde(default)]
        oauth: Option<McpOAuthConfig>,
        #[serde(default = "default_mcp_timeout_ms")]
        timeout_ms: u64,
    },
}

fn default_mcp_timeout_ms() -> u64 {
    30_000
}

fn default_long_running() -> bool {
    true
}

impl Default for McpTransport {
    fn default() -> Self {
        Self::Stdio {
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            inherit_env: false,
            env_allowlist: Vec::new(),
            long_running: true,
            timeout_ms: default_mcp_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpIdentityHeader {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpOAuthConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub redirect_port: Option<u16>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct McpDiagnostics {
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub server_version: Option<String>,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub last_operation: Option<String>,
    #[serde(default)]
    pub last_operation_at: Option<String>,
    #[serde(default)]
    pub last_duration_ms: Option<u64>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub last_error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTool {
    #[serde(default = "new_mcp_tool_id")]
    pub tool_id: Uuid,
    pub name: String,
    pub qualified_name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_input_schema")]
    pub input_schema: Value,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn new_mcp_tool_id() -> Uuid {
    Uuid::new_v4()
}

fn default_input_schema() -> Value {
    serde_json::json!({"type": "object"})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServer {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub supports_parallel_tool_calls: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub provider_url: Option<String>,
    #[serde(default)]
    pub logo_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub transport: McpTransport,
    #[serde(default)]
    pub tools: Vec<McpTool>,
    #[serde(default)]
    pub status: McpServerStatus,
    #[serde(default)]
    pub last_synced_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub diagnostics: McpDiagnostics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpServerStatus {
    #[default]
    Unknown,
    Connecting,
    Ready,
    Offline,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthStatus {
    #[default]
    Idle,
    Starting,
    AuthPending,
    BrowserOpened,
    WaitingCallback,
    Authorized,
    Connected,
    AuthTimeout,
    AuthFailed,
    LaunchFailed,
    InitFailed,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpAuthState {
    pub status: McpAuthStatus,
    #[serde(default)]
    pub authorize_url: Option<String>,
    #[serde(default)]
    pub last_success_at: Option<String>,
    #[serde(default)]
    pub last_updated_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub browser_opened: bool,
    pub session_dir: Option<String>,
}

impl McpAuthState {
    pub fn unsupported() -> Self {
        Self {
            status: McpAuthStatus::Unsupported,
            authorize_url: None,
            last_success_at: None,
            last_updated_at: None,
            last_error: None,
            browser_opened: false,
            session_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct McpServerDraft {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub enabled: bool,
    #[serde(default = "crate::domain::serde_defaults::default_true")]
    pub supports_parallel_tool_calls: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub provider_url: Option<String>,
    #[serde(default)]
    pub logo_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpServerOverride {
    pub server_id: Uuid,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub customized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpToolOverride {
    pub server_id: Uuid,
    pub tool_name: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpAgentOverrides {
    #[serde(default)]
    pub servers: Vec<McpServerOverride>,
    #[serde(default)]
    pub tools: Vec<McpToolOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpAgentSettings {
    pub defaults: Vec<McpServer>,
    pub overrides: McpAgentOverrides,
    pub effective: Vec<McpServer>,
}

impl McpTransport {
    pub fn redacted(&self) -> Self {
        let mut transport = self.clone();
        match &mut transport {
            Self::Stdio { env, .. } => {
                for value in env.values_mut() {
                    if !value.is_empty() {
                        *value = mask_secret(value);
                    }
                }
            }
            Self::StreamableHttp {
                bearer_token,
                headers,
                identity_header,
                oauth,
                ..
            }
            | Self::Sse {
                bearer_token,
                headers,
                identity_header,
                oauth,
                ..
            } => {
                if let Some(value) = bearer_token.as_mut() {
                    *value = mask_secret(value);
                }
                for value in headers.values_mut() {
                    if !value.is_empty() {
                        *value = mask_secret(value);
                    }
                }
                if let Some(identity) = identity_header.as_mut() {
                    if !identity.value.is_empty() {
                        identity.value = mask_secret(&identity.value);
                    }
                }
                if let Some(oauth) = oauth.as_mut() {
                    if let Some(secret) = oauth.client_secret.as_mut() {
                        *secret = mask_secret(secret);
                    }
                }
            }
        }
        transport
    }

    pub fn merge_redacted_secrets(self, previous: &Self) -> Self {
        let mut merged = self;
        match (&mut merged, previous) {
            (Self::Stdio { env: current, .. }, Self::Stdio { env: old, .. }) => {
                for (key, value) in current.iter_mut() {
                    if value == "__preserve__" {
                        if let Some(previous_value) = old.get(key) {
                            *value = previous_value.clone();
                        }
                    }
                }
            }
            (
                Self::StreamableHttp {
                    bearer_token: current_token,
                    headers: current,
                    identity_header: current_identity,
                    oauth: current_oauth,
                    ..
                },
                Self::StreamableHttp {
                    bearer_token: old_token,
                    headers: old,
                    identity_header: old_identity,
                    oauth: old_oauth,
                    ..
                },
            )
            | (
                Self::Sse {
                    bearer_token: current_token,
                    headers: current,
                    identity_header: current_identity,
                    oauth: current_oauth,
                    ..
                },
                Self::Sse {
                    bearer_token: old_token,
                    headers: old,
                    identity_header: old_identity,
                    oauth: old_oauth,
                    ..
                },
            ) => {
                if current_token.as_deref() == Some("__preserve__") {
                    *current_token = old_token.clone();
                }
                for (key, value) in current.iter_mut() {
                    if value == "__preserve__" {
                        if let Some(previous_value) = old.get(key) {
                            *value = previous_value.clone();
                        }
                    }
                }
                if let Some(identity) = current_identity
                    .as_mut()
                    .filter(|identity| identity.value == "__preserve__")
                {
                    if let Some(previous) = old_identity {
                        identity.value = previous.value.clone();
                    } else {
                        identity.value.clear();
                    }
                }
                if let Some(oauth) = current_oauth
                    .as_mut()
                    .filter(|oauth| oauth.client_secret.as_deref() == Some("__preserve__"))
                {
                    oauth.client_secret = old_oauth
                        .as_ref()
                        .and_then(|previous| previous.client_secret.clone());
                }
            }
            _ => {}
        }
        merged
    }
}

fn mask_secret(value: &str) -> String {
    crate::domain::tools::mask_secret_tail(value, true)
}

impl McpServer {
    pub fn redacted(&self) -> Self {
        let mut value = self.clone();
        value.transport = value.transport.redacted();
        value
    }
}

pub fn qualify_tool_name(server_name: &str, name: &str) -> String {
    qualify_tool_name_with_suffix(server_name, name, 1)
}

pub fn qualify_tool_name_with_suffix(server_name: &str, name: &str, occurrence: usize) -> String {
    let mut server = server_name
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if server.is_empty() {
        server.push_str("server");
    }
    let mut sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        sanitized.push_str("tool");
    }
    let suffix = if occurrence > 1 {
        format!("_{occurrence}")
    } else {
        String::new()
    };
    let prefix = format!("mcp_{}_", server);
    let max_name_chars = 64usize.saturating_sub(prefix.len() + suffix.len());
    format!(
        "{}{}{}",
        prefix,
        sanitized.chars().take(max_name_chars).collect::<String>(),
        suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualifies_tool_names_with_sanitization_and_collision_suffixes() {
        let first = qualify_tool_name("Local MCP", "read.file");
        let collision = qualify_tool_name_with_suffix("Local MCP", "read/file", 2);
        let long_name = qualify_tool_name("Local MCP", &"x".repeat(200));

        assert_eq!(first, "mcp_Local_MCP_read_file");
        assert_eq!(collision, "mcp_Local_MCP_read_file_2");
        assert_eq!(
            qualify_tool_name("本地MCP网关工具", "codegraph"),
            "mcp_本地MCP网关工具_codegraph"
        );
        assert_ne!(first, collision);
        assert!(long_name.len() <= 64);
        assert!(long_name.starts_with("mcp_Local_MCP_"));
    }

    #[test]
    fn redacts_and_restores_http_secrets() {
        let transport = McpTransport::StreamableHttp {
            url: "https://example.test/mcp".to_string(),
            bearer_token: Some("token-1234".to_string()),
            headers: [("X-Api-Key".to_string(), "key-5678".to_string())]
                .into_iter()
                .collect(),
            client_cert: None,
            client_key: None,
            identity_header: None,
            oauth: None,
            timeout_ms: 30_000,
        };
        let redacted = transport.redacted();
        assert_eq!(
            redacted,
            McpTransport::StreamableHttp {
                url: "https://example.test/mcp".to_string(),
                bearer_token: Some("****1234".to_string()),
                headers: [("X-Api-Key".to_string(), "****5678".to_string())]
                    .into_iter()
                    .collect(),
                client_cert: None,
                client_key: None,
                identity_header: None,
                oauth: None,
                timeout_ms: 30_000,
            }
        );

        let preserved = McpTransport::StreamableHttp {
            url: "https://example.test/mcp".to_string(),
            bearer_token: Some("__preserve__".to_string()),
            headers: [("X-Api-Key".to_string(), "__preserve__".to_string())]
                .into_iter()
                .collect(),
            client_cert: None,
            client_key: None,
            identity_header: None,
            oauth: None,
            timeout_ms: 30_000,
        }
        .merge_redacted_secrets(&transport);
        assert_eq!(preserved, transport);
    }
}
