use reqwest::StatusCode;
use serde::Deserialize;

use crate::tools::web_search::types::{ProviderError, WebSearchHit, sanitize_error_message};

/// Browser-like UA helps when the provider sits behind Cloudflare bot filters.
const JINA_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; WeiAgent/1.0; +https://github.com/weiagent)";

#[derive(Debug, Deserialize)]
struct JinaResponse {
    #[serde(default)]
    data: Vec<JinaResult>,
}

#[derive(Debug, Deserialize)]
struct JinaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
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
    let endpoint = build_endpoint(base_url, query, max_results)?;

    let response = client
        .get(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", JINA_USER_AGENT)
        .send()
        .await
        .map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("Jina request failed: {error}"),
                api_key,
            ))
        })?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("Jina response body error: {error}"),
                api_key,
            ))
        })?;

    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Jina HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }
    if !status.is_success() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Jina HTTP {status}: {}", truncate(&text, 240)),
            api_key,
        )));
    }

    let parsed: JinaResponse = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Jina returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;

    Ok(parsed
        .data
        .into_iter()
        .filter_map(|item| {
            let url = item.url.filter(|value| !value.trim().is_empty())?;
            let snippet = item
                .description
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    item.content
                        .map(|value| truncate(value.trim(), 500))
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or_default();
            Some(WebSearchHit {
                title: item
                    .title
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| url.clone()),
                url,
                snippet,
                score: item.score,
            })
        })
        .collect())
}

fn build_endpoint(
    base_url: &str,
    query: &str,
    max_results: u32,
) -> Result<reqwest::Url, ProviderError> {
    let mut endpoint = reqwest::Url::parse(base_url.trim_end_matches('/')).map_err(|error| {
        ProviderError::Fatal(format!("Jina base URL is invalid: {error}"))
    })?;
    endpoint
        .path_segments_mut()
        .map_err(|_| ProviderError::Fatal("Jina base URL cannot accept a path".to_string()))?
        .push(query);
    endpoint
        .query_pairs_mut()
        .append_pair("num", &max_results.to_string())
        .append_pair("type", "web");
    Ok(endpoint)
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
    fn builds_encoded_search_endpoint() {
        let endpoint = build_endpoint("https://s.jina.ai", "rust async/await", 7).unwrap();
        assert_eq!(endpoint.as_str(), "https://s.jina.ai/rust%20async%2Fawait?num=7&type=web");
    }

    #[test]
    fn maps_jina_json() {
        let raw = r#"{"code":200,"status":200,"data":[{"title":"Jina Doc","url":"https://example.com/jina","description":"snippet text","content":"full body","score":0.85},{"title":null,"url":"https://example.com/b","description":"","content":"content here only"}]}"#;
        let parsed: JinaResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].title.as_deref(), Some("Jina Doc"));
        assert!(parsed.data[0].description.as_ref().is_some());
        assert_eq!(parsed.data[1].title, None);
        assert_eq!(parsed.data[1].content.as_deref(), Some("content here only"));
    }

    #[test]
    fn prefers_description_then_content() {
        let raw = r#"{"data":[{"title":"A","url":"https://example.com/a","description":"","content":"from content"},{"title":"B","url":"https://example.com/b","description":"from desc","content":"ignored"}]}"#;
        let parsed: JinaResponse = serde_json::from_str(raw).unwrap();
        let hits: Vec<WebSearchHit> = parsed
            .data
            .into_iter()
            .filter_map(|item| {
                let url = item.url.filter(|value| !value.trim().is_empty())?;
                let snippet = item
                    .description
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| {
                        item.content
                            .map(|value| truncate(value.trim(), 500))
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or_default();
                Some(WebSearchHit {
                    title: item.title.unwrap_or_else(|| url.clone()),
                    url,
                    snippet,
                    score: item.score,
                })
            })
            .collect();
        assert_eq!(hits[0].snippet, "from content");
        assert_eq!(hits[1].snippet, "from desc");
    }
}
