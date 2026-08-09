use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    storage::Database,
};

const MAX_LOG_FIELD_BYTES: usize = 256 * 1024;
const MAX_LOG_ERROR_BYTES: usize = 64 * 1024;
const MAX_RETAINED_LOGS: usize = 20_000;
const MAX_RETAINED_DAYS: usize = 30;
const MAX_DATABASE_BYTES: u64 = 500 * 1024 * 1024;
const MAX_LIST_LIMIT: usize = 500;
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LlmLogMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl LlmLogMetrics {
    pub fn timed(started: Instant) -> Self {
        Self {
            duration_ms: Some(elapsed_ms(started)),
            ..Self::default()
        }
    }

    pub fn with_first_token(mut self, first_token_ms: Option<u64>) -> Self {
        self.first_token_ms = first_token_ms;
        self
    }

    pub fn merge_missing(&mut self, other: &Self) {
        if self.first_token_ms.is_none() {
            self.first_token_ms = other.first_token_ms;
        }
        if self.duration_ms.is_none() {
            self.duration_ms = other.duration_ms;
        }
        if self.input_tokens.is_none() {
            self.input_tokens = other.input_tokens;
        }
        if self.output_tokens.is_none() {
            self.output_tokens = other.output_tokens;
        }
        if self.cache_write_tokens.is_none() {
            self.cache_write_tokens = other.cache_write_tokens;
        }
        if self.cache_read_tokens.is_none() {
            self.cache_read_tokens = other.cache_read_tokens;
        }
        if self.cost_usd.is_none() {
            self.cost_usd = other.cost_usd;
        }
    }
}

