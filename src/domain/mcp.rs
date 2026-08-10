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
        #[serde(default = "default_mcp_timeout_ms")]
        timeout_ms: u64,
    },
    Sse {
        url: String,
        #[serde(default)]
        bearer_token: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default = "default_mcp_timeout_ms")]
        timeout_ms: u64,
    },
}

fn default_mcp_timeout_ms() -> u64 {
    30_000
}

impl Default for McpTransport {
    fn default() -> Self {
        Self::Stdio {
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            long_running: false,
            timeout_ms: default_mcp_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub qualified_name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_input_schema")]
    pub input_schema: Value,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn default_input_schema() -> Value {
    serde_json::json!({"type": "object"})
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServer {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
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
    #[serde(default = "default_true")]
    pub enabled: bool,
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
                ..
            }
            | Self::Sse {
                bearer_token,
                headers,
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
                    ..
                },
                Self::StreamableHttp {
                    bearer_token: old_token,
                    headers: old,
                    ..
                },
            )
            | (
                Self::Sse {
                    bearer_token: current_token,
                    headers: current,
                    ..
                },
                Self::Sse {
                    bearer_token: old_token,
                    headers: old,
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
            }
            _ => {}
        }
        merged
    }
}

fn mask_secret(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 4 {
        return "****".to_string();
    }
    let suffix = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("****{suffix}")
}

impl McpServer {
    pub fn redacted(&self) -> Self {
        let mut value = self.clone();
        value.transport = value.transport.redacted();
        value
    }
}

pub fn qualify_tool_name(server_id: Uuid, name: &str) -> String {
    qualify_tool_name_with_suffix(server_id, name, 1)
}

pub fn qualify_tool_name_with_suffix(server_id: Uuid, name: &str, occurrence: usize) -> String {
    let short_id = server_id.to_string().replace('-', "");
    let mut sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
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
    let prefix = format!("mcp__{}__", &short_id[..8]);
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
        let server_id = Uuid::from_u128(0x1234567890abcdef1234567890abcdef);
        let first = qualify_tool_name(server_id, "read.file");
        let collision = qualify_tool_name_with_suffix(server_id, "read/file", 2);
        let long_name = qualify_tool_name(server_id, &"x".repeat(200));

        assert_eq!(first, "mcp__12345678__read_file");
        assert_eq!(collision, "mcp__12345678__read_file_2");
        assert_ne!(first, collision);
        assert!(long_name.len() <= 64);
        assert!(long_name.starts_with("mcp__12345678__"));
    }

    #[test]
    fn redacts_and_restores_http_secrets() {
        let transport = McpTransport::StreamableHttp {
            url: "https://example.test/mcp".to_string(),
            bearer_token: Some("token-1234".to_string()),
            headers: [("X-Api-Key".to_string(), "key-5678".to_string())]
                .into_iter()
                .collect(),
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
                timeout_ms: 30_000,
            }
        );

        let preserved = McpTransport::StreamableHttp {
            url: "https://example.test/mcp".to_string(),
            bearer_token: Some("__preserve__".to_string()),
            headers: [("X-Api-Key".to_string(), "__preserve__".to_string())]
                .into_iter()
                .collect(),
            timeout_ms: 30_000,
        }
        .merge_redacted_secrets(&transport);
        assert_eq!(preserved, transport);
    }
}
