use std::{collections::HashSet, path::PathBuf};

use chrono::Utc;
use tokio::fs;
use uuid::Uuid;

use crate::{
    domain::{
        ChatMessage, ContextCheckpoint, Conversation, ConversationMessagePage, ConversationUsage,
        CreateConversationRequest, UpdateConversationRequest,
    },
    error::{AppError, AppResult},
    storage::{conversation_db, store::Store},
};

impl Store {
    async fn ensure_agent_exists(&self, agent_id: Uuid) -> AppResult<()> {
        let agents = self.read_agents().await?;
        if !agents.iter().any(|agent| agent.id == agent_id) {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    pub async fn list_conversations(&self, agent_id: Uuid) -> AppResult<Vec<Conversation>> {
        self.ensure_agent_exists(agent_id).await?;
        self.database
            .call(move |conn| conversation_db::list_conversations_conn(conn, agent_id))
            .await
    }

    pub async fn list_conversation_ids(&self, agent_id: Uuid) -> AppResult<Vec<Uuid>> {
        Ok(self
            .list_conversations(agent_id)
            .await?
            .into_iter()
            .map(|conversation| conversation.id)
            .collect())
    }

    pub async fn get_conversation(&self, id: Uuid) -> AppResult<Conversation> {
        self.database
            .call(move |conn| conversation_db::get_conversation_conn(conn, id))
            .await
    }

    pub async fn create_conversation(
        &self,
        agent_id: Uuid,
        request: CreateConversationRequest,
    ) -> AppResult<Conversation> {
        self.ensure_agent_exists(agent_id).await?;
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
        let stored = conversation.clone();
        self.database
            .call(move |conn| conversation_db::create_conversation_conn(conn, &stored))
            .await?;
        Ok(conversation)
    }

    pub async fn update_conversation(
        &self,
        id: Uuid,
        request: UpdateConversationRequest,
    ) -> AppResult<Conversation> {
        self.database
            .call(move |conn| {
                conversation_db::update_conversation_conn(
                    conn,
                    id,
                    request.is_pinned,
                    request.title,
                )
            })
            .await
    }

    pub async fn update_conversation_title(
        &self,
        id: Uuid,
        title: String,
    ) -> AppResult<Conversation> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(AppError::BadRequest(
                "conversation title cannot be empty".to_string(),
            ));
        }
        self.database
            .call(move |conn| conversation_db::update_conversation_title_conn(conn, id, title))
            .await
    }

    pub async fn delete_conversation(&self, id: Uuid) -> AppResult<()> {
        let removed = self
            .database
            .call(move |conn| conversation_db::delete_conversation_conn(conn, id))
            .await?;
        self.cleanup_removed_generated_attachments(id, &removed)
            .await;
        Ok(())
    }