pub fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmLogEntry {
    pub id: String,
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub model_id: String,
    pub provider_name: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether request/response payload was persisted and can be inspected.
    pub has_payload: bool,
    pub is_compaction: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmLogDetail {
    #[serde(flatten)]
    pub summary: LlmLogEntry,
    pub raw_request: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmLogAgentStat {
    pub agent_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub agent_name: String,
    pub request_count: usize,
    pub latest_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmLogModelStat {
    pub model_key: String,
    pub provider_name: String,
    pub model_id: String,
    pub request_count: usize,
    pub latest_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct LlmLogListFilter {
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub provider_name: Option<String>,
    pub model_id: Option<String>,
    pub q: Option<String>,
    /// Inclusive lower bound as RFC3339 / ISO-8601 datetime string.
    pub since: Option<String>,
    /// Exclusive upper bound as RFC3339 / ISO-8601 datetime string.
    pub until: Option<String>,
    /// Optional request outcome: `success` or `error`.
    pub status: Option<String>,
    pub is_compaction: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LlmLogKindSummary {
    pub all_count: usize,
    pub compaction_count: usize,
    pub regular_count: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LlmLogSummary {
    pub request_count: usize,
    pub success_count: usize,
    pub error_count: usize,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmLogListPage {
    pub items: Vec<LlmLogEntry>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub agent_stats: Vec<LlmLogAgentStat>,
    pub model_stats: Vec<LlmLogModelStat>,
    pub summary: LlmLogSummary,
    pub kind_summary: LlmLogKindSummary,
}

fn validate_log_id(log_id: &str) -> AppResult<Uuid> {
    Uuid::parse_str(log_id).map_err(|_| AppError::BadRequest("invalid log id".to_string()))
}

fn now_parts() -> (String, i64) {
    let now = Utc::now();
    (now.to_rfc3339(), now.timestamp_millis())
}

pub struct LlmLogger {
    database: Arc<Database>,
    pub enabled: Arc<AtomicBool>,
    writes_since_cleanup: AtomicUsize,
    pending_writes: AtomicUsize,
    failed_writes: AtomicUsize,
    pending_notify: tokio::sync::Notify,
}

impl LlmLogger {
    #[cfg(test)]
    pub fn new(data_dir: &Path, enabled: bool) -> AppResult<Self> {
        Self::with_database(Arc::new(Database::open(data_dir)?), enabled)
    }

    pub fn with_database(database: Arc<Database>, enabled: bool) -> AppResult<Self> {
        Ok(Self {
            database,
            enabled: Arc::new(AtomicBool::new(enabled)),
            writes_since_cleanup: AtomicUsize::new(0),
            pending_writes: AtomicUsize::new(0),
            failed_writes: AtomicUsize::new(0),
            pending_notify: tokio::sync::Notify::new(),
        })
    }

    #[cfg(test)]
    pub fn db_path(&self) -> &Path {
        self.database.path()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, value: bool) {
        self.enabled.store(value, Ordering::Relaxed);
    }

    pub fn begin_pending_write(&self) {
        self.pending_writes.fetch_add(1, Ordering::AcqRel);
    }

    pub fn finish_pending_write(&self, result: &AppResult<String>) {
        if let Err(error) = result {
            self.failed_writes.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%error, "failed to persist LLM log");
        }
        if self.pending_writes.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.pending_notify.notify_one();
        }
    }

    pub fn failed_write_count(&self) -> usize {
        self.failed_writes.load(Ordering::Relaxed)
    }

    pub async fn shutdown(&self) -> AppResult<()> {
        while self.pending_writes.load(Ordering::Acquire) > 0 {
            self.pending_notify.notified().await;
        }
        self.database
            .call(|conn| {
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                cleanup_logs_conn(conn)?;
                Ok(())
            })
            .await
    }

    #[cfg(test)]
    pub(crate) async fn with_conn_for_test<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> AppResult<T> + Send + 'static,
    {
        self.database.call(move |conn| f(conn)).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_log(
        &self,
        agent_name: &str,
        agent_id: Option<&str>,
        conversation_id: Option<&str>,
        is_compaction: bool,
        model_id: &str,
        provider_name: &str,
        raw_request: &str,
        raw_response: Option<&str>,
        raw_status: Option<u16>,
        error: Option<&str>,
        metrics: LlmLogMetrics,
        include_payload: bool,
    ) -> AppResult<String> {
        let agent_name = agent_name.to_string();
        let agent_id = agent_id.map(str::to_string);
        let conversation_id = conversation_id.map(str::to_string);
        let is_compaction = is_compaction;
        let model_id = model_id.to_string();
        let provider_name = provider_name.to_string();
        let raw_request = raw_request.to_string();
        let raw_response = raw_response.map(str::to_string);
        let error = error.map(str::to_string);

        let id = self
            .database
            .call(move |conn| {
                write_log_conn(
                    conn,
                    &agent_name,
                    agent_id.as_deref(),
                    conversation_id.as_deref(),
                    is_compaction,
                    &model_id,
                    &provider_name,
                    &raw_request,
                    raw_response.as_deref(),
                    raw_status,
                    error.as_deref(),
                    metrics,
                    include_payload,
                )
            })
            .await?;

        let scheduled_cleanup =
            (self.writes_since_cleanup.fetch_add(1, Ordering::Relaxed) + 1).is_multiple_of(32);
        let over_size_limit =
            database_files_bytes_for_path(self.database.path()) > MAX_DATABASE_BYTES;
        if (scheduled_cleanup || over_size_limit)
            && let Err(error) = self.database.call(cleanup_logs_conn).await
        {
            tracing::warn!(%error, "failed to clean LLM logs");
        }
        Ok(id)
    }

    #[cfg(test)]
    pub async fn list_logs(&self, offset: usize, limit: usize) -> AppResult<Vec<LlmLogEntry>> {
        Ok(self
            .list_logs_page(offset, limit, LlmLogListFilter::default())
            .await?
            .items)
    }

    pub async fn list_logs_page(
        &self,
        offset: usize,
        limit: usize,
        filter: LlmLogListFilter,
    ) -> AppResult<LlmLogListPage> {
        self.database
            .call(move |conn| {
                cleanup_logs_conn(conn)?;
                list_logs_page_conn(conn, offset, limit, filter)
            })
            .await
    }
}

fn list_logs_page_conn(
    conn: &Connection,
    offset: usize,
    limit: usize,
    filter: LlmLogListFilter,
) -> AppResult<LlmLogListPage> {
    let limit = limit.clamp(1, MAX_LIST_LIMIT);
    let (where_sql, bind) = build_filter_clause(&filter);

    let total: i64 = {
        let sql = format!("SELECT COUNT(*) FROM llm_requests {where_sql}");
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_row(rusqlite::params_from_iter(bind.iter()), |row| row.get(0))?
    };

    let summary = {
        let sql = format!(
                    "SELECT
                        COUNT(*) as request_count,
                        SUM(CASE
                            WHEN status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                            THEN 1 ELSE 0 END) as success_count,
                        SUM(CASE
                            WHEN (status_code IS NOT NULL AND status_code >= 400)
                              OR (error IS NOT NULL AND error != '')
                            THEN 1 ELSE 0 END) as error_count,
                        COALESCE(SUM(CASE
                            WHEN status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                            THEN input_tokens ELSE 0 END), 0) as input_tokens,
                        COALESCE(SUM(CASE
                            WHEN status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                            THEN output_tokens ELSE 0 END), 0) as output_tokens,
                        COALESCE(SUM(CASE
                            WHEN status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                            THEN cache_read_tokens ELSE 0 END), 0) as cache_read_tokens,
                        COALESCE(SUM(CASE
                            WHEN status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                            THEN cache_write_tokens ELSE 0 END), 0) as cache_write_tokens,
                        COALESCE(SUM(CASE
                            WHEN status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                            THEN cost_usd ELSE 0 END), 0) as cost_usd
                     FROM llm_requests
                     {where_sql}"
                );
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_row(rusqlite::params_from_iter(bind.iter()), |row| {
            let request_count: i64 = row.get(0)?;
            let success_count: i64 = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
            let error_count: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            Ok(LlmLogSummary {
                request_count: request_count.max(0) as usize,
                success_count: success_count.max(0) as usize,
                error_count: error_count.max(0) as usize,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                cache_write_tokens: row.get(6)?,
                cost_usd: row.get(7)?,
            })
        })?
    };

    let mut kind_filter = filter.clone();
    kind_filter.is_compaction = None;
    let (kind_where, kind_bind) = build_filter_clause(&kind_filter);
    let kind_summary = {
        let sql = format!(
            "SELECT COUNT(*),
                    SUM(CASE WHEN is_compaction = 1 THEN 1 ELSE 0 END)
             FROM llm_requests {kind_where}"
        );
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_row(rusqlite::params_from_iter(kind_bind.iter()), |row| {
            let all_count: i64 = row.get(0)?;
            let compaction_count: i64 = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
            Ok(LlmLogKindSummary {
                all_count: all_count.max(0) as usize,
                compaction_count: compaction_count.max(0) as usize,
                regular_count: all_count.saturating_sub(compaction_count).max(0) as usize,
            })
        })?
    };

    let list_sql = format!(
        "SELECT id, agent_name, agent_id, conversation_id, model_id, provider_name,
                        created_at, status_code, first_token_ms, duration_ms,
                        input_tokens, output_tokens, cache_write_tokens, cache_read_tokens,
                        cost_usd, error, has_payload, is_compaction
                 FROM llm_requests
                 {where_sql}
                 ORDER BY created_at_ms DESC, id DESC
                 LIMIT ? OFFSET ?"
    );
    let mut list_bind = bind.clone();
    list_bind.push(limit.to_string());
    list_bind.push(offset.to_string());

    let mut stmt = conn.prepare(&list_sql)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(list_bind.iter()), |row| {
            row_to_entry(row)
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let stats_filter = LlmLogListFilter {
        agent_id: None,
        agent_name: None,
        provider_name: None,
        model_id: None,
        q: filter.q.clone(),
        since: filter.since.clone(),
        until: filter.until.clone(),
        status: None,
        is_compaction: filter.is_compaction,
    };
    let (stats_where, stats_bind) = build_filter_clause(&stats_filter);
    let agent_stats_sql = format!(
        "SELECT
                    agent_id,
                    agent_name,
                    COUNT(*) as request_count,
                    MAX(created_at_ms) as latest_ms,
                    MAX(created_at) as latest_at
                 FROM llm_requests
                 {stats_where}
                 GROUP BY
                    CASE
                        WHEN agent_id IS NOT NULL AND agent_id != '' THEN 'id:' || agent_id
                        ELSE 'name:' || agent_name
                    END
                 ORDER BY latest_ms DESC"
    );
    let mut agent_stats_stmt = conn.prepare(&agent_stats_sql)?;
    let agent_stats = agent_stats_stmt
        .query_map(rusqlite::params_from_iter(stats_bind.iter()), |row| {
            let agent_id: Option<String> = row.get(0)?;
            let agent_name: String = row.get(1)?;
            let request_count: i64 = row.get(2)?;
            let latest_at: String = row.get(4)?;
            let agent_key = match agent_id.as_deref().filter(|v| !v.is_empty()) {
                Some(id) => format!("id:{id}"),
                None => format!("name:{agent_name}"),
            };
            Ok(LlmLogAgentStat {
                agent_key,
                agent_id,
                agent_name,
                request_count: request_count as usize,
                latest_at,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let model_stats_sql = format!(
        "SELECT
                    provider_name,
                    model_id,
                    COUNT(*) as request_count,
                    MAX(created_at_ms) as latest_ms,
                    MAX(created_at) as latest_at
                 FROM llm_requests
                 {stats_where}
                 GROUP BY provider_name, model_id
                 ORDER BY latest_ms DESC"
    );
    let mut model_stats_stmt = conn.prepare(&model_stats_sql)?;
    let model_stats = model_stats_stmt
        .query_map(rusqlite::params_from_iter(stats_bind.iter()), |row| {
            let provider_name: String = row.get(0)?;
            let model_id: String = row.get(1)?;
            let request_count: i64 = row.get(2)?;
            let latest_at: String = row.get(4)?;
            let model_key = format!("{provider_name}::{model_id}");
            Ok(LlmLogModelStat {
                model_key,
                provider_name,
                model_id,
                request_count: request_count as usize,
                latest_at,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(LlmLogListPage {
        items,
        total: total as usize,
        offset,
        limit,
        agent_stats,
        model_stats,
        summary,
        kind_summary,
    })
}

impl LlmLogger {
    pub async fn get_log(&self, log_id: &str) -> AppResult<LlmLogDetail> {
        let log_id = log_id.to_string();
        self.database
            .call(move |conn| get_log_conn(conn, &log_id))
            .await
    }

    #[cfg(test)]
    pub(crate) async fn cleanup_logs_for_test(&self) -> AppResult<()> {
        self.database.call(cleanup_logs_conn).await
    }
}

fn get_log_conn(conn: &Connection, log_id: &str) -> AppResult<LlmLogDetail> {
    let _ = validate_log_id(log_id)?;
    let mut stmt = conn.prepare(
        "SELECT r.id, r.agent_name, r.agent_id, r.conversation_id, r.model_id, r.provider_name,
                        r.created_at, r.status_code, r.first_token_ms, r.duration_ms,
                        r.input_tokens, r.output_tokens, r.cache_write_tokens, r.cache_read_tokens,
                        r.cost_usd, r.error, r.has_payload, r.is_compaction,
                        p.raw_request, p.raw_response
                 FROM llm_requests r
                 LEFT JOIN llm_payloads p ON p.request_id = r.id
                 WHERE r.id = ?1",
    )?;
    let detail = stmt
        .query_row(params![log_id], |row| {
            let mut entry = row_to_entry(row)?;
            let raw_request: Option<String> = row.get(18)?;
            let raw_response: Option<String> = row.get(19)?;
            let raw_request = raw_request.unwrap_or_default();
            let has_saved = !raw_request.trim().is_empty()
                || raw_response
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty());
            entry.has_payload = has_saved;
            Ok(LlmLogDetail {
                summary: entry,
                raw_request,
                raw_response,
            })
        })
        .optional()?
        .ok_or(AppError::NotFound)?;
    Ok(detail)
}

#[allow(clippy::too_many_arguments)]
fn write_log_conn(
    conn: &mut Connection,
    agent_name: &str,
    agent_id: Option<&str>,
    conversation_id: Option<&str>,
    is_compaction: bool,
    model_id: &str,
    provider_name: &str,
    raw_request: &str,
    raw_response: Option<&str>,
    raw_status: Option<u16>,
    error: Option<&str>,
    mut metrics: LlmLogMetrics,
    include_payload: bool,
) -> AppResult<String> {
    let finalized_response = raw_response.map(finalize_response_for_log);
    metrics.merge_missing(&extract_metrics_from_raw(
        finalized_response.as_deref().or(raw_response),
    ));

    let (stored_request, stored_response) = if include_payload {
        (
            truncate_log_field(raw_request),
            finalized_response.map(|value| truncate_log_field(&value)),
        )
    } else {
        (String::new(), None)
    };
    let mut has_payload = include_payload
        && (!stored_request.trim().is_empty()
            || stored_response
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()));
    let stored_error = error.map(|value| truncate_log_field_to(value, MAX_LOG_ERROR_BYTES));
    let estimated_bytes = stored_request
        .len()
        .saturating_add(stored_response.as_deref().map(str::len).unwrap_or(0))
        .saturating_add(stored_error.as_deref().map(str::len).unwrap_or(0))
        .saturating_add(8 * 1024) as u64;
    if database_used_bytes(conn)?.saturating_add(estimated_bytes) > MAX_DATABASE_BYTES {
        cleanup_logs_conn(conn)?;
    }
    let used_bytes = database_used_bytes(conn)?;
    if used_bytes.saturating_add(estimated_bytes) > MAX_DATABASE_BYTES {
        has_payload = false;
    }
    if used_bytes.saturating_add(8 * 1024) > MAX_DATABASE_BYTES {
        return Err(AppError::Database(
            "LLM log database reached the 500 MB capacity limit".to_string(),
        ));
    }
    let id = Uuid::new_v4().to_string();
    let (created_at, created_at_ms) = now_parts();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO llm_requests (
            id, created_at, created_at_ms, agent_id, agent_name, conversation_id,
            provider_name, model_id, status_code, first_token_ms, duration_ms,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
            cost_usd, error, has_payload, is_compaction
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19
        )",
        params![
            id,
            created_at,
            created_at_ms,
            agent_id,
            agent_name,
            conversation_id,
            provider_name,
            model_id,
            raw_status.map(i64::from),
            metrics.first_token_ms.map(|v| v as i64),
            metrics.duration_ms.map(|v| v as i64),
            metrics.input_tokens.map(|v| v as i64),
            metrics.output_tokens.map(|v| v as i64),
            metrics.cache_read_tokens.map(|v| v as i64),
            metrics.cache_write_tokens.map(|v| v as i64),
            metrics.cost_usd,
            stored_error,
            has_payload as i64,
            is_compaction as i64,
        ],
    )?;
    if has_payload {
        tx.execute(
            "INSERT INTO llm_payloads (request_id, raw_request, raw_response)
             VALUES (?1, ?2, ?3)",
            params![id, stored_request, stored_response],
        )?;
    }
    tx.commit()?;
    Ok(id)
}

fn cleanup_logs_conn(conn: &mut Connection) -> AppResult<()> {
    let cutoff_ms = Utc::now()
        .timestamp_millis()
        .saturating_sub((MAX_RETAINED_DAYS as i64) * 24 * 60 * 60 * 1000);
    let mut changed = conn.execute(
        "DELETE FROM llm_requests WHERE created_at_ms < ?1",
        params![cutoff_ms],
    )? > 0;

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM llm_requests", [], |row| row.get(0))?;
    if count > MAX_RETAINED_LOGS as i64 {
        let excess = count - MAX_RETAINED_LOGS as i64;
        changed |= conn.execute(
            "DELETE FROM llm_requests WHERE id IN (
                SELECT id FROM llm_requests
                ORDER BY created_at_ms ASC, id ASC
                LIMIT ?1
            )",
            params![excess],
        )? > 0;
    }

    let target_bytes = MAX_DATABASE_BYTES * 9 / 10;
    while database_used_bytes(conn)? > target_bytes {
        let removed = conn.execute(
            "DELETE FROM llm_requests WHERE id IN (
                SELECT id FROM llm_requests
                ORDER BY created_at_ms ASC, id ASC
                LIMIT 1000
            )",
            [],
        )?;
        if removed == 0 {
            break;
        }
        changed = true;
    }

    conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
    let (page_count, free_count) = database_page_counts(conn)?;
    let fragmented = page_count > 0 && free_count.saturating_mul(4) >= page_count;
    if changed && (database_files_bytes(conn)? > MAX_DATABASE_BYTES || fragmented) {
        conn.execute_batch("VACUUM;")?;
    }
    Ok(())
}

fn database_used_bytes(conn: &Connection) -> AppResult<u64> {
    let (page_count, free_count) = database_page_counts(conn)?;
    let page_size: u64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(page_count
        .saturating_sub(free_count)
        .saturating_mul(page_size))
}

fn database_page_counts(conn: &Connection) -> AppResult<(u64, u64)> {
    let page_count = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let free_count = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
    Ok((page_count, free_count))
}

fn database_files_bytes(conn: &Connection) -> AppResult<u64> {
    let path: String = conn.query_row(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        [],
        |row| row.get(0),
    )?;
    Ok(database_files_bytes_for_path(Path::new(&path)))
}

fn database_files_bytes_for_path(path: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| {
            let mut file = path.as_os_str().to_os_string();
            file.push(suffix);
            std::fs::metadata(std::path::PathBuf::from(file))
                .map(|metadata| metadata.len())
                .unwrap_or(0)
        })
        .fold(0_u64, u64::saturating_add)
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LlmLogEntry> {
    let status: Option<i64> = row.get(7)?;
    let first_token_ms: Option<i64> = row.get(8)?;
    let duration_ms: Option<i64> = row.get(9)?;
    let input_tokens: Option<i64> = row.get(10)?;
    let output_tokens: Option<i64> = row.get(11)?;
    let cache_write_tokens: Option<i64> = row.get(12)?;
    let cache_read_tokens: Option<i64> = row.get(13)?;
    let has_payload: i64 = row.get(16)?;
    Ok(LlmLogEntry {
        id: row.get(0)?,
        agent_name: row.get(1)?,
        agent_id: row.get(2)?,
        conversation_id: row.get(3)?,
        model_id: row.get(4)?,
        provider_name: row.get(5)?,
        created_at: row.get(6)?,
        raw_status: status.and_then(|v| u16::try_from(v).ok()),
        first_token_ms: first_token_ms.map(|v| v as u64),
        duration_ms: duration_ms.map(|v| v as u64),
        input_tokens: input_tokens.map(|v| v as u64),
        output_tokens: output_tokens.map(|v| v as u64),
        cache_write_tokens: cache_write_tokens.map(|v| v as u64),
        cache_read_tokens: cache_read_tokens.map(|v| v as u64),
        cost_usd: row.get(14)?,
        error: row.get(15)?,
        has_payload: has_payload != 0,
        is_compaction: row.get::<_, i64>(17)? != 0,
    })
}

fn build_filter_clause(filter: &LlmLogListFilter) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut params = Vec::new();

    if let Some(agent_id) = filter
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        if let Some(agent_name) = filter
            .agent_name
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            clauses.push(
                "((agent_id IS NOT NULL AND agent_id != '' AND agent_id = ?)
                  OR ((agent_id IS NULL OR agent_id = '') AND agent_name = ?))"
                    .to_string(),
            );
            params.push(agent_id.to_string());
            params.push(agent_name.to_string());
        } else {
            clauses.push("agent_id = ?".to_string());
            params.push(agent_id.to_string());
        }
    } else if let Some(agent_name) = filter
        .agent_name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("agent_name = ?".to_string());
        params.push(agent_name.to_string());
    }

    if let Some(provider_name) = filter
        .provider_name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("provider_name = ?".to_string());
        params.push(provider_name.to_string());
    }

    if let Some(model_id) = filter
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("model_id = ?".to_string());
        params.push(model_id.to_string());
    }

    if let Some(q) = filter.q.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let pattern = format!("%{}%", q.to_ascii_lowercase());
        clauses.push(
            "(lower(agent_name) LIKE ?
              OR lower(provider_name) LIKE ?
              OR lower(model_id) LIKE ?
              OR lower(COALESCE(conversation_id, '')) LIKE ?
              OR lower(COALESCE(agent_id, '')) LIKE ?)"
                .to_string(),
        );
        for _ in 0..5 {
            params.push(pattern.clone());
        }
    }

    if let Some(since) = filter
        .since
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("created_at >= ?".to_string());
        params.push(since.to_string());
    }

    if let Some(until) = filter
        .until
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        clauses.push("created_at < ?".to_string());
        params.push(until.to_string());
    }

    match filter.status.as_deref().map(str::trim) {
        Some("success") => {
            clauses.push(
                "status_code IS NOT NULL AND status_code >= 200 AND status_code < 300
                 AND (error IS NULL OR error = '')"
                    .to_string(),
            );
        }
        Some("error") => {
            clauses.push(
                "((status_code IS NOT NULL AND status_code >= 400)
                  OR (error IS NOT NULL AND error != ''))"
                    .to_string(),
            );
        }
        _ => {}
    }

    if let Some(is_compaction) = filter.is_compaction {
        clauses.push("is_compaction = ?".to_string());
        params.push(if is_compaction { "1" } else { "0" }.to_string());
    }

    if clauses.is_empty() {
        (String::new(), params)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), params)
    }
}

