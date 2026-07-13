use std::sync::Arc;

use crate::{
    app::run_manager::RunManager,
    llm::registry::DynLlmRegistry,
    storage::{JsonStore, LlmLogger},
    tools::{approval::ApprovalManager, process_manager::ProcessManager, registry::ToolRegistry},
};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<JsonStore>,
    pub llm_registry: DynLlmRegistry,
    pub llm_logger: Arc<LlmLogger>,
    pub process_manager: Arc<ProcessManager>,
    pub approval_manager: Arc<ApprovalManager>,
    pub tool_registry: Arc<ToolRegistry>,
    pub run_manager: Arc<RunManager>,
}
