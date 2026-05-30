use serde::{Deserialize, Serialize};

/// 终端工具配置，存储在 data/agents/{agent_id}/tools.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalToolConfig {
    pub enabled: bool,
    /// 直接拒绝的命令模式（正则）
    pub blocked_patterns: Vec<String>,
    /// 需要审批的命令模式（正则）
    pub approval_required_patterns: Vec<String>,
    /// 默认超时(ms)，超时后自动转后台
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
    /// 允许的工作目录根路径
    #[serde(default)]
    pub allowed_cwd_roots: Vec<String>,
}

fn default_timeout_ms() -> u64 {
    30000
}

impl Default for TerminalToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            blocked_patterns: vec![
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
            ],
            approval_required_patterns: vec![
                r"rm\s+".into(),
                r"git\s+push".into(),
                r"git\s+reset".into(),
                r"docker\s+rm".into(),
                r"npm\s+publish".into(),
                r"cargo\s+publish".into(),
                r"(?i)\bRemove-Item\b".into(),
                r"(?i)\bdel\b".into(),
                r"(?i)\brmdir\b".into(),
            ],
            default_timeout_ms: default_timeout_ms(),
            allowed_cwd_roots: vec![],
        }
    }
}

/// Agent 工具配置集合（tools.json 顶层结构）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentToolsConfig {
    #[serde(default)]
    pub terminal: TerminalToolConfig,
}

/// 命令安全检查结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandDecision {
    Allowed,
    NeedsApproval,
    Blocked(String),
}

/// 审批决策
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
    AlwaysApprove,
}

/// 审批请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub decision: ApprovalDecision,
}

/// 审批响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 待审批条目（内存中存储）
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PendingApproval {
    pub approval_id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub agent_id: uuid::Uuid,
    pub created_at: std::time::Instant,
}

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolResult {
    Completed {
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u64,
    },
    Running {
        process_id: String,
        message: String,
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
