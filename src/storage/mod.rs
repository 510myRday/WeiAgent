mod agents;
mod conversation_db;
mod conversations;
mod db;
mod files;
mod ip_proxies;
pub mod llm_logs;
mod mcp;
mod migrations;
mod providers;
mod scheduling;
mod skills;
mod sql_util;
mod store;
mod task_plans;
mod tool_settings;
mod tool_settings_schema;
mod validation;
mod workspaces;

pub use db::Database;
pub use llm_logs::LlmLogger;
pub(crate) use scheduling::validate_heartbeat_interval;
pub use skills::{BUILTIN_SKILL_GROUP_ID, BUILTIN_WEIAGENT_CLI_SKILL_NAME, SkillGroupRecord};
pub use store::Store;

pub(super) const BUILTIN_CEO_ICON: &str = "crown";
pub(super) const BUILTIN_CEO_COLOR: &str = "#2563eb";
pub(super) const WORKSPACE_PERSONALITY: &str = "personality.md";
pub(super) const WORKSPACE_MEMORY: &str = "memory.md";
pub(super) const WORKSPACE_INSTRUCTIONS: &str = "instructions.md";

#[cfg(test)]
mod tests;
