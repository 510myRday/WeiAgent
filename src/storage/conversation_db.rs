//! SQLite-backed storage for conversations and their messages.
//!
//! All functions here are synchronous connection-level helpers that run on the
//! dedicated database worker thread via `Database::call`. Message writes are
//! incremental (single-row INSERT/UPDATE).

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::sql_util::{parse_datetime, parse_uuid};
use uuid::Uuid;

use crate::{
    domain::{
        ChatMessage, ChatMessageStatus, ChatRole, ContextCheckpoint, Conversation,
        ConversationMessagePage, ConversationUsage, ModelUsage,
    },
    error::{AppError, AppResult},
};

const CONVERSATIONS_SCHEMA_VERSION: i64 = 3;
const RESPONSES_ITEMS_SCHEMA_VERSION: i64 = 4;
const CONVERSATION_TITLE_CHAR_LIMIT: usize = 80;

pub(super) const INTERRUPTED_TOOL_REASON: &str = "Tool execution was interrupted before a result was received. Do not assume it completed; retry the tool if needed.";

const CONVERSATIONS_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY NOT NULL,
    agent_id TEXT NOT NULL,
    title TEXT NOT NULL,
    message_count INTEGER NOT NULL DEFAULT 0,
    is_pinned INTEGER NOT NULL DEFAULT 0,
    context_checkpoint_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_conversations_agent
    ON conversations(agent_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS conversation_messages (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    reasoning_content TEXT,
    openai_response_items_json TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    tool_calls_json TEXT,
    tool_call_id TEXT,
    attachments_json TEXT NOT NULL DEFAULT '[]',
    usage_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_conversation_messages_conversation
    ON conversation_messages(conversation_id, seq);
";

pub(super) fn is_generated_conversation_title(title: &str) -> bool {
    matches!(title.trim(), "" | "New Conversation")
}

fn title_from_content(content: &str) -> Option<String> {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = normalized
        .chars()
        .take(CONVERSATION_TITLE_CHAR_LIMIT)
        .collect::<String>();
    (!title.is_empty()).then_some(title)
}

pub(super) fn title_from_first_user_message(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .find(|message| message.role == ChatRole::User)
        .and_then(|message| title_from_content(&message.content))
}

pub(super) fn conversation_usage(messages: &[ChatMessage]) -> ConversationUsage {
    messages
        .iter()
        .rev()
        .find_map(|message| message.usage.as_ref())
        .map(latest_usage_snapshot)
        .unwrap_or_default()
}

/// Latest model call is a snapshot of current context-window occupancy.
/// Summing every historical turn overcounts (turn1 50 + turn2 50 is still
/// ~50 tokens of context, not 100).
fn latest_usage_snapshot(usage: &ModelUsage) -> ConversationUsage {
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

pub(super) fn migrate_conversations(conn: &mut Connection) -> AppResult<()> {
    let conversations_applied = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_schema_migrations WHERE version = ?1)",
        [CONVERSATIONS_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !conversations_applied {
        let tx = conn.transaction()?;
        tx.execute_batch(CONVERSATIONS_SCHEMA_SQL)?;
        tx.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (?1, ?2, ?3)",
            params![
                CONVERSATIONS_SCHEMA_VERSION,
                "conversations_sqlite_v3",
                Utc::now().to_rfc3339()
            ],
        )?;
        tx.commit()?;
    }

    let responses_items_applied = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_schema_migrations WHERE version = ?1)",
        [RESPONSES_ITEMS_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !responses_items_applied {
        let tx = conn.transaction()?;
        let has_column = tx
            .prepare("PRAGMA table_info(conversation_messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "openai_response_items_json");
        if !has_column {
            tx.execute(
                "ALTER TABLE conversation_messages
                 ADD COLUMN openai_response_items_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        tx.execute(
            "INSERT INTO app_schema_migrations (version, name, applied_at)
             VALUES (?1, ?2, ?3)",
            params![
                RESPONSES_ITEMS_SCHEMA_VERSION,
                "responses_reasoning_items_sqlite_v4",
                Utc::now().to_rfc3339()
            ],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/*
 * Row mapping helpers.
 */

fn role_to_str(role: &ChatRole) -> &'static str {
    role.as_openai_role()
}

fn role_from_str(value: &str) -> AppResult<ChatRole> {
    match value {
        "user" => Ok(ChatRole::User),
        "assistant" => Ok(ChatRole::Assistant),
        "tool" => Ok(ChatRole::Tool),
        other => Err(AppError::Database(format!("unknown chat role: {other}"))),
    }
}

fn status_to_str(status: &ChatMessageStatus) -> &'static str {
    match status {
        ChatMessageStatus::Done => "done",
        ChatMessageStatus::Streaming => "streaming",
        ChatMessageStatus::Failed => "failed",
    }
}

fn status_from_str(value: &str) -> AppResult<ChatMessageStatus> {
    match value {
        "done" => Ok(ChatMessageStatus::Done),
        "streaming" => Ok(ChatMessageStatus::Streaming),
        "failed" => Ok(ChatMessageStatus::Failed),
        other => Err(AppError::Database(format!(
            "unknown chat message status: {other}"
        ))),
    }
}

struct ConversationRow {
    id: String,
    agent_id: String,
    title: String,
    message_count: i64,
    is_pinned: i64,
    context_checkpoint_json: Option<String>,
    created_at: String,
    updated_at: String,
}

const CONVERSATION_COLUMNS: &str = "id, agent_id, title, message_count, is_pinned,
    context_checkpoint_json, created_at, updated_at";

fn map_conversation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    Ok(ConversationRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        title: row.get(2)?,
        message_count: row.get(3)?,
        is_pinned: row.get(4)?,
        context_checkpoint_json: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn row_to_conversation(row: ConversationRow) -> AppResult<Conversation> {
    let context_checkpoint = row
        .context_checkpoint_json
        .as_deref()
        .map(serde_json::from_str::<ContextCheckpoint>)
        .transpose()?;
    Ok(Conversation {
        id: parse_uuid(&row.id, "id")?,
        agent_id: parse_uuid(&row.agent_id, "agent_id")?,
        title: row.title,
        message_count: usize::try_from(row.message_count).unwrap_or(0),
        is_pinned: row.is_pinned != 0,
        context_checkpoint,
        created_at: parse_datetime(&row.created_at, "created_at")?,
        updated_at: parse_datetime(&row.updated_at, "updated_at")?,
    })
}

fn insert_conversation(conn: &Connection, conversation: &Conversation) -> AppResult<()> {
    let checkpoint_json = conversation
        .context_checkpoint
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    conn.execute(
        "INSERT INTO conversations (
            id, agent_id, title, message_count, is_pinned,
            context_checkpoint_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            conversation.id.to_string(),
            conversation.agent_id.to_string(),
            conversation.title,
            conversation.message_count as i64,
            i64::from(conversation.is_pinned),
            checkpoint_json,
            conversation.created_at.to_rfc3339(),
            conversation.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

struct MessageRow {
    id: String,
    agent_id: String,
    conversation_id: String,
    role: String,
    content: String,
    reasoning_content: Option<String>,
    openai_response_items_json: String,
    status: String,
    created_at: String,
    tool_calls_json: Option<String>,
    tool_call_id: Option<String>,
    attachments_json: String,
    usage_json: Option<String>,
}

const MESSAGE_COLUMNS: &str = "id, agent_id, conversation_id, role, content, reasoning_content,
    openai_response_items_json, status, created_at, tool_calls_json, tool_call_id,
    attachments_json, usage_json";

fn map_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        conversation_id: row.get(2)?,
        role: row.get(3)?,
        content: row.get(4)?,
        reasoning_content: row.get(5)?,
        openai_response_items_json: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        tool_calls_json: row.get(9)?,
        tool_call_id: row.get(10)?,
        attachments_json: row.get(11)?,
        usage_json: row.get(12)?,
    })
}

fn row_to_message(row: MessageRow) -> AppResult<ChatMessage> {
    Ok(ChatMessage {
        id: parse_uuid(&row.id, "id")?,
        agent_id: parse_uuid(&row.agent_id, "agent_id")?,
        conversation_id: parse_uuid(&row.conversation_id, "conversation_id")?,
        role: role_from_str(&row.role)?,
        content: row.content,
        reasoning_content: row.reasoning_content,
        openai_response_items: serde_json::from_str(&row.openai_response_items_json)?,
        status: status_from_str(&row.status)?,
        created_at: parse_datetime(&row.created_at, "created_at")?,
        tool_calls: row
            .tool_calls_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        tool_call_id: row.tool_call_id,
        attachments: serde_json::from_str(&row.attachments_json)?,
        usage: row
            .usage_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
    })
}

struct MessageValues {
    role: &'static str,
    status: &'static str,
    openai_response_items_json: String,
    tool_calls_json: Option<String>,
    attachments_json: String,
    usage_json: Option<String>,
}

fn message_values(message: &ChatMessage) -> AppResult<MessageValues> {
    Ok(MessageValues {
        role: role_to_str(&message.role),
        status: status_to_str(&message.status),
        openai_response_items_json: serde_json::to_string(&message.openai_response_items)?,
        tool_calls_json: message
            .tool_calls
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
        attachments_json: serde_json::to_string(&message.attachments)?,
        usage_json: message
            .usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    })
}

fn insert_message(
    conn: &Connection,
    conversation_id: &Uuid,
    message: &ChatMessage,
) -> AppResult<()> {
    let values = message_values(message)?;
    conn.execute(
        "INSERT INTO conversation_messages (
            id, agent_id, conversation_id, role, content, reasoning_content,
            openai_response_items_json, status, created_at, tool_calls_json,
            tool_call_id, attachments_json, usage_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            message.id.to_string(),
            message.agent_id.to_string(),
            conversation_id.to_string(),
            values.role,
            message.content,
            message.reasoning_content,
            values.openai_response_items_json,
            values.status,
            message.created_at.to_rfc3339(),
            values.tool_calls_json,
            message.tool_call_id,
            values.attachments_json,
            values.usage_json,
        ],
    )?;
    Ok(())
}

/// Replaces the stored message in place (keeps its original ordering `seq`).
fn update_message(conn: &Connection, message: &ChatMessage) -> AppResult<()> {
    let values = message_values(message)?;
    conn.execute(
        "UPDATE conversation_messages SET
            agent_id = ?2, role = ?3, content = ?4, reasoning_content = ?5,
            openai_response_items_json = ?6, status = ?7, created_at = ?8,
            tool_calls_json = ?9, tool_call_id = ?10, attachments_json = ?11,
            usage_json = ?12
         WHERE id = ?1",
        params![
            message.id.to_string(),
            message.agent_id.to_string(),
            values.role,
            message.content,
            message.reasoning_content,
            values.openai_response_items_json,
            values.status,
            message.created_at.to_rfc3339(),
            values.tool_calls_json,
            message.tool_call_id,
            values.attachments_json,
            values.usage_json,
        ],
    )?;
    Ok(())
}

fn load_conversation(conn: &Connection, id: Uuid) -> AppResult<Conversation> {
    let row = conn
        .query_row(
            &format!("SELECT {CONVERSATION_COLUMNS} FROM conversations WHERE id = ?1"),
            [id.to_string()],
            map_conversation_row,
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    row_to_conversation(row)
}

fn load_messages(conn: &Connection, conversation_id: Uuid) -> AppResult<Vec<ChatMessage>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM conversation_messages
         WHERE conversation_id = ?1 ORDER BY seq ASC"
    ))?;
    let rows = stmt.query_map([conversation_id.to_string()], map_message_row)?;
    let mut messages = Vec::new();
    for row in rows {
        messages.push(row_to_message(row?)?);
    }
    Ok(messages)
}

fn message_count(conn: &Connection, conversation_id: Uuid) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
        [conversation_id.to_string()],
        |row| row.get(0),
    )?)
}

