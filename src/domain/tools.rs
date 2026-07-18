use serde::{Deserialize, Serialize};

/// Terminal tool configuration stored in data/agents/{agent_id}/tools.json.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalToolConfig {
    pub enabled: bool,
    /// User-facing permission mode for local terminal execution.
    #[serde(default)]
    pub permission_mode: TerminalPermissionMode,
    /// Exact, context-bound grants created by interactive "always allow".
    #[serde(default)]
    pub approval_grants: Vec<TerminalApprovalGrant>,
    /// Foreground wait timeout. A still-running command is moved to background after this time.
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
    /// Host roots that may be mounted/used by the tool. Empty means the WeiAgent data directory.
    #[serde(default)]
    pub allowed_cwd_roots: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalPermissionMode {
    /// Routine work runs in the local OS sandbox. Full host access requires user approval.
    #[default]
    RequestApproval,
    /// Codex runs commands with full host access and no approval prompts.
    FullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalApprovalGrant {
    pub command: String,
    pub cwd: Option<String>,
    pub permission_mode: TerminalPermissionMode,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_timeout_ms() -> u64 {
    30_000
}

pub const DEFAULT_MAX_LLM_ITERATIONS: u32 = 99;
pub const MAX_LLM_ITERATIONS: u32 = 999;

fn default_max_llm_iterations() -> u32 {
    DEFAULT_MAX_LLM_ITERATIONS
}

impl Default for TerminalToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            permission_mode: TerminalPermissionMode::RequestApproval,
            approval_grants: Vec::new(),
            default_timeout_ms: default_timeout_ms(),
            allowed_cwd_roots: Vec::new(),
        }
    }
}

/// Agent tool configuration collection (tools.json top-level structure).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentToolsConfig {
    /// Maximum tool-capable LLM iterations for one user turn.
    /// After this budget is exhausted, one final no-tools summary call is made.
    #[serde(default = "default_max_llm_iterations")]
    pub max_llm_iterations: u32,
    #[serde(default)]
    pub terminal: TerminalToolConfig,
}

impl Default for AgentToolsConfig {
    fn default() -> Self {
        Self {
            max_llm_iterations: default_max_llm_iterations(),
            terminal: TerminalToolConfig::default(),
        }
    }
}

