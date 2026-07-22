use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use futures_util::{StreamExt, future::join_all};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::ToolResult;
use crate::domain::tools::{EvidenceStorageConfig, MAX_WEB_EXTRACT_URLS};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_CHARS_PER_URL: usize = 8_000;
const MAX_CHARS_PER_URL: usize = 20_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebExtractArgs {
    pub urls: Vec<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub max_chars_per_url: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebExtractDocument {
    pub document_id: String,
    pub requested_url: String,
    pub final_url: String,
    pub title: String,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub relevant_excerpts: Vec<String>,
    pub extracted_chars: usize,
    pub stored: bool,
}

#[derive(Debug, Clone, Serialize)]
struct WebExtractFailure {
    requested_url: String,
    error: String,
}

#[derive(Debug, Clone, Serialize)]
struct WebExtractOutput {
    documents: Vec<WebExtractDocument>,
    failures: Vec<WebExtractFailure>,
    citation_instruction: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct StoredEvidence<'a> {
    document_id: &'a str,
    requested_url: &'a str,
    final_url: &'a str,
    title: &'a str,
    content_type: &'a str,
    published_at: &'a Option<String>,
    extracted_at: String,
    content: &'a str,
}

pub async fn execute_web_extract(
    args: WebExtractArgs,
    data_dir: &Path,
    evidence_storage: &EvidenceStorageConfig,
) -> ToolResult {
    let started = Instant::now();
    let urls = normalize_urls(args.urls);
    if urls.is_empty() {
        return ToolResult::Error {
            message: "urls must contain at least one http or https URL".to_string(),
        };
    }
    if urls.len() > MAX_WEB_EXTRACT_URLS {
        return ToolResult::Error {
            message: format!("web_extract accepts at most {MAX_WEB_EXTRACT_URLS} URLs"),
        };
    }
    let max_chars = args
        .max_chars_per_url
        .unwrap_or(DEFAULT_MAX_CHARS_PER_URL)
        .clamp(1_000, MAX_CHARS_PER_URL);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 {
                return attempt.error("too many redirects");
            }
            if is_public_http_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirect target is not a public HTTP URL")
            }
        }))
        .user_agent("WeiAgent/1.0 web_extract")
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ToolResult::Error {
                message: format!("failed to build extraction client: {error}"),
            };
        }
    };
    let query = args.query.unwrap_or_default();
    let tasks = urls.into_iter().map(|url| {
        extract_one(
            client.clone(),
            url,
            query.clone(),
            max_chars,
            data_dir,
            evidence_storage.enabled,
        )
    });
    let mut documents = Vec::new();
    let mut failures = Vec::new();
    for result in join_all(tasks).await {
        match result {
            Ok(document) => documents.push(document),
            Err(failure) => failures.push(failure),
        }
    }
    if evidence_storage.enabled {
        if let Err(error) = cleanup_evidence_store(data_dir, evidence_storage).await {
            tracing::warn!(error = %error, "failed to clean evidence store");
        }
    }
    let output = WebExtractOutput {
        documents,
        failures,
        citation_instruction: "Cite the final_url of the original page next to each supported claim. A stored document_id is internal evidence, not a public citation target.",
    };
    match serde_json::to_string_pretty(&output) {
        Ok(stdout) => ToolResult::Completed {
            stdout,
            stderr: String::new(),
            exit_code: 0,
            duration_ms: started.elapsed().as_millis() as u64,
            runtime: Some("builtin_web_extract".to_string()),
            output_cursor: 0,
        },
        Err(error) => ToolResult::Error {
            message: format!("failed to serialize extracted evidence: {error}"),
        },
    }
}

