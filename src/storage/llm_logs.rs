use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const MAX_LOG_FIELD_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_LOGS: usize = 1_000;
const MAX_LIST_LIMIT: usize = 500;
const LOG_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmLogEntry {
    pub id: String,
    pub agent_name: String,
    pub model_id: String,
    pub provider_name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmLogDetail {
    #[serde(flatten)]
    pub summary: LlmLogEntry,
    pub raw_request: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn validate_log_id(log_id: &str) -> AppResult<Uuid> {
    Uuid::parse_str(log_id).map_err(|_| AppError::BadRequest("invalid log id".to_string()))
}

pub struct LlmLogger {
    logs_dir: PathBuf,
    pub enabled: Arc<AtomicBool>,
    writes_since_cleanup: AtomicUsize,
}

impl LlmLogger {
    pub fn new(data_dir: &std::path::Path, enabled: bool) -> Self {
        Self {
            logs_dir: data_dir.join("llm_logs"),
            enabled: Arc::new(AtomicBool::new(enabled)),
            writes_since_cleanup: AtomicUsize::new(0),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, value: bool) {
        self.enabled.store(value, Ordering::Relaxed);
    }

    pub async fn ensure_dir(&self) -> AppResult<()> {
        fs::create_dir_all(&self.logs_dir).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_log(
        &self,
        agent_name: &str,
        model_id: &str,
        provider_name: &str,
        raw_request: &str,
        raw_response: Option<&str>,
        raw_status: Option<u16>,
        error: Option<&str>,
    ) -> AppResult<String> {
        self.ensure_dir().await?;
        let id = Uuid::new_v4().to_string();
        let detail = LlmLogDetail {
            summary: LlmLogEntry {
                id: id.clone(),
                agent_name: agent_name.to_string(),
                model_id: model_id.to_string(),
                provider_name: provider_name.to_string(),
                created_at: Utc::now().to_rfc3339(),
            },
            raw_request: truncate_log_field(raw_request),
            raw_response: raw_response.map(truncate_log_field),
            raw_status,
            error: error.map(truncate_log_field),
        };
        let content = serde_json::to_string_pretty(&detail)?;
        crate::fsutil::write_atomic(&self.logs_dir.join(format!("{id}.json")), content).await?;
        if self
            .writes_since_cleanup
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(32)
        {
            let _ = self.cleanup_logs().await;
        }
        Ok(id)
    }

    pub async fn list_logs(&self, offset: usize, limit: usize) -> AppResult<Vec<LlmLogEntry>> {
        self.ensure_dir().await?;
        self.cleanup_logs().await?;
        let limit = limit.clamp(1, MAX_LIST_LIMIT);
        let mut files = self.log_files().await?;
        files.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
        let mut entries = Vec::new();
        for (path, _) in files.into_iter().skip(offset).take(limit) {
            let content = fs::read_to_string(&path).await?;
            if let Ok(detail) = serde_json::from_str::<LlmLogDetail>(&content) {
                entries.push(detail.summary);
            }
        }
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries)
    }

    pub async fn get_log(&self, log_id: &str) -> AppResult<LlmLogDetail> {
        let _ = validate_log_id(log_id)?;
        let path = self.logs_dir.join(format!("{log_id}.json"));
        let content = fs::read_to_string(&path)
            .await
            .map_err(|_| AppError::NotFound)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub async fn delete_log(&self, log_id: &str) -> AppResult<()> {
        let _ = validate_log_id(log_id)?;
        let path = self.logs_dir.join(format!("{log_id}.json"));
        if fs::metadata(&path).await.is_ok() {
            fs::remove_file(path).await?;
        }
        Ok(())
    }

    async fn cleanup_logs(&self) -> AppResult<()> {
        let cutoff = SystemTime::now()
            .checked_sub(LOG_RETENTION)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut retained = Vec::new();
        for (path, modified) in self.log_files().await? {
            if modified < cutoff {
                let _ = fs::remove_file(path).await;
            } else {
                retained.push((path, modified));
            }
        }
        if retained.len() > MAX_RETAINED_LOGS {
            retained.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
            for (path, _) in retained.into_iter().skip(MAX_RETAINED_LOGS) {
                let _ = fs::remove_file(path).await;
            }
        }
        Ok(())
    }

    async fn log_files(&self) -> AppResult<Vec<(PathBuf, SystemTime)>> {
        let mut files = Vec::new();
        let mut dir = fs::read_dir(&self.logs_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "json") {
                continue;
            }
            let modified = entry
                .metadata()
                .await
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((path, modified));
        }
        Ok(files)
    }
}

fn truncate_log_field(value: &str) -> String {
    if value.len() <= MAX_LOG_FIELD_BYTES {
        return value.to_string();
    }
    let tail = String::from_utf8_lossy(&value.as_bytes()[value.len() - MAX_LOG_FIELD_BYTES..]);
    format!("[log field truncated]\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_listing_is_paginated_and_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let logger = LlmLogger::new(directory.path(), true);
        for index in 0..3 {
            logger
                .write_log(
                    "Agent",
                    "model",
                    "provider",
                    &format!("request-{index}"),
                    Some("response"),
                    Some(200),
                    None,
                )
                .await
                .unwrap();
        }
        assert_eq!(logger.list_logs(0, 2).await.unwrap().len(), 2);
        assert_eq!(logger.list_logs(2, 2).await.unwrap().len(), 1);
        assert_eq!(
            logger
                .list_logs(0, MAX_LIST_LIMIT + 100)
                .await
                .unwrap()
                .len(),
            3
        );
    }
}