fn truncate_log_field(value: &str) -> String {
    truncate_log_field_to(value, MAX_LOG_FIELD_BYTES)
}

fn truncate_log_field_to(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let marker = "[middle truncated]";
    let available = max_bytes.saturating_sub(marker.len() + 2);
    let head_bytes = available / 2;
    let tail_bytes = available.saturating_sub(head_bytes);
    let head = prefix_at_most(value, head_bytes);
    let tail = suffix_at_most(value, tail_bytes);
    format!("{head}\n{marker}\n{tail}")
}

fn prefix_at_most(value: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn suffix_at_most(value: &str, max_bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(max_bytes);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

pub fn finalize_response_for_log(raw_response: &str) -> String {
    // A head/tail stream preview must remain a stream preview so the UI can
    // show the explicit middle-truncation marker instead of silently
    // aggregating the two retained fragments together.
    if raw_response.contains("middle truncated") {
        return raw_response.to_string();
    }
    let payloads = collect_json_payloads(raw_response);
    if payloads.is_empty() {
        return raw_response.to_string();
    }

    // Non-streaming JSON body.
    if !looks_like_sse(raw_response) {
        if let Ok(value) = serde_json::from_str::<Value>(raw_response) {
            return serde_json::to_string_pretty(&value)
                .unwrap_or_else(|_| raw_response.to_string());
        }
        return raw_response.to_string();
    }

    if let Some(aggregated) = aggregate_stream_payloads(&payloads) {
        return serde_json::to_string_pretty(&aggregated)
            .unwrap_or_else(|_| raw_response.to_string());
    }

    // Fallback: keep the last payload that looks like a complete object with usage/content.
    if let Some(last) = payloads.last() {
        return serde_json::to_string_pretty(last).unwrap_or_else(|_| raw_response.to_string());
    }
    raw_response.to_string()
}

fn looks_like_sse(raw: &str) -> bool {
    raw.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("data:") || trimmed.starts_with("event:")
    })
}

