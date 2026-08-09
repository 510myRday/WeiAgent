use std::{pin::Pin, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::Stream;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider, ThinkingLevel},
    error::AppResult,
    llm::types::{ChatCompletionOutput, StreamEvent, ToolDefinition},
};

pub type ChatStream = Pin<Box<dyn Stream<Item = AppResult<StreamEvent>> + Send>>;

/// Called before sleeping between LLM request attempts.
/// Arguments: next_attempt (1-based, the attempt that will run next), max_attempts, reason.
pub type RetryHook = Arc<dyn Fn(u32, u32, &str) + Send + Sync>;

#[derive(Clone)]
pub struct RequestOptions {
    pub timeout: Duration,
    pub retry_attempts: u32,
    pub thinking_level: Option<ThinkingLevel>,
    pub on_retry: Option<RetryHook>,
    /// Marks internal context-summary requests in the request monitor.
    pub is_compaction: bool,
}

#[async_trait]
pub trait ChatClient: Send + Sync {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatCompletionOutput>;

    async fn stream(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
        tools: Option<Vec<ToolDefinition>>,
    ) -> AppResult<ChatStream>;

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>>;
}

pub type DynChatClient = Arc<dyn ChatClient>;
