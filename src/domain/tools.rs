use serde::{Deserialize, Serialize};

/// Terminal tool configuration stored in data/agents/{agent_id}/tools.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalToolConfig {
    pub enabled: bool,
    /// Commands matching these regex patterns are rejected before any allow rule.
    #[serde(default = "default_blocked_patterns")]
    pub blocked_patterns: Vec<String>,
    /// Commands matching these regex patterns require interactive approval.
    #[serde(default = "default_approval_patterns")]
    pub approval_required_patterns: Vec<String>,
    /// Legacy compatibility field. Persistent bypass patterns are no longer honored.
    #[serde(default)]
    pub always_allowed_patterns: Vec<String>,
    /// Exact, context-bound grants created by interactive "always allow".
    #[serde(default)]
    pub approval_grants: Vec<TerminalApprovalGrant>,
    /// Foreground wait timeout. A still-running command is moved to background after this time.
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
    /// Host roots that may be mounted/used by the tool. Empty means the server working directory.
    #[serde(default)]
    pub allowed_cwd_roots: Vec<String>,
    /// Process/container sandbox configuration.
    #[serde(default)]
    pub sandbox: TerminalSandboxConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// Require Docker and fail closed when the daemon is unavailable.
    Docker,
    /// Compatibility mode with cwd containment and a sanitized environment, but no kernel sandbox.
    NativeRestricted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSandboxConfig {
    #[serde(default = "default_sandbox_mode")]
    pub mode: SandboxMode,
    #[serde(default = "default_docker_image")]
    pub docker_image: String,
    #[serde(default)]
    pub network_enabled: bool,
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: u64,
    #[serde(default = "default_cpu_limit")]
    pub cpu_limit: f64,
    #[serde(default = "default_pids_limit")]
    pub pids_limit: u32,
    #[serde(default)]
    pub workspace_read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalApprovalGrant {
    pub command: String,
    pub cwd: Option<String>,
    pub sandbox_mode: SandboxMode,
    pub network_enabled: bool,
    pub workspace_read_only: bool,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_timeout_ms() -> u64 {
    30_000
}

fn default_sandbox_mode() -> SandboxMode {
    SandboxMode::Docker
}

fn default_docker_image() -> String {
    "rust:1-bookworm".to_string()
}

fn default_memory_limit_mb() -> u64 {
    512
}

fn default_cpu_limit() -> f64 {
    1.0
}

fn default_pids_limit() -> u32 {
    128
}

fn default_max_llm_iterations() -> u32 {
    20
}

fn default_blocked_patterns() -> Vec<String> {
    vec![
        r"rm\s+-rf\s+/".into(),
        r"curl.*\|\s*sh".into(),
        r"wget.*\|\s*sh".into(),
        r"sudo\s+".into(),
        r"chmod\s+777".into(),
        r";\s*rm\s+-rf".into(),
        r"\$\(".into(),
        r"(?i)\bformat\s+[a-z]:".into(),
        r"(?i)\bdel\s+/[fsq]".into(),
        r"(?i)\brmdir\s+/s".into(),
        r"(?i)\bRemove-Item\b.*\b-Recurse\b".into(),
    ]
}

fn default_approval_patterns() -> Vec<String> {
    vec![
        r"(?i)\brm\b".into(),
        r"(?i)\bgit\s+(?:-\S+\s+)*push\b".into(),
        r"(?i)\bgit\s+(?:-\S+\s+)*reset\b".into(),
        r"(?i)\bdocker\s+rm\b".into(),
        r"(?i)\bnpm\s+publish\b".into(),
        r"(?i)\bcargo\s+publish\b".into(),
        r"(?i)\bRemove-Item\b".into(),
        r"(?i)\bdel\b".into(),
        r"(?i)\berase\b".into(),
        r"(?i)\brd\b".into(),
        r"(?i)\brmdir\b".into(),
    ]
}

impl Default for TerminalSandboxConfig {
    fn default() -> Self {
        Self {
            mode: default_sandbox_mode(),
            docker_image: default_docker_image(),
            network_enabled: false,
            memory_limit_mb: default_memory_limit_mb(),
            cpu_limit: default_cpu_limit(),
            pids_limit: default_pids_limit(),
            workspace_read_only: false,
        }
    }
}

impl Default for TerminalToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            blocked_patterns: default_blocked_patterns(),
            approval_required_patterns: default_approval_patterns(),
            always_allowed_patterns: Vec::new(),
            approval_grants: Vec::new(),
            default_timeout_ms: default_timeout_ms(),
            allowed_cwd_roots: Vec::new(),
            sandbox: TerminalSandboxConfig::default(),
        }
    }
}

/// Agent tool configuration collection (tools.json top-level structure).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        self.max_llm_iterations.clamp(1, 50) as usize
    }
}

impl TerminalToolConfig {
    pub fn has_context_grant(&self, command: &str, cwd: Option<&str>) -> bool {
        let effective_network_enabled = match &self.sandbox.mode {
            SandboxMode::Docker => self.sandbox.network_enabled,
            SandboxMode::NativeRestricted => true,
        };
        self.approval_grants.iter().any(|grant| {
            grant.command == command
                && grant.cwd.as_deref() == cwd
                && grant.sandbox_mode == self.sandbox.mode
                && grant.network_enabled == effective_network_enabled
                && grant.workspace_read_only == self.sandbox.workspace_read_only
                && grant
                    .expires_at
                    .is_some_and(|expires_at| expires_at > chrono::Utc::now())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandDecision {
    Allowed,
    NeedsApproval,
    Blocked(String),
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
    pub network_enabled: bool,
    pub write_roots: Vec<String>,
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
}
