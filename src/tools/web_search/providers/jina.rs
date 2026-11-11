use reqwest::RequestBuilder;
use serde::Deserialize;

use super::support::{MAX_EXTRACT_RESPONSE_BYTES, classify_common_status, send_checked, truncate};
use crate::tools::web_search::types::{
    ProviderError, ProviderExtractDocument, ProviderExtractFailure, ProviderExtractOutput,
    ProviderExtractRequest, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind,
    WebSearchHit,
};

/// Browser-like UA helps when the provider sits behind Cloudflare bot filters.
const JINA_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; WeiAgent/1.0; +https://github.com/weiagent)";
const MAX_CONTENT_EXCERPT_CHARS: usize = 4_000;

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
    request: &ProviderSearchRequest,
) -> Result<ProviderSearchOutput, ProviderError> {
    let endpoint = build_endpoint(base_url, &request.query, request.max_results)?;

    let builder = build_request(client, endpoint, api_key);
    let text = send_checked(builder, "Jina", api_key, None, 240, "…").await?;

    let parsed: JinaResponse = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Jina returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;

    let results = parsed
        .data
        .into_iter()
        .filter_map(|item| {
            let url = item.url.filter(|value| !value.trim().is_empty())?;
            let content_excerpt = item
                .content
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| truncate(value, MAX_CONTENT_EXCERPT_CHARS));
            let snippet = item
                .description
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    content_excerpt
                        .as_deref()
                        .map(|value| truncate(value, 500))
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
                content_excerpt,
                kind: SearchHitKind::WebPage,
                published_at: None,
                score: item.score,
            })
        })
        .collect();
    Ok(ProviderSearchOutput {
        results,
    })
}

pub async fn extract(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    let endpoint = format!("{}/{}", base_url.trim_end_matches('/'), request.url);
    let endpoint = reqwest::Url::parse(&endpoint).map_err(|error| {
        ProviderError::Retryable(format!("Jina Reader URL is invalid: {error}"))
    })?;
    let builder = build_request(client, endpoint, api_key)
        .header("X-Return-Format", "markdown")
        .header(
            "X-Timeout",
            (request.timeout_ms / 1_000).clamp(1, 180).to_string(),
        );
    let text = send_checked(
        builder,
        "Jina Reader",
        api_key,
        Some(MAX_EXTRACT_RESPONSE_BYTES),
        240,
        "…",
    )
    .await?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Jina Reader returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;
    Ok(parse_extract_output(&value, &request.url))
}

