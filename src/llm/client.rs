use std::{pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_util::Stream;

use crate::{
    domain::{Agent, ChatMessage, ModelProvider},
    error::AppResult,
};

pub type ChatStream = Pin<Box<dyn Stream<Item = AppResult<String>> + Send>>;

#[async_trait]
pub trait ChatClient: Send + Sync {
    async fn complete(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
    ) -> AppResult<String>;

    async fn stream(
        &self,
        agent: &Agent,
        provider: &ModelProvider,
        messages: &[ChatMessage],
        system_prompt: &str,
    ) -> AppResult<ChatStream>;

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>>;
}

pub type DynChatClient = Arc<dyn ChatClient>;
