use std::collections::HashSet;

use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{
        ChatMessage, ChatRole, ContextCheckpoint, Conversation, ConversationMessagePage,
        ConversationUsage, CreateConversationRequest, UpdateConversationRequest,
    },
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

        let mut conversations = self
            .read_conversations_unlocked()
            .await?
            .into_iter()
            .filter(|conversation| conversation.agent_id == agent_id)
            .collect::<Vec<_>>();
        conversations.sort_by_key(|item| {
            (
                std::cmp::Reverse(item.is_pinned),
                std::cmp::Reverse(item.updated_at),
            )
        });
        Ok(conversations)
    }

    pub async fn list_conversation_ids(&self, agent_id: Uuid) -> AppResult<Vec<Uuid>> {
        let _guard = self.lock.lock().await;
        let agents = self.read_agents_unlocked().await?;
        if !agents.iter().any(|agent| agent.id == agent_id) {
            return Err(AppError::NotFound);
        }
        Ok(self
            .read_conversations_unlocked()
            .await?
            .into_iter()
            .filter(|conversation| conversation.agent_id == agent_id)
            .map(|conversation| conversation.id)
            .collect())
    }

    pub(super) async fn repair_conversation_metadata(&self) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut changed = false;
        let mut conversations = self.read_conversations_unlocked().await?;
        for conversation in &mut conversations {
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
        Ok(())
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
            is_pinned: false,
            context_checkpoint: None,
            created_at: now,
            updated_at: now,
        };
        conversations.push(conversation.clone());
        self.write_conversations_unlocked(&conversations).await?;
        fs::write(self.conversation_file(conversation.id), "[]").await?;
        Ok(conversation)
    }

    pub async fn update_conversation(
        &self,
        id: Uuid,
        request: UpdateConversationRequest,
    ) -> AppResult<Conversation> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let conversation = conversations
            .iter_mut()
            .find(|conversation| conversation.id == id)
            .ok_or(AppError::NotFound)?;
        conversation.is_pinned = request.is_pinned;
        let updated = conversation.clone();
        self.write_conversations_unlocked(&conversations).await?;
        Ok(updated)
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

    pub async fn read_conversation_message_page(
        &self,
        conversation_id: Uuid,
        before: Option<Uuid>,
        limit: usize,
    ) -> AppResult<ConversationMessagePage> {
        self.get_conversation(conversation_id).await?;
        let messages = self
            .read_conversation_messages_unchecked(conversation_id)
            .await?;
        let end = match before {
            Some(cursor) => messages
                .iter()
                .position(|message| message.id == cursor)
                .ok_or_else(|| AppError::BadRequest("message cursor was not found".to_string()))?,
            None => messages.len(),
        };
        let mut start = end.saturating_sub(limit);
        while start > 0 && messages[start].role != ChatRole::User {
            start -= 1;
        }
        let has_more = start > 0;
        let usage = conversation_usage(&messages);
        Ok(ConversationMessagePage {
            messages: messages[start..end].to_vec(),
            next_cursor: has_more.then(|| messages[start].id),
            has_more,
            usage,
        })
    }

    pub async fn read_conversation_usage(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<ConversationUsage> {
        self.get_conversation(conversation_id).await?;
        let messages = self
            .read_conversation_messages_unchecked(conversation_id)
            .await?;
        Ok(conversation_usage(&messages))
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

    pub async fn truncate_conversation_from_message(
        &self,
        conversation_id: Uuid,
        message_id: Uuid,
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
        let index = existing
            .iter()
            .position(|message| message.id == message_id)
            .ok_or_else(|| {
                AppError::BadRequest(
                    "restart message does not belong to this conversation".to_string(),
                )
            })?;
        if existing[index].role != ChatRole::User {
            return Err(AppError::BadRequest(
                "conversation can only restart from a user message".to_string(),
            ));
        }

        let derived_title = title_from_first_user_message(&existing);
        let title_tracks_first_message = is_generated_conversation_title(&conversation.title)
            || derived_title.as_deref() == Some(conversation.title.as_str());
        let checkpoint_survives =
            conversation
                .context_checkpoint
                .as_ref()
                .is_some_and(|checkpoint| {
                    existing
                        .iter()
                        .position(|message| message.id == checkpoint.covers_until_message_id)
                        .is_some_and(|checkpoint_index| checkpoint_index < index)
                });
        existing.truncate(index);
        write_json(&self.conversation_file(conversation_id), &existing).await?;

        conversation.message_count = existing.len();
        if !checkpoint_survives {
            conversation.context_checkpoint = None;
        }
        if title_tracks_first_message {
            conversation.title = title_from_first_user_message(&existing)
                .unwrap_or_else(|| "New Conversation".to_string());
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
        if let Some(message_index) = existing.iter().position(|stored| stored.id == message.id) {
            let invalidates_checkpoint = conversation
                .context_checkpoint
                .as_ref()
                .and_then(|checkpoint| {
                    existing
                        .iter()
                        .position(|stored| stored.id == checkpoint.covers_until_message_id)
                })
                .is_none_or(|checkpoint_index| message_index <= checkpoint_index);
            existing[message_index] = message;
            if invalidates_checkpoint {
                conversation.context_checkpoint = None;
            }
        } else {
            existing.push(message);
        }
        write_json(&self.conversation_file(conversation_id), &existing).await?;
        conversation.message_count = existing.len();
        conversation.updated_at = Utc::now();
        self.write_conversations_unlocked(&conversations).await
    }

    pub async fn fail_streaming_messages(&self, conversation_id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let conversation = conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            .ok_or(AppError::NotFound)?;
        let mut messages = self
            .read_conversation_messages_unchecked(conversation_id)
            .await?;
        let mut changed = false;
        for message in &mut messages {
            if message.status == crate::domain::ChatMessageStatus::Streaming {
                message.status = crate::domain::ChatMessageStatus::Failed;
                changed = true;
            }
        }
        let completed_call_ids = messages
            .iter()
            .filter(|message| message.role == ChatRole::Tool)
            .filter_map(|message| message.tool_call_id.clone())
            .collect::<HashSet<_>>();
        let incomplete_calls = messages
            .iter()
            .filter(|message| message.role == ChatRole::Assistant)
            .flat_map(|message| {
                let agent_id = message.agent_id;
                message
                    .tool_calls
                    .as_ref()
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(move |call| {
                        call.get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(|id| (agent_id, id.to_string()))
                    })
            })
            .filter(|(_, call_id)| !completed_call_ids.contains(call_id))
            .collect::<Vec<_>>();
        for (agent_id, call_id) in incomplete_calls {
            messages.push(ChatMessage {
                id: Uuid::new_v4(),
                agent_id,
                conversation_id,
                role: ChatRole::Tool,
                content: serde_json::json!({
                    "status": "rejected",
                    "reason": "Run cancelled"
                })
                .to_string(),
                reasoning_content: None,
                status: crate::domain::ChatMessageStatus::Done,
                created_at: Utc::now(),
                tool_calls: None,
                tool_call_id: Some(call_id),
                attachments: Vec::new(),
                usage: None,
            });
            changed = true;
        }
        if changed {
            write_json(&self.conversation_file(conversation_id), &messages).await?;
            conversation.message_count = messages.len();
            conversation.updated_at = Utc::now();
            self.write_conversations_unlocked(&conversations).await?;
        }
        Ok(())
    }
    pub async fn read_context_checkpoint(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Option<ContextCheckpoint>> {
        Ok(self
            .get_conversation(conversation_id)
            .await?
            .context_checkpoint)
    }

    pub async fn write_context_checkpoint(
        &self,
        conversation_id: Uuid,
        checkpoint: ContextCheckpoint,
    ) -> AppResult<ContextCheckpoint> {
        let _guard = self.lock.lock().await;
        let mut conversations = self.read_conversations_unlocked().await?;
        let conversation = conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            .ok_or(AppError::NotFound)?;
        conversation.context_checkpoint = Some(checkpoint.clone());
        conversation.updated_at = Utc::now();
        self.write_conversations_unlocked(&conversations).await?;
        Ok(checkpoint)
    }
}

fn conversation_usage(messages: &[ChatMessage]) -> ConversationUsage {
    // Latest model call is a snapshot of current context-window occupancy.
    // Summing every historical turn overcounts (turn1 50 + turn2 50 is still
    // ~50 tokens of context, not 100).
    let Some(usage) = messages
        .iter()
        .rev()
        .find_map(|message| message.usage.as_ref())
    else {
        return ConversationUsage::default();
    };

    // The provider-normalized prompt total includes uncached, cache-read, and
    // cache-write prompt tokens. Add the latest completion because it becomes
    // part of the next request's history.
    let prompt_tokens = if usage.tokens.total_prompt_tokens > 0 {
        usage.tokens.total_prompt_tokens
    } else {
        usage
            .tokens
            .input_tokens
            .saturating_add(usage.tokens.cache_read_tokens)
            .saturating_add(usage.tokens.cache_write_tokens)
    };
    let context_tokens = prompt_tokens.saturating_add(usage.tokens.output_tokens);

    ConversationUsage {
        tokens: usage.tokens.clone(),
        total_tokens: context_tokens,
        cost_usd: usage.cost_usd,
    }
}