fn aggregate_stream_payloads(payloads: &[Value]) -> Option<Value> {
    aggregate_openai_chat(payloads)
        .or_else(|| aggregate_anthropic(payloads))
        .or_else(|| aggregate_gemini(payloads))
        .or_else(|| aggregate_openai_responses(payloads))
}

fn aggregate_openai_chat(payloads: &[Value]) -> Option<Value> {
    let chat_payloads: Vec<&Value> = payloads
        .iter()
        .filter(|payload| {
            payload
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|choices| {
                    choices.iter().any(|choice| choice.get("delta").is_some())
                        || payload
                            .get("object")
                            .and_then(Value::as_str)
                            .is_some_and(|object| object.contains("chat.completion"))
                })
        })
        .collect();
    if chat_payloads.is_empty() {
        return None;
    }

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut role = "assistant".to_string();
    let mut finish_reason = Value::Null;
    let mut model = Value::Null;
    let mut id = Value::Null;
    let mut usage = Value::Null;
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_call_map: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();

    for payload in chat_payloads {
        if id.is_null()
            && let Some(value) = payload.get("id")
        {
            id = value.clone();
        }
        if model.is_null()
            && let Some(value) = payload.get("model")
        {
            model = value.clone();
        }
        if let Some(value) = payload.get("usage") {
            usage = value.clone();
        }
        let Some(choices) = payload.get("choices").and_then(Value::as_array) else {
            continue;
        };
        for choice in choices {
            if let Some(reason) = choice.get("finish_reason")
                && !reason.is_null()
            {
                finish_reason = reason.clone();
            }
            let Some(delta) = choice.get("delta") else {
                if let Some(message) = choice.get("message")
                    && let Some(text) = message.get("content").and_then(Value::as_str)
                {
                    content.push_str(text);
                }
                continue;
            };
            if let Some(value) = delta.get("role").and_then(Value::as_str) {
                role = value.to_string();
            }
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                content.push_str(text);
            }
            if let Some(text) = delta
                .get("reasoning_content")
                .and_then(Value::as_str)
                .or_else(|| delta.get("reasoning").and_then(Value::as_str))
            {
                reasoning.push_str(text);
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let entry = tool_call_map.entry(index).or_insert_with(|| {
                        (
                            call.get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            call.pointer("/function/name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            String::new(),
                        )
                    });
                    if let Some(call_id) = call.get("id").and_then(Value::as_str)
                        && !call_id.is_empty()
                    {
                        entry.0 = call_id.to_string();
                    }
                    if let Some(name) = call.pointer("/function/name").and_then(Value::as_str)
                        && !name.is_empty()
                    {
                        entry.1 = name.to_string();
                    }
                    if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str)
                    {
                        entry.2.push_str(args);
                    }
                }
            }
        }
    }

    for (index, (call_id, name, arguments)) in tool_call_map {
        tool_calls.push(serde_json::json!({
            "index": index,
            "id": call_id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": arguments,
            }
        }));
    }

    let mut message = serde_json::json!({
        "role": role,
        "content": content,
    });
    if !reasoning.is_empty() {
        message["reasoning_content"] = Value::String(reasoning);
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    Some(serde_json::json!({
        "id": id,
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": usage,
    }))
}

