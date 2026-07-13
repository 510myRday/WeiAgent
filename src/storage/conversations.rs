use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{ChatMessage, ChatRole, Conversation, CreateConversationRequest},
    error::{AppError, AppResult},
    storage::{files::write_json, json_store::JsonStore},
};

const CONVERSATION_TITLE_CHAR_LIMIT: usize = 80;

fn is_generated_conversation_title(title: &str) -> bool {
    matches!(
        title.trim(),
        "" | "New Conversation" | "New conversation" | "新会话" | "新建会话"
    )
}

fn title_from_first_user_message(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .find(|message| message.role == ChatRole::User)
        .and_then(|message| {
            let normalized = message
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let title = normalized
                .chars()
                .take(CONVERSATION_TITLE_CHAR_LIMIT)
                .collect::<String>();
            (!title.is_empty()).then_some(title)
        })
}

impl JsonStore {
    pub async fn list_conversations(&self, agent_id: Uuid) -> AppResult<Vec<Conversation>> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if !agents.iter().any(|agent| agent.id == agent_id) {
            return Err(AppError::NotFound);
        }

        let mut changed = false;
        let mut conversations = self.read_conversations_unlocked().await?;
        for conversation in conversations
            .iter_mut()
            .filter(|conversation| conversation.agent_id == agent_id)
        {
            let messages = self
                .read_conversation_messages_unchecked(conversation.id)
                .await?;
            if conversation.message_count != messages.len() {
                conversation.message_count = messages.len();
                changed = true;
            }
            if is_generated_conversation_title(&conversation.title)
                && let Some(title) = title_from_first_user_message(&messages)
                && conversation.title != title
            {
                conversation.title = title;
                changed = true;
            }
        }

        if changed {
            self.write_conversations_unlocked(&conversations).await?;
        }

        let mut conversations = conversations
            .into_iter()
            .filter(|conversation| conversation.agent_id == agent_id)
            .collect::<Vec<_>>();
        conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(conversations)
    }

    pub async fn get_conversation(&self, id: Uuid) -> AppResult<Conversation> {
        self.read_conversations()
            .await?
            .into_iter()
            .find(|conversation| conversation.id == id)
            .ok_or(AppError::NotFound)
    }

    pub async fn create_conversation(
        &self,
        agent_id: Uuid,
        request: CreateConversationRequest,
    ) -> AppResult<Conversation> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if !agents.iter().any(|agent| agent.id == agent_id) {
            return Err(AppError::NotFound);
        }
        let mut conversations = self.read_conversations_unlocked().await?;
        let now = Utc::now();
        let conversation = Conversation {
            id: Uuid::new_v4(),
            agent_id,
            title: request
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or("New Conversation")
                .to_string(),
            message_count: 0,
            created_at: now,
            updated_at: now,
        };
        conversations.push(conversation.clone());
        self.write_conversations_unlocked(&conversations).await?;
        fs::write(self.conversation_file(conversation.id), "[]").await?;
        Ok(conversation)
    }

    pub async fn delete_conversation(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let before = conversations.len();
        conversations.retain(|conversation| conversation.id != id);
        if conversations.len() == before {
            return Err(AppError::NotFound);
        }
        self.write_conversations_unlocked(&conversations).await?;
        let file = self.conversation_file(id);
        if fs::metadata(&file).await.is_ok() {
            fs::remove_file(file).await?;
        }
        Ok(())
    }

    pub async fn read_conversation_messages(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Vec<ChatMessage>> {
        self.get_conversation(conversation_id).await?;
        self.read_conversation_messages_unchecked(conversation_id)
            .await
    }

    pub async fn append_conversation_messages(
        &self,
        conversation_id: Uuid,
        messages: &[ChatMessage],
    ) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let conversation = conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            .ok_or(AppError::NotFound)?;

        let mut existing = self
            .read_conversation_messages_unchecked(conversation_id)
            .await?;
        existing.extend_from_slice(messages);
        write_json(&self.conversation_file(conversation_id), &existing).await?;
        conversation.message_count = existing.len();
        if is_generated_conversation_title(&conversation.title)
            && let Some(title) = title_from_first_user_message(&existing)
        {
            conversation.title = title;
        }
        conversation.updated_at = Utc::now();
        self.write_conversations_unlocked(&conversations).await
    }

    pub async fn upsert_conversation_message(
        &self,
        conversation_id: Uuid,
        message: ChatMessage,
    ) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let conversation = conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            .ok_or(AppError::NotFound)?;
        let mut existing = self
            .read_conversation_messages_unchecked(conversation_id)
            .await?;
        if let Some(stored) = existing.iter_mut().find(|stored| stored.id == message.id) {
            *stored = message;
        } else {
            existing.push(message);
        }
        write_json(&self.conversation_file(conversation_id), &existing).await?;
        conversation.message_count = existing.len();
        conversation.updated_at = Utc::now();
        self.write_conversations_unlocked(&conversations).await
    }
}
