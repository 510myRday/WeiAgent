use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::tools::web_search::types::{
    ProviderError, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind, WebSearchHit,
    sanitize_error_message,
};

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
            let snippet = item
                .highlights
                .as_ref()
                .map(|parts| parts.join(" … "))
                .filter(|value| !value.trim().is_empty())
                .or(item.text)
                .unwrap_or_default();
            Some(WebSearchHit {
                title: item
                    .title
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| url.clone()),
                url,
                snippet,
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
}
