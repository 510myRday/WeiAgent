use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::tools::web_search::types::{ProviderError, WebSearchHit, sanitize_error_message};

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
    query: &str,
    max_results: u32,
) -> Result<Vec<WebSearchHit>, ProviderError> {
    let endpoint = format!("{}/search", base_url.trim_end_matches('/'));
    let body = json!({
        "query": query,
        "max_results": max_results,
        "include_answer": false,
    });

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
    let text = response
        .text()
        .await
        .map_err(|error| {
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

    Ok(parsed
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
                score: item.score,
            })
        })
        .collect())
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
}

