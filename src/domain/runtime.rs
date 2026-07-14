use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRuntimeSource {
    Managed,
    Environment,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRuntimeStatus {
    pub supported: bool,
    pub available: bool,
    pub managed_installed: bool,
    pub installing: bool,
    pub target_version: String,
    pub platform: Option<String>,
    pub source: Option<SandboxRuntimeSource>,
    pub detected_version: Option<String>,
    pub executable: Option<String>,
    pub error: Option<String>,
}