fn first_user_message_title(conn: &Connection, conversation_id: Uuid) -> AppResult<Option<String>> {
    let content: Option<String> = conn
        .query_row(
            "SELECT content FROM conversation_messages
             WHERE conversation_id = ?1 AND role = 'user'
             ORDER BY seq ASC LIMIT 1",
            [conversation_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(content.as_deref().and_then(title_from_content))
}

/// Refreshes count/updated_at (and the derived title when it is still a
/// generated placeholder) after any message mutation.
fn refresh_conversation_after_write(
    tx: &Transaction<'_>,
    conversation: &Conversation,
) -> AppResult<()> {
    let count = message_count(tx, conversation.id)?;
    let title = if is_generated_conversation_title(&conversation.title) {
        first_user_message_title(tx, conversation.id)?.unwrap_or_else(|| conversation.title.clone())
    } else {
        conversation.title.clone()
    };
    tx.execute(
        "UPDATE conversations SET message_count = ?2, title = ?3, updated_at = ?4 WHERE id = ?1",
        params![
            conversation.id.to_string(),
            count,
            title,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

fn clear_checkpoint(conn: &Connection, conversation_id: Uuid) -> AppResult<()> {
    conn.execute(
        "UPDATE conversations SET context_checkpoint_json = NULL WHERE id = ?1",
        [conversation_id.to_string()],
    )?;
    Ok(())
}

/*
 * Public connection-level operations.
 */

pub(super) fn list_conversations_conn(
    conn: &Connection,
    agent_id: Uuid,
) -> AppResult<Vec<Conversation>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CONVERSATION_COLUMNS} FROM conversations
         WHERE agent_id = ?1
         ORDER BY is_pinned DESC, updated_at DESC, created_at ASC, id ASC"
    ))?;
    let rows = stmt.query_map([agent_id.to_string()], map_conversation_row)?;
    let mut conversations = Vec::new();
    for row in rows {
        conversations.push(row_to_conversation(row?)?);
    }
    Ok(conversations)
}

pub(super) fn get_conversation_conn(conn: &Connection, id: Uuid) -> AppResult<Conversation> {
    load_conversation(conn, id)
}

pub(super) fn create_conversation_conn(
    conn: &Connection,
    conversation: &Conversation,
) -> AppResult<()> {
    insert_conversation(conn, conversation)
}

pub(super) fn update_conversation_conn(
    conn: &mut Connection,
    id: Uuid,
    is_pinned: bool,
    title: Option<String>,
) -> AppResult<Conversation> {
    let tx = conn.transaction()?;
    let mut conversation = load_conversation(&tx, id)?;
    conversation.is_pinned = is_pinned;
    if let Some(title) = title {
        let title = title.trim();
        if !title.is_empty() {
            conversation.title = title.to_string();
        }
    }
    tx.execute(
        "UPDATE conversations SET is_pinned = ?2, title = ?3 WHERE id = ?1",
        params![
            id.to_string(),
            i64::from(conversation.is_pinned),
            conversation.title
        ],
    )?;
    tx.commit()?;
    Ok(conversation)
}

pub(super) fn update_conversation_title_conn(
    conn: &mut Connection,
    id: Uuid,
    title: String,
) -> AppResult<Conversation> {
    let tx = conn.transaction()?;
    let mut conversation = load_conversation(&tx, id)?;
    conversation.title = title;
    conversation.updated_at = Utc::now();
    tx.execute(
        "UPDATE conversations SET title = ?2, updated_at = ?3 WHERE id = ?1",
        params![
            id.to_string(),
            conversation.title,
            conversation.updated_at.to_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(conversation)
}

pub(super) fn delete_conversation_conn(conn: &mut Connection, id: Uuid) -> AppResult<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM conversation_messages WHERE conversation_id = ?1",
        [id.to_string()],
    )?;
    let deleted = tx.execute("DELETE FROM conversations WHERE id = ?1", [id.to_string()])?;
    tx.commit()?;
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub(super) fn delete_agent_conversations_conn(
    conn: &mut Connection,
    agent_id: Uuid,
) -> AppResult<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM conversation_messages WHERE conversation_id IN (
            SELECT id FROM conversations WHERE agent_id = ?1
         )",
        [agent_id.to_string()],
    )?;
    tx.execute(
        "DELETE FROM conversations WHERE agent_id = ?1",
        [agent_id.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn read_messages_conn(
    conn: &Connection,
    conversation_id: Uuid,
) -> AppResult<Vec<ChatMessage>> {
    load_conversation(conn, conversation_id)?;
    load_messages(conn, conversation_id)
}

pub(super) fn read_message_page_conn(
    conn: &Connection,
    conversation_id: Uuid,
    before: Option<Uuid>,
    limit: usize,
) -> AppResult<ConversationMessagePage> {
    load_conversation(conn, conversation_id)?;
    let messages = load_messages(conn, conversation_id)?;
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

pub(super) fn read_usage_conn(
    conn: &Connection,
    conversation_id: Uuid,
) -> AppResult<ConversationUsage> {
    load_conversation(conn, conversation_id)?;
    let usage_json: Option<String> = conn
        .query_row(
            "SELECT usage_json FROM conversation_messages
             WHERE conversation_id = ?1 AND usage_json IS NOT NULL
             ORDER BY seq DESC LIMIT 1",
            [conversation_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(usage_json) = usage_json else {
        return Ok(ConversationUsage::default());
    };
    let usage: ModelUsage = serde_json::from_str(&usage_json)?;
    Ok(latest_usage_snapshot(&usage))
}

pub(super) fn append_messages_conn(
    conn: &mut Connection,
    conversation_id: Uuid,
    messages: &[ChatMessage],
) -> AppResult<()> {
    let tx = conn.transaction()?;
    let conversation = load_conversation(&tx, conversation_id)?;
    for message in messages {
        insert_message(&tx, &conversation_id, message)?;
    }
    refresh_conversation_after_write(&tx, &conversation)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn upsert_message_conn(
    conn: &mut Connection,
    conversation_id: Uuid,
    message: &ChatMessage,
) -> AppResult<()> {
    let tx = conn.transaction()?;
    let conversation = load_conversation(&tx, conversation_id)?;
    let existing_seq: Option<i64> = tx
        .query_row(
            "SELECT seq FROM conversation_messages WHERE id = ?1 AND conversation_id = ?2",
            params![message.id.to_string(), conversation_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(message_seq) = existing_seq {
        // Rewriting history at or before the checkpoint boundary invalidates
        // the compaction summary.
        let checkpoint_seq = conversation
            .context_checkpoint
            .as_ref()
            .map(|checkpoint| {
                tx.query_row(
                    "SELECT seq FROM conversation_messages
                     WHERE id = ?1 AND conversation_id = ?2",
                    params![
                        checkpoint.covers_until_message_id.to_string(),
                        conversation_id.to_string()
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
            })
            .transpose()?
            .flatten();
        let invalidates_checkpoint = conversation.context_checkpoint.is_some()
            && checkpoint_seq.is_none_or(|checkpoint_seq| message_seq <= checkpoint_seq);
        update_message(&tx, message)?;
        if invalidates_checkpoint {
            clear_checkpoint(&tx, conversation_id)?;
        }
    } else {
        insert_message(&tx, &conversation_id, message)?;
    }
    refresh_conversation_after_write(&tx, &conversation)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn truncate_from_message_conn(
    conn: &mut Connection,
    conversation_id: Uuid,
    message_id: Uuid,
) -> AppResult<Vec<ChatMessage>> {
    let tx = conn.transaction()?;
    let conversation = load_conversation(&tx, conversation_id)?;
    let existing = load_messages(&tx, conversation_id)?;
    let index = existing
        .iter()
        .position(|message| message.id == message_id)
        .ok_or_else(|| {
            AppError::BadRequest("restart message does not belong to this conversation".to_string())
        })?;
    if existing[index].role != ChatRole::User {
        return Err(AppError::BadRequest(
            "conversation can only restart from a user message".to_string(),
        ));
    }

    let derived_title = title_from_first_user_message(&existing);
    let title_tracks_first_message = is_generated_conversation_title(&conversation.title)
        || derived_title.as_deref() == Some(conversation.title.as_str());
    let checkpoint_survives = conversation
        .context_checkpoint
        .as_ref()
        .is_some_and(|checkpoint| {
            existing
                .iter()
                .position(|message| message.id == checkpoint.covers_until_message_id)
                .is_some_and(|checkpoint_index| checkpoint_index < index)
        });

    let remaining = &existing[..index];
    for removed in &existing[index..] {
        tx.execute(
            "DELETE FROM conversation_messages WHERE id = ?1",
            [removed.id.to_string()],
        )?;
    }
    if !checkpoint_survives {
        clear_checkpoint(&tx, conversation_id)?;
    }
    let title = if title_tracks_first_message {
        title_from_first_user_message(remaining).unwrap_or_else(|| "New Conversation".to_string())
    } else {
        conversation.title.clone()
    };
    tx.execute(
        "UPDATE conversations SET message_count = ?2, title = ?3, updated_at = ?4 WHERE id = ?1",
        params![
            conversation_id.to_string(),
            remaining.len() as i64,
            title,
            Utc::now().to_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(existing[index..].to_vec())
}

pub(super) fn fail_streaming_messages_conn(
    conn: &mut Connection,
    conversation_id: Uuid,
) -> AppResult<()> {
    let tx = conn.transaction()?;
    let conversation = load_conversation(&tx, conversation_id)?;
    let messages = load_messages(&tx, conversation_id)?;
    let mut changed = false;
    for message in &messages {
        if message.status == ChatMessageStatus::Streaming {
            tx.execute(
                "UPDATE conversation_messages SET status = 'failed' WHERE id = ?1",
                [message.id.to_string()],
            )?;
            changed = true;
        }
    }
    changed |= append_interrupted_tool_outputs(&tx, conversation_id, &messages)?;
    if changed {
        refresh_conversation_after_write(&tx, &conversation)?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn repair_incomplete_tool_calls_conn(
    conn: &mut Connection,
    conversation_id: Uuid,
) -> AppResult<()> {
    let tx = conn.transaction()?;
    let conversation = load_conversation(&tx, conversation_id)?;
    let messages = load_messages(&tx, conversation_id)?;
    if append_interrupted_tool_outputs(&tx, conversation_id, &messages)? {
        refresh_conversation_after_write(&tx, &conversation)?;
    }
    tx.commit()?;
    Ok(())
}

fn append_interrupted_tool_outputs(
    tx: &Transaction<'_>,
    conversation_id: Uuid,
    messages: &[ChatMessage],
) -> AppResult<bool> {
    let completed_call_ids = messages
        .iter()
        .filter(|message| message.role == ChatRole::Tool)
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    let mut repaired_call_ids = std::collections::HashSet::new();
    let incomplete_calls = messages
        .iter()
        .filter(|message| message.role == ChatRole::Assistant)
        .flat_map(|message| {
            message
                .tool_calls
                .as_ref()
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|call| {
                    call.get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(|id| (message.agent_id, id))
                })
        })
        .filter(|(_, call_id)| {
            !completed_call_ids.contains(call_id)
                && repaired_call_ids.insert((*call_id).to_string())
        })
        .collect::<Vec<_>>();
    let changed = !incomplete_calls.is_empty();

    for (agent_id, call_id) in incomplete_calls {
        let message = ChatMessage {
            id: Uuid::new_v4(),
            agent_id,
            conversation_id,
            role: ChatRole::Tool,
            content: serde_json::to_string(&crate::domain::ToolResult::Interrupted {
                reason: INTERRUPTED_TOOL_REASON.to_string(),
            })?,
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: Utc::now(),
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
            attachments: Vec::new(),
            usage: None,
        };
        insert_message(tx, &conversation_id, &message)?;
    }
    Ok(changed)
}

pub(super) fn write_checkpoint_conn(
    conn: &mut Connection,
    conversation_id: Uuid,
    checkpoint: &ContextCheckpoint,
) -> AppResult<()> {
    let tx = conn.transaction()?;
    load_conversation(&tx, conversation_id)?;
    tx.execute(
        "UPDATE conversations SET context_checkpoint_json = ?2, updated_at = ?3 WHERE id = ?1",
        params![
            conversation_id.to_string(),
            serde_json::to_string(checkpoint)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(())
}
