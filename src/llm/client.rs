use std::{pin::Pin, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::Stream;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider, ThinkingLevel},
    error::AppResult,
    llm::types::{ChatCompletionOutput, StreamEvent, ToolDefinition},
};

pub type ChatStream = Pin<Box<dyn Stream<Item = AppResult<StreamEvent>> + Send>>;

/// Called before sleeping between LLM request retries.
/// Arguments: retry_number (1-based), max_retries, reason.
pub type RetryHook = Arc<dyn Fn(u32, u32, &str) + Send + Sync>;

#[derive(Clone)]
pub struct RequestOptions {
    pub timeout: Duration,
    /// Number of retries after the initial request. A value of 5 allows up to
    /// 6 total HTTP requests in one model turn.
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
