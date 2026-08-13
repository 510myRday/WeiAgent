use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillFile {
    pub path: String,
    pub kind: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillConflict {
    pub name: String,
    pub count: usize,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    pub root_dir: String,
    pub skill_md: String,
    #[serde(default)]
    pub scripts: Vec<SkillFile>,
    #[serde(default)]
    pub references: Vec<SkillFile>,
    #[serde(default)]
    pub examples: Vec<SkillFile>,
    pub content_hash: String,
    pub modified_at: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
    pub enabled: bool,
    pub global_override: Option<bool>,
    pub group_override: Option<bool>,
    pub agent_override: Option<bool>,
    pub agent_group_override: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillGroupState {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    pub skill_count: usize,
    pub enabled: bool,
    pub global_override: Option<bool>,
    pub agent_override: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SkillOverrides {
    #[serde(default)]
    pub skills: Vec<SkillOverride>,
    #[serde(default)]
    pub groups: Vec<SkillGroupOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillOverride {
    pub name: String,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillGroupOverride {
    pub group: String,
    #[serde(default)]
    pub group_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillSettings {
    pub skills_dir: String,
    pub defaults: Vec<SkillManifest>,
    pub overrides: SkillOverrides,
    pub effective: Vec<SkillManifest>,
    pub groups: Vec<SkillGroupState>,
    #[serde(default)]
    pub orphan_skills: Vec<String>,
    #[serde(default)]
    pub orphan_groups: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
    #[serde(default)]
    pub conflicts: Vec<SkillConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillImportTask {
    pub id: uuid::Uuid,
    pub source: String,
    pub kind: String,
    #[serde(default)]
    pub group_id: Option<String>,
    pub status: String,
    pub phase: String,
    pub progress: u8,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SkillAgentOverridesRequest {
    #[serde(default)]
    pub skills: Vec<SkillOverride>,
    #[serde(default)]
    pub groups: Vec<SkillGroupOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSkillGroupRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenameSkillGroupRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillGroupOrderRequest {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillGroupAssignmentRequest {
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillGroupSkillOrderRequest {
    pub skill_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillContent {
    pub name: String,
    pub path: String,
    pub root_dir: String,
    pub cwd: String,
    pub content: String,
    pub media_type: String,
    pub data_base64: Option<String>,
    pub files: Vec<SkillFile>,
    pub created_at: String,
    pub updated_at: String,
    pub scripts: Vec<SkillFile>,
    pub references: Vec<SkillFile>,
    pub examples: Vec<SkillFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillChangeEvent {
    pub version: u64,
    pub list_changed: bool,
    pub changed_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillChangeFeed {
    pub version: u64,
    pub events: Vec<SkillChangeEvent>,
    pub full_refresh: bool,
}
