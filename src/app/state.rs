use std::sync::Arc;

use crate::{
    llm::registry::DynLlmRegistry,
    storage::{JsonStore, LlmLogger},
};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<JsonStore>,
    pub llm_registry: DynLlmRegistry,
    pub llm_logger: Arc<LlmLogger>,
}