fn aggregate_anthropic(payloads: &[Value]) -> Option<Value> {
    let relevant: Vec<&Value> = payloads
        .iter()
        .filter(|payload| {
            payload
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| {
                    kind.starts_with("message") || kind.starts_with("content_block")
                })
        })
        .collect();
    if relevant.is_empty() {
        return None;
    }

    let mut content = String::new();
    let mut id = Value::Null;
    let mut model = Value::Null;
    let mut usage = serde_json::Map::new();
    let mut stop_reason = Value::Null;

    for payload in relevant {
        match payload.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(message) = payload.get("message") {
                    if let Some(value) = message.get("id") {
                        id = value.clone();
                    }
                    if let Some(value) = message.get("model") {
                        model = value.clone();
                    }
                    if let Some(Value::Object(map)) = message.get("usage") {
                        for (key, value) in map {
                            usage.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
            Some("content_block_delta") => {
                if let Some(text) = payload.pointer("/delta/text").and_then(Value::as_str) {
                    content.push_str(text);
                }
            }
            Some("message_delta") => {
                if let Some(value) = payload.pointer("/delta/stop_reason") {
                    stop_reason = value.clone();
                }
                if let Some(Value::Object(map)) = payload.get("usage") {
                    for (key, value) in map {
                        usage.insert(key.clone(), value.clone());
                    }
                }
            }
            _ => {}
        }
    }

    Some(serde_json::json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": content}],
        "stop_reason": stop_reason,
        "usage": usage,
    }))
}