impl AgentToolsConfig {
    pub fn max_llm_iterations(&self) -> usize {
        self.max_llm_iterations.clamp(1, MAX_LLM_ITERATIONS) as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentToolsOverrides {
    #[serde(default = "tool_overrides_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_llm_iterations: Option<u32>,
    #[serde(default)]
    pub terminal: TerminalToolOverrides,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalToolOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<TerminalPermissionMode>,
    #[serde(default)]
    pub approval_grants: Vec<TerminalApprovalGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_cwd_roots: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolsSettings {
    pub defaults: AgentToolsConfig,
    pub overrides: AgentToolsOverrides,
    pub effective: AgentToolsConfig,
}

fn tool_overrides_version() -> u32 {
    1
}

impl Default for AgentToolsOverrides {
    fn default() -> Self {
        Self {
            version: tool_overrides_version(),
            max_llm_iterations: None,
            terminal: TerminalToolOverrides::default(),
        }
    }
}

impl AgentToolsOverrides {
    pub fn remove_expired_approval_grants(&mut self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let previous_len = self.terminal.approval_grants.len();
        self.terminal
            .approval_grants
            .retain(|grant| grant.expires_at.is_some_and(|expires_at| expires_at > now));
        self.terminal.approval_grants.len() != previous_len
    }

    pub fn apply_to(&self, defaults: &AgentToolsConfig) -> AgentToolsConfig {
        AgentToolsConfig {
            max_llm_iterations: self
                .max_llm_iterations
                .unwrap_or(defaults.max_llm_iterations),
            terminal: TerminalToolConfig {
                enabled: self.terminal.enabled.unwrap_or(defaults.terminal.enabled),
                permission_mode: self
                    .terminal
                    .permission_mode
                    .clone()
                    .unwrap_or_else(|| defaults.terminal.permission_mode.clone()),
                approval_grants: self.terminal.approval_grants.clone(),
                default_timeout_ms: self
                    .terminal
                    .default_timeout_ms
                    .unwrap_or(defaults.terminal.default_timeout_ms),
                allowed_cwd_roots: self
                    .terminal
                    .allowed_cwd_roots
                    .clone()
                    .unwrap_or_else(|| defaults.terminal.allowed_cwd_roots.clone()),
            },
        }
    }

    pub fn from_full_config(
        config: &AgentToolsConfig,
        defaults: &AgentToolsConfig,
    ) -> AgentToolsOverrides {
        AgentToolsOverrides {
            version: tool_overrides_version(),
            max_llm_iterations: (config.max_llm_iterations != defaults.max_llm_iterations)
                .then_some(config.max_llm_iterations),
            terminal: TerminalToolOverrides {
                enabled: (config.terminal.enabled != defaults.terminal.enabled)
                    .then_some(config.terminal.enabled),
                permission_mode: (config.terminal.permission_mode
                    != defaults.terminal.permission_mode)
                    .then_some(config.terminal.permission_mode.clone()),
                approval_grants: config.terminal.approval_grants.clone(),
                default_timeout_ms: (config.terminal.default_timeout_ms
                    != defaults.terminal.default_timeout_ms)
                    .then_some(config.terminal.default_timeout_ms),
                allowed_cwd_roots: (config.terminal.allowed_cwd_roots
                    != defaults.terminal.allowed_cwd_roots)
                    .then_some(config.terminal.allowed_cwd_roots.clone()),
            },
        }
    }

    #[cfg(test)]
    pub fn full_override(config: &AgentToolsConfig) -> AgentToolsOverrides {
        AgentToolsOverrides {
            version: tool_overrides_version(),
            max_llm_iterations: Some(config.max_llm_iterations),
            terminal: TerminalToolOverrides {
                enabled: Some(config.terminal.enabled),
                permission_mode: Some(config.terminal.permission_mode.clone()),
                approval_grants: config.terminal.approval_grants.clone(),
                default_timeout_ms: Some(config.terminal.default_timeout_ms),
                allowed_cwd_roots: Some(config.terminal.allowed_cwd_roots.clone()),
            },
        }
    }
}

impl TerminalToolConfig {
    pub fn has_context_grant(&self, command: &str, cwd: Option<&str>) -> bool {
        self.approval_grants.iter().any(|grant| {
            grant.command == command
                && grant.cwd.as_deref() == cwd
                && grant.permission_mode == TerminalPermissionMode::RequestApproval
                && grant
                    .expires_at
                    .is_some_and(|expires_at| expires_at > chrono::Utc::now())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_loop_defaults_to_ninety_nine_iterations() {
        assert_eq!(
            AgentToolsConfig::default().max_llm_iterations,
            DEFAULT_MAX_LLM_ITERATIONS
        );
        assert_eq!(DEFAULT_MAX_LLM_ITERATIONS, 99);
    }

    #[test]
    fn runtime_iteration_count_is_bounded_by_the_supported_maximum() {
        let config = AgentToolsConfig {
            max_llm_iterations: MAX_LLM_ITERATIONS + 1,
            ..AgentToolsConfig::default()
        };
        assert_eq!(config.max_llm_iterations(), MAX_LLM_ITERATIONS as usize);
    }

    #[test]
    fn empty_agent_grants_are_present_in_api_json() {
        let value = serde_json::to_value(AgentToolsOverrides::default()).unwrap();
        assert_eq!(value["terminal"]["approval_grants"], serde_json::json!([]));
    }

    #[test]
    fn expired_and_undated_approval_grants_are_removed() {
        let now = chrono::Utc::now();
        let mut overrides = AgentToolsOverrides::default();
        overrides.terminal.approval_grants = vec![
            TerminalApprovalGrant {
                command: "expired".to_string(),
                cwd: None,
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(now - chrono::Duration::hours(25)),
                expires_at: Some(now - chrono::Duration::hours(1)),
            },
            TerminalApprovalGrant {
                command: "missing-expiry".to_string(),
                cwd: None,
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(now),
                expires_at: None,
            },
            TerminalApprovalGrant {
                command: "active".to_string(),
                cwd: Some("workspace".to_string()),
                permission_mode: TerminalPermissionMode::RequestApproval,
                created_at: Some(now),
                expires_at: Some(now + chrono::Duration::hours(1)),
            },
        ];

        assert!(overrides.remove_expired_approval_grants(now));
        assert_eq!(overrides.terminal.approval_grants.len(), 1);
        assert_eq!(overrides.terminal.approval_grants[0].command, "active");
        assert!(!overrides.remove_expired_approval_grants(now));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
    AlwaysApprove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub agent_id: uuid::Uuid,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub approval_id: String,
    pub agent_id: uuid::Uuid,
    pub conversation_id: uuid::Uuid,
    pub tool_call_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub runtime: String,
    pub permission: String,
    pub justification: Option<String>,
    pub background: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolResult {
    Completed {
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u64,
        /// Execution runtime actually used for this command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        #[serde(default)]
        output_cursor: u64,
    },
    Running {
        process_id: String,
        message: String,
        #[serde(default)]
        stdout: String,
        #[serde(default)]
        stderr: String,
        #[serde(default)]
        duration_ms: u64,
        /// Execution runtime actually used for this command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        #[serde(default)]
        output_cursor: u64,
    },
    Blocked {
        reason: String,
    },
    PendingApproval {
        command: String,
        approval_id: String,
    },
    Rejected {
        reason: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub process_id: String,
    pub agent_id: uuid::Uuid,
    pub conversation_id: uuid::Uuid,
    pub tool_call_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub status: ProcessStatus,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: u64,
    pub runtime: String,
    pub tty: bool,
    pub output_cursor: u64,
}