    pub async fn read_conversation_messages(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<Vec<ChatMessage>> {
        self.database
            .call(move |conn| conversation_db::read_messages_conn(conn, conversation_id))
            .await
    }

    pub async fn read_conversation_message_page(
        &self,
        conversation_id: Uuid,
        before: Option<Uuid>,
        limit: usize,
    ) -> AppResult<ConversationMessagePage> {
        self.database
            .call(move |conn| {
                conversation_db::read_message_page_conn(conn, conversation_id, before, limit)
            })
            .await
    }

    pub async fn read_conversation_usage(
        &self,
        conversation_id: Uuid,
    ) -> AppResult<ConversationUsage> {
        self.database
            .call(move |conn| conversation_db::read_usage_conn(conn, conversation_id))
            .await
    }

    pub async fn append_conversation_messages(
        &self,
        conversation_id: Uuid,
        messages: &[ChatMessage],
    ) -> AppResult<()> {
        let messages = messages.to_vec();
        self.database
            .call(move |conn| {
                conversation_db::append_messages_conn(conn, conversation_id, &messages)
            })
            .await
    }

    pub async fn truncate_conversation_from_message(
        &self,
        conversation_id: Uuid,
        message_id: Uuid,
    ) -> AppResult<()> {
        let removed = self
            .database
            .call(move |conn| {
                conversation_db::truncate_from_message_conn(conn, conversation_id, message_id)
            })
            .await?;
        self.cleanup_removed_generated_attachments(conversation_id, &removed)
            .await;
        Ok(())
    }

    pub(crate) async fn cleanup_removed_generated_attachments(
        &self,
        conversation_id: Uuid,
        removed_messages: &[ChatMessage],
    ) {
        let generated_dir = self
            .attachments_dir
            .join("generated")
            .join(conversation_id.to_string());
        let generated_dir = match fs::canonicalize(&generated_dir).await {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(
                    conversation_id = %conversation_id,
                    error = %error,
                    "failed to locate generated image directory for cleanup"
                );
                return;
            }
        };

        let retained_paths: HashSet<String> =
            match self.read_conversation_messages(conversation_id).await {
                Ok(messages) => messages
                    .iter()
                    .flat_map(|message| message.attachments.iter())
                    .filter_map(|attachment| match &attachment.source {
                        crate::domain::ImageSource::Local { path } => {
                            Some(crate::fsutil::path_identity_key(&PathBuf::from(path)))
                        }
                        _ => None,
                    })
                    .collect(),
                Err(AppError::NotFound) => HashSet::new(),
                Err(error) => {
                    tracing::warn!(
                        conversation_id = %conversation_id,
                        error = %error,
                        "failed to inspect remaining messages before generated image cleanup"
                    );
                    return;
                }
            };

        let mut removed_paths = HashSet::new();
        for attachment in removed_messages
            .iter()
            .flat_map(|message| message.attachments.iter())
        {
            let crate::domain::ImageSource::Local { path } = &attachment.source else {
                continue;
            };
            let candidate = PathBuf::from(path);
            let resolved = match fs::canonicalize(&candidate).await {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(
                        conversation_id = %conversation_id,
                        path = %candidate.display(),
                        error = %error,
                        "failed to resolve generated image for cleanup"
                    );
                    continue;
                }
            };
            if !crate::fsutil::path_is_within(&resolved, &generated_dir)
                || crate::fsutil::paths_equal(&resolved, &generated_dir)
            {
                continue;
            }
            let key = crate::fsutil::path_identity_key(&resolved);
            if retained_paths.contains(&key) || !removed_paths.insert(key) {
                continue;
            }
            if let Err(error) = fs::remove_file(&resolved).await {
                tracing::warn!(
                    conversation_id = %conversation_id,
                    path = %resolved.display(),
                    error = %error,
                    "failed to remove generated image after conversation restart"
                );
            }
        }
        if let Err(error) = fs::remove_dir(&generated_dir).await
            && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                conversation_id = %conversation_id,
                path = %generated_dir.display(),
                error = %error,
                "failed to remove empty generated image directory"
            );
        }
    }

    pub async fn upsert_conversation_message(
        &self,
        conversation_id: Uuid,
        message: ChatMessage,
    ) -> AppResult<()> {
        self.database
            .call(move |conn| conversation_db::upsert_message_conn(conn, conversation_id, &message))
            .await
    }

    pub async fn fail_streaming_messages(&self, conversation_id: Uuid) -> AppResult<()> {
        self.database
            .call(move |conn| conversation_db::fail_streaming_messages_conn(conn, conversation_id))
            .await
    }

    pub async fn repair_incomplete_tool_calls(&self, conversation_id: Uuid) -> AppResult<()> {
        self.database
            .call(move |conn| {
                conversation_db::repair_incomplete_tool_calls_conn(conn, conversation_id)
            })
            .await
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
        let stored = checkpoint.clone();
        self.database
            .call(move |conn| {
                conversation_db::write_checkpoint_conn(conn, conversation_id, &stored)
            })
            .await?;
        Ok(checkpoint)
    }
}