fn aggregate_gemini(payloads: &[Value]) -> Option<Value> {
    let gemini: Vec<&Value> = payloads
        .iter()
        .filter(|payload| {
            payload.get("candidates").is_some() || payload.get("usageMetadata").is_some()
        })
        .collect();
    if gemini.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut usage = Value::Null;
    let mut model = Value::Null;
    for payload in gemini {
        if let Some(value) = payload.get("usageMetadata") {
            usage = value.clone();
        }
        if model.is_null()
            && let Some(value) = payload.get("modelVersion").or_else(|| payload.get("model"))
        {
            model = value.clone();
        }
        if let Some(candidates) = payload.get("candidates").and_then(Value::as_array) {
            for candidate in candidates {
                if let Some(parts) = candidate
                    .pointer("/content/parts")
                    .and_then(Value::as_array)
                {
                    for part in parts {
                        if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                            text.push_str(chunk);
                        }
                    }
                }
            }
        }
    }
    if text.is_empty() && usage.is_null() {
        return None;
    }
    Some(serde_json::json!({
        "model": model,
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{"text": text}]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": usage,
    }))
}

fn aggregate_openai_responses(payloads: &[Value]) -> Option<Value> {
    let relevant: Vec<&Value> = payloads
        .iter()
        .filter(|payload| {
            payload
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("response.") || kind == "response")
        })
        .collect();
    if relevant.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut usage = Value::Null;
    let mut id = Value::Null;
    let mut model = Value::Null;
    for payload in relevant {
        if let Some(value) = payload
            .pointer("/response/id")
            .or_else(|| payload.get("id"))
        {
            id = value.clone();
        }
        if let Some(value) = payload
            .pointer("/response/model")
            .or_else(|| payload.get("model"))
        {
            model = value.clone();
        }
        if let Some(value) = payload
            .get("usage")
            .or_else(|| payload.pointer("/response/usage"))
        {
            usage = value.clone();
        }
        if let Some(chunk) = payload.pointer("/delta").and_then(Value::as_str) {
            text.push_str(chunk);
        }
        if let Some(chunk) = payload.pointer("/text").and_then(Value::as_str) {
            text.push_str(chunk);
        }
    }
    Some(serde_json::json!({
        "id": id,
        "object": "response",
        "model": model,
        "output_text": text,
        "usage": usage,
    }))
}

