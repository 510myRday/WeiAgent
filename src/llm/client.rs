use std::{pin::Pin, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::Stream;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::AppResult,
};

pub type ChatStream = Pin<Box<dyn Stream<Item = AppResult<String>> + Send>>;

#[derive(Debug, Clone, Copy)]
pub struct RequestOptions {
    pub timeout: Duration,
    pub retry_attempts: u32,
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
    ) -> AppResult<String>;

    async fn stream(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
        options: RequestOptions,
    ) -> AppResult<ChatStream>;

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>>;
}

pub type DynChatClient = Arc<dyn ChatClient>;
