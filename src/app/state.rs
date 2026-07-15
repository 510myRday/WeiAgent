use std::sync::Arc;

use crate::{
    app::run_manager::RunManager,
    llm::registry::DynLlmRegistry,
    storage::{JsonStore, LlmLogger},
    tools::{
        approval::ApprovalManager, registry::ToolRegistry, runtime::CodexRuntimeManager,
        terminal::TerminalManager,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<JsonStore>,
    pub llm_registry: DynLlmRegistry,
    pub llm_logger: Arc<LlmLogger>,
    pub terminal_manager: Arc<TerminalManager>,
    pub approval_manager: Arc<ApprovalManager>,
    pub codex_runtime: Arc<CodexRuntimeManager>,
    pub tool_registry: Arc<ToolRegistry>,
    pub run_manager: Arc<RunManager>,
}