pub fn extract_metrics_from_raw(raw_response: Option<&str>) -> LlmLogMetrics {
    let mut metrics = LlmLogMetrics::default();
    let Some(raw) = raw_response else {
        return metrics;
    };

    let payloads = collect_json_payloads(raw);
    for value in payloads.iter().rev() {
        if let Some(usage) = value.get("usage") {
            apply_usage_value(&mut metrics, usage);
            if metrics.cost_usd.is_none() {
                metrics.cost_usd = number_as_f64(usage.get("cost"));
            }
        }
        if let Some(usage) = value.get("usageMetadata") {
            apply_gemini_usage(&mut metrics, usage);
        }
        if let Some(usage) = value.pointer("/message/usage") {
            apply_usage_value(&mut metrics, usage);
        }
        if metrics.cost_usd.is_none() {
            metrics.cost_usd = number_as_f64(value.get("cost"));
        }
        if metrics.input_tokens.is_some() || metrics.output_tokens.is_some() {
            break;
        }
    }
    metrics
}

fn collect_json_payloads(raw: &str) -> Vec<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return vec![value];
    }

    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            let data = line.strip_prefix("data:")?.trim();
            if data.is_empty() || data == "[DONE]" {
                return None;
            }
            serde_json::from_str(data).ok()
        })
        .collect()
}

fn apply_usage_value(metrics: &mut LlmLogMetrics, usage: &Value) {
    let prompt = first_u64(
        usage,
        &[
            "prompt_tokens",
            "input_tokens",
            "promptTokenCount",
            "inputTokenCount",
        ],
    );
    let completion = first_u64(
        usage,
        &[
            "completion_tokens",
            "output_tokens",
            "candidatesTokenCount",
            "outputTokenCount",
        ],
    );
    let cache_read = first_u64(usage, &["cache_read_tokens", "cache_read_input_tokens"])
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| first_u64(details, &["cached_tokens", "cache_read_tokens"]))
        })
        .or_else(|| first_u64(usage, &["cached_tokens", "cachedContentTokenCount"]));
    let cache_write = first_u64(
        usage,
        &[
            "cache_write_tokens",
            "cache_creation_input_tokens",
            "cache_creation_tokens",
        ],
    )
    .or_else(|| {
        usage
            .get("prompt_tokens_details")
            .and_then(|details| first_u64(details, &["cache_write_tokens"]))
    });

    if let Some(prompt) = prompt {
        let cached = cache_read.unwrap_or(0);
        metrics.input_tokens = Some(prompt.saturating_sub(cached));
    }
    if metrics.output_tokens.is_none() {
        metrics.output_tokens = completion;
    }
    if metrics.cache_read_tokens.is_none() {
        metrics.cache_read_tokens = cache_read;
    }
    if metrics.cache_write_tokens.is_none() {
        metrics.cache_write_tokens = cache_write;
    }
}

fn apply_gemini_usage(metrics: &mut LlmLogMetrics, usage: &Value) {
    let prompt = first_u64(usage, &["promptTokenCount", "prompt_token_count"]);
    let output = first_u64(
        usage,
        &[
            "candidatesTokenCount",
            "candidates_token_count",
            "outputTokenCount",
        ],
    );
    let cache_read = first_u64(
        usage,
        &["cachedContentTokenCount", "cached_content_token_count"],
    );
    if let Some(prompt) = prompt {
        let cached = cache_read.unwrap_or(0);
        metrics.input_tokens = Some(prompt.saturating_sub(cached));
    }
    if metrics.output_tokens.is_none() {
        metrics.output_tokens = output;
    }
    if metrics.cache_read_tokens.is_none() {
        metrics.cache_read_tokens = cache_read;
    }
}

fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| number_as_u64(value.get(*key)))
}

fn number_as_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| value.as_f64().map(|n| n.max(0.0) as u64))
        .or_else(|| value.as_str()?.parse().ok())
}

