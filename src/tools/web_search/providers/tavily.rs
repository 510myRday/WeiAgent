use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::tools::web_search::types::{
    ProviderError, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind, WebSearchHit,
    sanitize_error_message,
};

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
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
        "max_results": request.max_results,
        "include_answer": false,
    });
    if let Some(object) = body.as_object_mut() {
        if !request.allowed_domains.is_empty() {
            object.insert(
                "include_domains".to_string(),
                json!(request.allowed_domains),
            );
        }
        if !request.blocked_domains.is_empty() {
            object.insert(
                "exclude_domains".to_string(),
                json!(request.blocked_domains),
            );
        }
        if let Some(days) = request.recency_days {
            object.insert("days".to_string(), json!(days));
        }
    }

    let response = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("Tavily request failed: {error}"),
                api_key,
            ))
        })?;

    let status = response.status();
    let text = response.text().await.map_err(|error| {
        ProviderError::Retryable(sanitize_error_message(
            &format!("Tavily response body error: {error}"),
            api_key,
        ))
    })?;

    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Tavily HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }
    if !status.is_success() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Tavily HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }

    let parsed: TavilyResponse = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Tavily returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;

    let results = parsed
        .results
        .into_iter()
        .filter_map(|item| {
            let url = item.url.filter(|value| !value.trim().is_empty())?;
            Some(WebSearchHit {
                title: item
                    .title
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| url.clone()),
                url,
                snippet: item.content.unwrap_or_default(),
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
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("missing api key")
    {
        return "invalid_key";
    }
    if lower.contains("http 432")
        || lower.contains("http 433")
        || lower.contains("usage limit")
        || lower.contains("pay-as-you-go limit")
        || lower.contains("credits exhausted")
        || lower.contains("quota exceeded")
    {
        return "quota_exhausted";
    }
    if lower.contains("http 429")
        || lower.contains("too many requests")
        || lower.contains("excessive requests")
        || lower.contains("rate limit")
    {
        return "rate_limited";
    }
    classify_common_status(&lower)
}

fn classify_common_status(lower: &str) -> &'static str {
    if lower.contains("http 403") || lower.contains("forbidden") {
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
    fn maps_tavily_json() {
        let raw = r#"{
            "results": [
                {"title": "A", "url": "https://example.com/a", "content": "snippet", "score": 0.9},
                {"title": null, "url": "https://example.com/b", "content": "x"}
            ]
        }"#;
        let parsed: TavilyResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(parsed.results[0].title.as_deref(), Some("A"));
    }

    #[test]
    fn classifies_tavily_key_statuses() {
        assert_eq!(
            classify_key_status("Tavily HTTP 401: Unauthorized: missing or invalid API key"),
            "invalid_key"
        );
        assert_eq!(
            classify_key_status("Tavily HTTP 432: request exceeds your plan's usage limit"),
            "quota_exhausted"
        );
        assert_eq!(
            classify_key_status("Tavily HTTP 429: excessive requests"),
            "rate_limited"
        );
    }
}