async fn extract_one(
    client: reqwest::Client,
    requested_url: String,
    query: String,
    max_chars: usize,
    data_dir: &Path,
    store_enabled: bool,
) -> Result<WebExtractDocument, WebExtractFailure> {
    let parsed = reqwest::Url::parse(&requested_url).map_err(|error| WebExtractFailure {
        requested_url: requested_url.clone(),
        error: format!("invalid URL: {error}"),
    })?;
    if !is_public_http_url(&parsed) {
        return Err(WebExtractFailure {
            requested_url,
            error: "URL must use HTTP(S) and target a public host".to_string(),
        });
    }
    let response = client
        .get(parsed)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/pdf,text/plain,application/json",
        )
        .send()
        .await
        .map_err(|error| WebExtractFailure {
            requested_url: requested_url.clone(),
            error: format!("request failed: {error}"),
        })?;
    let status = response.status();
    let final_url = response.url().to_string();
    if !status.is_success() {
        return Err(WebExtractFailure {
            requested_url,
            error: format!("HTTP {status}"),
        });
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_ascii_lowercase();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| WebExtractFailure {
            requested_url: requested_url.clone(),
            error: format!("response body failed: {error}"),
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(WebExtractFailure {
                requested_url,
                error: format!("response exceeds {MAX_RESPONSE_BYTES} bytes"),
            });
        }
        bytes.extend_from_slice(&chunk);
    }

    let extracted = extract_content(&bytes, &content_type).map_err(|error| WebExtractFailure {
        requested_url: requested_url.clone(),
        error,
    })?;
    let content = normalize_whitespace(&extracted.content);
    if content.is_empty() {
        return Err(WebExtractFailure {
            requested_url,
            error: "page contained no extractable text".to_string(),
        });
    }
    let document_id = evidence_id(&final_url, &bytes);
    let relevant_excerpts = select_excerpts(&content, &query, max_chars);
    let store_path = data_dir
        .join("evidence")
        .join(format!("{document_id}.json"));
    let stored = if store_enabled {
        match serde_json::to_string_pretty(&StoredEvidence {
            document_id: &document_id,
            requested_url: &requested_url,
            final_url: &final_url,
            title: &extracted.title,
            content_type: &content_type,
            published_at: &extracted.published_at,
            extracted_at: chrono::Utc::now().to_rfc3339(),
            content: &content,
        }) {
            Ok(json) => {
                if let Some(parent) = store_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                crate::fsutil::write_atomic(&store_path, json).await.is_ok()
            }
            Err(_) => false,
        }
    } else {
        false
    };
    Ok(WebExtractDocument {
        document_id,
        requested_url,
        final_url,
        title: extracted.title,
        content_type,
        published_at: extracted.published_at,
        relevant_excerpts,
        extracted_chars: content.chars().count(),
        stored,
    })
}

#[derive(Debug)]
struct EvidenceFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EvidenceCleanupStats {
    pub removed_files: usize,
    pub removed_bytes: u64,
    pub remaining_bytes: u64,
}

pub async fn cleanup_evidence_store(
    data_dir: &Path,
    config: &EvidenceStorageConfig,
) -> std::io::Result<EvidenceCleanupStats> {
    if !config.enabled {
        return Ok(EvidenceCleanupStats::default());
    }

    let evidence_dir = data_dir.join("evidence");
    let mut directory = match tokio::fs::read_dir(&evidence_dir).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EvidenceCleanupStats::default());
        }
        Err(error) => return Err(error),
    };
    let now = SystemTime::now();
    let retention = Duration::from_secs(u64::from(config.retention_days).saturating_mul(86_400));
    let cutoff = now.checked_sub(retention).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut files = Vec::new();
    let mut stats = EvidenceCleanupStats::default();

    while let Some(entry) = directory.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let metadata = match entry.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = metadata.len();
        if modified < cutoff {
            if remove_evidence_file(&path).await? {
                stats.removed_files += 1;
                stats.removed_bytes = stats.removed_bytes.saturating_add(size);
            }
            continue;
        }
        files.push(EvidenceFile {
            path,
            size,
            modified,
        });
    }

    files.sort_by_key(|file| file.modified);
    let mut total_bytes = files
        .iter()
        .fold(0_u64, |total, file| total.saturating_add(file.size));
    let max_bytes = config.max_total_mb.saturating_mul(1024 * 1024);
    for file in files {
        if total_bytes <= max_bytes {
            break;
        }
        if remove_evidence_file(&file.path).await? {
            total_bytes = total_bytes.saturating_sub(file.size);
            stats.removed_files += 1;
            stats.removed_bytes = stats.removed_bytes.saturating_add(file.size);
        }
    }
    stats.remaining_bytes = total_bytes;
    Ok(stats)
}

async fn remove_evidence_file(path: &Path) -> std::io::Result<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

struct ExtractedContent {
    title: String,
    published_at: Option<String>,
    content: String,
}

fn extract_content(bytes: &[u8], content_type: &str) -> Result<ExtractedContent, String> {
    if content_type == "application/pdf" || bytes.starts_with(b"%PDF-") {
        let content = pdf_extract::extract_text_from_mem(bytes)
            .map_err(|error| format!("PDF extraction failed: {error}"))?;
        return Ok(ExtractedContent {
            title: "PDF document".to_string(),
            published_at: None,
            content,
        });
    }
    let text = String::from_utf8_lossy(bytes).to_string();
    if content_type.contains("html") || text.trim_start().starts_with('<') {
        return Ok(extract_html(&text));
    }
    Ok(ExtractedContent {
        title: "Web document".to_string(),
        published_at: None,
        content: text,
    })
}