fn number_as_f64(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|n| n as f64))
        .or_else(|| value.as_u64().map(|n| n as f64))
        .or_else(|| value.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_logger(enabled: bool) -> (tempfile::TempDir, LlmLogger) {
        let directory = tempfile::tempdir().unwrap();
        let logger = LlmLogger::new(directory.path(), enabled).unwrap();
        (directory, logger)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn log_listing_is_paginated_and_bounded() {
        let (_dir, logger) = temp_logger(true);
        for index in 0..3 {
            logger
                .write_log(
                    "Agent",
                    None,
                    None,
                    false,
                    "model",
                    "provider",
                    &format!("request-{index}"),
                    Some("response"),
                    Some(200),
                    None,
                    LlmLogMetrics::default(),
                    true,
                )
                .await
                .unwrap();
        }
        let page = logger
            .list_logs_page(0, 2, LlmLogListFilter::default())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 3);
        assert_eq!(logger.list_logs(2, 2).await.unwrap().len(), 1);
        assert!(logger.db_path().ends_with(crate::storage::db::DB_FILE_NAME));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_log_can_omit_payload_but_keep_metrics() {
        let (_dir, logger) = temp_logger(false);
        let id = logger
            .write_log(
                "Agent",
                Some("a1"),
                Some("c1"),
                false,
                "model",
                "provider",
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
                Some(r#"{"usage":{"prompt_tokens":11,"completion_tokens":3,"cost":0.02}}"#),
                Some(200),
                None,
                LlmLogMetrics {
                    duration_ms: Some(42),
                    first_token_ms: Some(7),
                    ..LlmLogMetrics::default()
                },
                false,
            )
            .await
            .unwrap();

        let entries = logger.list_logs(0, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].raw_status, Some(200));
        assert_eq!(entries[0].duration_ms, Some(42));
        assert_eq!(entries[0].first_token_ms, Some(7));
        assert_eq!(entries[0].input_tokens, Some(11));
        assert_eq!(entries[0].output_tokens, Some(3));
        assert_eq!(entries[0].cost_usd, Some(0.02));
        assert!(!entries[0].has_payload);

        let detail = logger.get_log(&id).await.unwrap();
        assert!(detail.raw_request.is_empty());
        assert!(
            detail.raw_response.as_deref().unwrap_or("").is_empty()
                || detail.raw_response.is_none()
        );
        assert!(!detail.summary.has_payload);

        let payload_count: i64 = logger
            .with_conn_for_test(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM llm_payloads WHERE request_id = ?1",
                    params![id],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(payload_count, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_log_stores_payload_when_enabled() {
        let (_dir, logger) = temp_logger(true);
        let id = logger
            .write_log(
                "AI tutor",
                Some("agent-1"),
                Some("conv-1"),
                false,
                "hy3-free",
                "opencode-free",
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
                Some(r#"{"usage":{"prompt_tokens":10,"completion_tokens":2},"choices":[{"message":{"content":"ok"}}]}"#),
                Some(200),
                None,
                LlmLogMetrics {
                    duration_ms: Some(100),
                    first_token_ms: Some(50),
                    ..LlmLogMetrics::default()
                },
                true,
            )
            .await
            .unwrap();

        let detail = logger.get_log(&id).await.unwrap();
        assert!(detail.summary.has_payload);
        assert!(detail.raw_request.contains("hi"));
        assert!(detail.raw_response.as_deref().unwrap_or("").contains("ok"));
        assert_eq!(detail.summary.input_tokens, Some(10));
        assert_eq!(detail.summary.output_tokens, Some(2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn filter_by_agent_and_query() {
        let (_dir, logger) = temp_logger(true);
        logger
            .write_log(
                "Alpha",
                Some("id-a"),
                None,
                false,
                "m1",
                "prov-a",
                "req-a",
                Some("resp"),
                Some(200),
                None,
                LlmLogMetrics::default(),
                true,
            )
            .await
            .unwrap();
        logger
            .write_log(
                "Beta",
                Some("id-b"),
                None,
                false,
                "m2",
                "prov-b",
                "req-b",
                Some("resp"),
                Some(200),
                None,
                LlmLogMetrics::default(),
                true,
            )
            .await
            .unwrap();

        let page = logger
            .list_logs_page(
                0,
                10,
                LlmLogListFilter {
                    agent_id: Some("id-a".into()),
                    agent_name: Some("Alpha".into()),
                    provider_name: None,
                    model_id: None,
                    q: None,
                    since: None,
                    until: None,
                    status: None,
                    is_compaction: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].agent_name, "Alpha");

        let page = logger
            .list_logs_page(
                0,
                10,
                LlmLogListFilter {
                    agent_id: None,
                    agent_name: None,
                    provider_name: None,
                    model_id: None,
                    q: Some("prov-b".into()),
                    since: None,
                    until: None,
                    status: None,
                    is_compaction: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].provider_name, "prov-b");
        assert_eq!(page.agent_stats.len(), 1);
    }

    #[test]
    fn finalize_aggregates_openai_chat_sse() {
        let raw = "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"A\"}}]}\n\ndata: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"B\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\ndata: [DONE]\n";
        let finalized = finalize_response_for_log(raw);
        let value: Value = serde_json::from_str(&finalized).unwrap();
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["choices"][0]["message"]["content"], "AB");
        assert_eq!(value["usage"]["prompt_tokens"], 2);
        assert!(!finalized.contains("chat.completion.chunk"));
    }

    #[test]
    fn finalize_keeps_head_tail_stream_marker_visible() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"HEAD\"}}]}\n[LLM stream log middle truncated]\ndata: {\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}";
        assert_eq!(finalize_response_for_log(raw), raw);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cleanup_removes_logs_older_than_retention_window() {
        let (_dir, logger) = temp_logger(false);
        let now = chrono::Utc::now();
        let timestamps = [
            now - chrono::Duration::days(MAX_RETAINED_DAYS as i64 + 1),
            now - chrono::Duration::days(MAX_RETAINED_DAYS as i64 - 1),
            now,
        ];
        for (offset, ts) in timestamps.into_iter().enumerate() {
            let created_at = ts.to_rfc3339();
            let created_at_ms = ts.timestamp_millis();
            logger
                .with_conn_for_test(move |conn| {
                    conn.execute(
                        "INSERT INTO llm_requests (
                            id, created_at, created_at_ms, agent_id, agent_name, conversation_id,
                            provider_name, model_id, status_code, first_token_ms, duration_ms,
                            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                            cost_usd, error, has_payload
                        ) VALUES (?1, ?2, ?3, NULL, 'A', NULL, 'p', 'm', 200, NULL, NULL,
                                  NULL, NULL, NULL, NULL, NULL, NULL, 0)",
                        params![
                            format!("00000000-0000-0000-0000-{offset:012}"),
                            created_at,
                            created_at_ms
                        ],
                    )?;
                    Ok(())
                })
                .await
                .unwrap();
        }

        logger.cleanup_logs_for_test().await.unwrap();

        let (total, days): (i64, i64) = logger
            .with_conn_for_test(|conn| {
                let total =
                    conn.query_row("SELECT COUNT(*) FROM llm_requests", [], |row| row.get(0))?;
                let days = conn.query_row(
                    "SELECT COUNT(DISTINCT strftime('%Y-%m-%d', created_at_ms / 1000, 'unixepoch'))
                     FROM llm_requests",
                    [],
                    |row| row.get(0),
                )?;
                Ok((total, days))
            })
            .await
            .unwrap();
        assert_eq!(days, 2);
        assert_eq!(total, 2);

        let oldest: String = logger
            .with_conn_for_test(|conn| {
                Ok(conn.query_row(
                    "SELECT strftime('%Y-%m-%d', MIN(created_at_ms) / 1000, 'unixepoch')
                     FROM llm_requests",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            oldest,
            (now - chrono::Duration::days(MAX_RETAINED_DAYS as i64 - 1))
                .format("%Y-%m-%d")
                .to_string()
        );
    }
}
