use std::sync::Arc;

use crate::{
    app::{run_manager::RunManager, security::ApiSecurity},
    llm::registry::DynLlmRegistry,
    storage::{LlmLogger, Store},
    tools::{
        approval::ApprovalManager, registry::ToolRegistry, runtime::EnvironmentRuntimeManager,
        terminal::TerminalManager,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub api_security: Arc<ApiSecurity>,
    pub store: Arc<Store>,
    pub llm_registry: DynLlmRegistry,
    pub llm_logger: Arc<LlmLogger>,
    pub terminal_manager: Arc<TerminalManager>,
    pub approval_manager: Arc<ApprovalManager>,
    pub environment_runtime: Arc<EnvironmentRuntimeManager>,
    pub tool_registry: Arc<ToolRegistry>,
    pub run_manager: Arc<RunManager>,
}