fn extract_html(html: &str) -> ExtractedContent {
    let document = Html::parse_document(html);
    let title = first_text(&document, "meta[property='og:title']", Some("content"))
        .or_else(|| first_text(&document, "title", None))
        .unwrap_or_else(|| "Web page".to_string());
    let published_at = first_text(
        &document,
        "meta[property='article:published_time']",
        Some("content"),
    )
    .or_else(|| first_text(&document, "meta[name='date']", Some("content")))
    .or_else(|| first_text(&document, "time[datetime]", Some("datetime")));
    let root_selector = Selector::parse("article, main, body").expect("static selector");
    let content = document
        .select(&root_selector)
        .next()
        .map(|element| element.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    ExtractedContent {
        title,
        published_at,
        content,
    }
}

fn first_text(document: &Html, selector: &str, attribute: Option<&str>) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    let element = document.select(&selector).next()?;
    let value = if let Some(attribute) = attribute {
        element.value().attr(attribute)?.to_string()
    } else {
        element.text().collect::<Vec<_>>().join(" ")
    };
    let value = normalize_whitespace(&value);
    (!value.is_empty()).then_some(value)
}

fn select_excerpts(content: &str, query: &str, max_chars: usize) -> Vec<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 2)
        .collect::<Vec<_>>();
    let mut paragraphs = content
        .split(['\n', '。', '！', '？'])
        .map(str::trim)
        .filter(|paragraph| paragraph.chars().count() >= 40)
        .map(|paragraph| {
            let lower = paragraph.to_lowercase();
            let score = terms
                .iter()
                .filter(|term| lower.contains(term.as_str()))
                .count();
            (score, paragraph.to_string())
        })
        .collect::<Vec<_>>();
    paragraphs.sort_by_key(|item| std::cmp::Reverse(item.0));
    let mut remaining = max_chars;
    let mut excerpts = Vec::new();
    for (_, paragraph) in paragraphs {
        if remaining == 0 || excerpts.len() >= 8 {
            break;
        }
        let excerpt = paragraph
            .chars()
            .take(remaining.min(2_000))
            .collect::<String>();
        remaining = remaining.saturating_sub(excerpt.chars().count());
        excerpts.push(excerpt);
    }
    if excerpts.is_empty() {
        excerpts.push(content.chars().take(max_chars).collect());
    }
    excerpts
}

fn normalize_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    urls.into_iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty() && seen.insert(url.clone()))
        .collect()
}

fn evidence_id(final_url: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(final_url.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_public_http_url(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified())
        }
        Ok(IpAddr::V6(ip)) => !(ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local()),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_local_urls() {
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://127.0.0.1/x").unwrap()
        ));
        assert!(!is_public_http_url(
            &reqwest::Url::parse("http://localhost/x").unwrap()
        ));
        assert!(is_public_http_url(
            &reqwest::Url::parse("https://example.com/x").unwrap()
        ));
    }

    #[test]
    fn html_extracts_title_date_and_body() {
        let extracted = extract_html(
            "<html><head><title>A</title><meta property='article:published_time' content='2026-01-01'></head><body><main>Hello world</main></body></html>",
        );
        assert_eq!(extracted.title, "A");
        assert_eq!(extracted.published_at.as_deref(), Some("2026-01-01"));
        assert!(extracted.content.contains("Hello world"));
    }

    #[tokio::test]
    async fn cleanup_removes_expired_files_and_enforces_capacity() {
        let data_dir = tempfile::tempdir().unwrap();
        let evidence_dir = data_dir.path().join("evidence");
        tokio::fs::create_dir_all(&evidence_dir).await.unwrap();

        let expired_path = evidence_dir.join("expired.json");
        tokio::fs::write(&expired_path, vec![b'x'; 32])
            .await
            .unwrap();
        let expired_file = std::fs::OpenOptions::new()
            .write(true)
            .open(&expired_path)
            .unwrap();
        expired_file
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH)
                    .set_accessed(SystemTime::UNIX_EPOCH),
            )
            .unwrap();

        for index in 0..3 {
            tokio::fs::write(
                evidence_dir.join(format!("current-{index}.json")),
                vec![b'x'; 700 * 1024],
            )
            .await
            .unwrap();
        }
        tokio::fs::write(evidence_dir.join("keep.txt"), b"unrelated")
            .await
            .unwrap();

        let config = EvidenceStorageConfig {
            enabled: true,
            retention_days: 30,
            max_total_mb: 1,
        };
        let stats = cleanup_evidence_store(data_dir.path(), &config)
            .await
            .unwrap();

        assert!(!expired_path.exists());
        assert!(evidence_dir.join("keep.txt").exists());
        assert!(stats.removed_files >= 3);
        assert!(stats.remaining_bytes <= 1024 * 1024);
    }
}