fn parse_extract_output(value: &serde_json::Value, requested_url: &str) -> ProviderExtractOutput {
    let mut output = ProviderExtractOutput::default();
    if let Some(message) = value
        .get("message")
        .or_else(|| value.get("error"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        output.failures.push(ProviderExtractFailure {
            requested_url: requested_url.to_string(),
            message: message.to_string(),
        });
    }
    let data = value.get("data").unwrap_or(value);
    let items = match data {
        serde_json::Value::Array(items) => items.iter().collect::<Vec<_>>(),
        serde_json::Value::Object(_) => vec![data],
        _ => Vec::new(),
    };
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let content = object
            .get("content")
            .or_else(|| object.get("markdown"))
            .or_else(|| object.get("text"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
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
            requested_url: requested_url.to_string(),
            title: object
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&final_url)
                .to_string(),
            final_url,
            content: content.to_string(),
            content_format: "markdown".to_string(),
            published_at: object
                .get("publishedTime")
                .or_else(|| object.get("published_at"))
                .or_else(|| object.get("date"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
        });
    }
    if output.documents.is_empty() && output.failures.is_empty() {
        output.failures.push(ProviderExtractFailure {
            requested_url: requested_url.to_string(),
            message: "Jina Reader returned no extractable content".to_string(),
        });
    }
    output
}

fn build_request(
    client: &reqwest::Client,
    endpoint: reqwest::Url,
    api_key: &str,
) -> RequestBuilder {
    let request = client
        .get(endpoint)
        .header("Accept", "application/json")
        .header("User-Agent", JINA_USER_AGENT);
    let api_key = api_key.trim();
    if api_key.is_empty() {
        request
    } else {
        request.bearer_auth(api_key)
    }
}

pub fn classify_key_status(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("http 401")
        || lower.contains("auth_invalid_api_key")
        || lower.contains("auth_missing_api_key")
        || lower.contains("invalid api key")
        || lower.contains("authentication required")
        || lower.contains("unauthorized")
    {
        return "invalid_key";
    }
    if lower.contains("authz_insufficient_balance")
        || lower.contains("insufficient account balance")
        || lower.contains("payment required")
        || lower.contains("credits exhausted")
        || lower.contains("quota exceeded")
    {
        return "quota_exhausted";
    }
    if lower.contains("http 429")
        || lower.contains("rate_request_limit_exceeded")
        || lower.contains("rate_token_limit_exceeded")
        || lower.contains("rate_concurrency_limit_exceeded")
        || lower.contains("rate_ip_limit_exceeded")
        || lower.contains("rate limit exceeded")
        || lower.contains("too many requests")
    {
        return "rate_limited";
    }
    if lower.contains("authz_resource_limit_exceeded")
        || lower.contains("resource limit exceeded for your plan")
        || lower.contains("http 403")
        || lower.contains("forbidden")
    {
        return "permission_denied";
    }
    if lower.contains("http 400")
        || lower.contains("input_model_not_found")
        || lower.contains("invalid authorization format")
        || lower.contains("base url is invalid")
    {
        return "configuration_error";
    }
    if lower.contains("http 5")
        || lower.contains("internal_error")
        || lower.contains("service_unavailable")
        || lower.contains("service_timeout")
        || lower.contains("service temporarily unavailable")
    {
        return "service_error";
    }
    // Jina-specific 400/5xx tokens are handled above; the shared tail covers
    // the generic network-error phrases and the final fallback.
    classify_common_status(&lower)
}

fn build_endpoint(
    base_url: &str,
    query: &str,
    max_results: u32,
) -> Result<reqwest::Url, ProviderError> {
    let mut endpoint = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .map_err(|error| ProviderError::Retryable(format!("Jina base URL is invalid: {error}")))?;
    endpoint
        .query_pairs_mut()
        .append_pair("q", query)
        .append_pair("num", &max_results.to_string())
        .append_pair("type", "web");
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_encoded_search_endpoint() {
        let endpoint = build_endpoint("https://s.jina.ai", "rust async/await", 7).unwrap();
        assert_eq!(
            endpoint.as_str(),
            "https://s.jina.ai/?q=rust+async%2Fawait&num=7&type=web"
        );
    }

    #[test]
    fn omits_authorization_for_anonymous_search() {
        let client = reqwest::Client::new();
        let endpoint = build_endpoint("https://s.jina.ai", "rust", 1).unwrap();
        let request = build_request(&client, endpoint, "").build().unwrap();
        assert!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .is_none()
        );
    }

    #[test]
    fn sends_authorization_when_key_is_configured() {
        let client = reqwest::Client::new();
        let endpoint = build_endpoint("https://s.jina.ai", "rust", 1).unwrap();
        let request = build_request(&client, endpoint, "jina-key")
            .build()
            .unwrap();
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer jina-key")
        );
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
                    content_excerpt: None,
                    kind: SearchHitKind::WebPage,
                    published_at: None,
                    score: item.score,
                })
            })
            .collect();
        assert_eq!(hits[0].snippet, "from content");
        assert_eq!(hits[1].snippet, "from desc");
    }

    #[test]
    fn classifies_jina_key_statuses() {
        assert_eq!(
            classify_key_status("Jina HTTP 401: AUTH_INVALID_API_KEY"),
            "invalid_key"
        );
        assert_eq!(
            classify_key_status("Jina HTTP 403: AUTHZ_INSUFFICIENT_BALANCE"),
            "quota_exhausted"
        );
        assert_eq!(
            classify_key_status("Jina HTTP 429: RATE_REQUEST_LIMIT_EXCEEDED"),
            "rate_limited"
        );
    }

    #[test]
    fn maps_jina_reader_json() {
        let output = parse_extract_output(
            &serde_json::json!({
                "code": 200,
                "data": {
                    "title": "Example",
                    "url": "https://example.com/final",
                    "content": "# Example\n\nReader content"
                }
            }),
            "https://example.com",
        );
        assert_eq!(output.documents.len(), 1);
        assert!(output.documents[0].content.contains("Reader content"));
    }
}
