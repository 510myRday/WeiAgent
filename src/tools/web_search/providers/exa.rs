use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::tools::web_search::types::{
    ProviderError, ProviderExtractDocument, ProviderExtractFailure, ProviderExtractOutput,
    ProviderExtractRequest, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind,
    WebSearchHit, sanitize_error_message,
};

const MAX_EXTRACT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONTENT_EXCERPT_CHARS: usize = 4_000;

#[derive(Debug, Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    highlights: Option<Vec<String>>,
    #[serde(default)]
    score: Option<f64>,
}

pub async fn search(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
) -> Result<ProviderSearchOutput, ProviderError> {
    let endpoint = format!("{}/search", base_url.trim_end_matches('/'));
    let mut body = json!({
        "query": request.query,
        "numResults": request.max_results,
        "contents": {
            "highlights": true,
            "text": { "maxCharacters": 1000 }
        }
    });
    if let Some(object) = body.as_object_mut() {
        if !request.allowed_domains.is_empty() {
            object.insert("includeDomains".to_string(), json!(request.allowed_domains));
        }
        if !request.blocked_domains.is_empty() {
            object.insert("excludeDomains".to_string(), json!(request.blocked_domains));
        }
        if let Some(days) = request.recency_days {
            let start = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
            object.insert("startPublishedDate".to_string(), json!(start.to_rfc3339()));
        }
    }

    let response = client
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("Exa request failed: {error}"),
                api_key,
            ))
        })?;

    let status = response.status();
    let text = response.text().await.map_err(|error| {
        ProviderError::Retryable(sanitize_error_message(
            &format!("Exa response body error: {error}"),
            api_key,
        ))
    })?;

    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Exa HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }
    if !status.is_success() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Exa HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }

    let parsed: ExaResponse = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Exa returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;

    let results = parsed
        .results
        .into_iter()
        .filter_map(|item| {
            let url = item.url.filter(|value| !value.trim().is_empty())?;
            let content_excerpt = item
                .text
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| truncate(value, MAX_CONTENT_EXCERPT_CHARS));
            let snippet = item
                .highlights
                .as_ref()
                .map(|parts| parts.join(" … "))
                .filter(|value| !value.trim().is_empty())
                .or_else(|| content_excerpt.as_deref().map(|value| truncate(value, 500)))
                .unwrap_or_default();
            Some(WebSearchHit {
                title: item
                    .title
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| url.clone()),
                url,
                snippet,
                content_excerpt,
                kind: SearchHitKind::WebPage,
                published_at: None,
                score: item.score,
            })
        })
        .collect();
    Ok(ProviderSearchOutput {
        results,
        ai_syntheses: Vec::new(),
    })
}

pub async fn extract(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    let endpoint = format!("{}/contents", base_url.trim_end_matches('/'));
    let body = json!({
        "urls": [request.url],
        "text": true,
        "maxAgeHours": if request.hard_page { 0 } else { 24 },
        "livecrawlTimeout": request.timeout_ms.clamp(1_000, 90_000),
    });
    let response = client
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("Exa contents request failed: {error}"),
                api_key,
            ))
        })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| {
        ProviderError::Retryable(sanitize_error_message(
            &format!("Exa contents response body error: {error}"),
            api_key,
        ))
    })?;
    if bytes.len() > MAX_EXTRACT_RESPONSE_BYTES {
        return Err(ProviderError::Retryable(format!(
            "Exa contents response exceeds {MAX_EXTRACT_RESPONSE_BYTES} bytes"
        )));
    }
    let text = String::from_utf8_lossy(&bytes);
    if !status.is_success() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Exa contents HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Exa contents returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;
    Ok(parse_extract_output(&value, &request.url))
}

fn parse_extract_output(value: &serde_json::Value, requested_url: &str) -> ProviderExtractOutput {
    let mut output = ProviderExtractOutput::default();
    if let Some(results) = value.get("results").and_then(serde_json::Value::as_array) {
        for result in results {
            let Some(object) = result.as_object() else {
                continue;
            };
            let content = object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    object
                        .get("highlights")
                        .and_then(serde_json::Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .filter(|value| !value.trim().is_empty())
                });
            let Some(content) = content else {
                continue;
            };
            let final_url = object
                .get("url")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(requested_url)
                .to_string();
            output.documents.push(ProviderExtractDocument {
                requested_url: object
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(requested_url)
                    .to_string(),
                title: object
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(&final_url)
                    .to_string(),
                final_url,
                content,
                content_format: "text".to_string(),
                published_at: object
                    .get("publishedDate")
                    .or_else(|| object.get("published_at"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                ai_derived: false,
            });
        }
    }
    if let Some(statuses) = value.get("statuses").and_then(serde_json::Value::as_array) {
        for status in statuses {
            let Some(object) = status.as_object() else {
                continue;
            };
            let state = object
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if state.eq_ignore_ascii_case("success") {
                continue;
            }
            output.failures.push(ProviderExtractFailure {
                requested_url: object
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(requested_url)
                    .to_string(),
                message: object
                    .get("error")
                    .or_else(|| object.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Exa content extraction failed")
                    .to_string(),
            });
        }
    }
    output
}

pub fn classify_key_status(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("http 401")
        || lower.contains("invalid_api_key")
        || lower.contains("invalid api key")
        || lower.contains("unauthorized")
    {
        return "invalid_key";
    }
    if lower.contains("http 402")
        || lower.contains("no_more_credits")
        || lower.contains("credits exhausted")
        || lower.contains("spending budget exceeded")
        || lower.contains("payment required")
    {
        return "quota_exhausted";
    }
    if lower.contains("http 429")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
    {
        return "rate_limited";
    }
    if lower.contains("http 403")
        || lower.contains("insufficient permissions")
        || lower.contains("feature disabled")
        || lower.contains("forbidden")
    {
        return "permission_denied";
    }
    if lower.contains("http 400") || lower.contains("invalid request") {
        return "configuration_error";
    }
    if lower.contains("http 5")
        || lower.contains("service unavailable")
        || lower.contains("internal server error")
        || lower.contains("bad gateway")
    {
        return "service_error";
    }
    if lower.contains("request failed")
        || lower.contains("response body error")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("connect")
        || lower.contains("dns")
    {
        return "network_error";
    }
    "error"
}

fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_exa_json() {
        let raw = r#"{
            "results": [
                {
                    "title": "Exa Doc",
                    "url": "https://example.com/exa",
                    "text": "full text",
                    "highlights": ["h1", "h2"],
                    "score": 0.5
                }
            ]
        }"#;
        let parsed: ExaResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.results[0].title.as_deref(), Some("Exa Doc"));
        assert_eq!(parsed.results[0].highlights.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn classifies_exa_key_statuses() {
        assert_eq!(
            classify_key_status("Exa HTTP 401: INVALID_API_KEY"),
            "invalid_key"
        );
        assert_eq!(
            classify_key_status("Exa HTTP 402: NO_MORE_CREDITS"),
            "quota_exhausted"
        );
        assert_eq!(
            classify_key_status("Exa HTTP 429: rate limit exceeded"),
            "rate_limited"
        );
    }

    #[test]
    fn maps_exa_contents_json() {
        let output = parse_extract_output(
            &serde_json::json!({
                "results": [{
                    "id": "https://example.com",
                    "title": "Example",
                    "url": "https://example.com/final",
                    "text": "Full extracted article"
                }],
                "statuses": [{"id":"https://example.com","status":"success"}]
            }),
            "https://example.com",
        );
        assert_eq!(output.documents.len(), 1);
        assert_eq!(output.documents[0].title, "Example");
        assert!(output.failures.is_empty());
    }
}
