use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexRuntimeSource {
    Managed,
    Environment,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeComponentKind {
    Codex,
    Node,
    Python,
    Uv,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeComponentStatus {
    pub kind: RuntimeComponentKind,
    pub available: bool,
    pub required: bool,
    pub managed_installed: bool,
    pub installing: bool,
    pub source: Option<CodexRuntimeSource>,
    pub detected_version: Option<String>,
    pub target_version: Option<String>,
    pub update_available: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRuntimeStatus {
    pub supported: bool,
    pub available: bool,
    pub managed_installed: bool,
    pub installing: bool,
    pub target_version: String,
    pub update_available: bool,
    pub platform: Option<String>,
    pub source: Option<CodexRuntimeSource>,
    pub detected_version: Option<String>,
    pub executable: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub components: Vec<RuntimeComponentStatus>,
}
