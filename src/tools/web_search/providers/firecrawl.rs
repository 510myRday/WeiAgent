use std::collections::HashSet;

use chrono::Datelike;
use serde_json::{Map, Value, json};

use super::support::{classify_common_status, send_checked, string_field, truncate_with};
use crate::tools::web_search::types::{
    ProviderError, ProviderExtractDocument, ProviderExtractFailure, ProviderExtractOutput,
    ProviderExtractRequest, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind, SearchMode,
    WebSearchHit,
};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_DEEP_RESULTS: u32 = 5;
const MAX_SNIPPET_CHARS: usize = 1_200;
const MAX_CONTENT_EXCERPT_CHARS: usize = 4_000;

pub async fn search(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
) -> Result<ProviderSearchOutput, ProviderError> {
    let endpoint = build_endpoint(base_url)?;
    let body = build_request_body(request);
    let builder = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body);
    let text = send_checked(
        builder,
        "Firecrawl",
        api_key,
        Some(MAX_RESPONSE_BYTES),
        320,
        "...",
    )
    .await?;

    let parsed: Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Firecrawl returned invalid JSON: {error}; body={}",
            truncate(&text, 200)
        ))
    })?;
    if parsed.get("success").and_then(Value::as_bool) == Some(false) {
        let detail = response_error(&parsed).unwrap_or_else(|| "search failed".to_string());
        return Err(ProviderError::Retryable(format!(
            "Firecrawl search failed: {}",
            truncate(&detail, 320)
        )));
    }

    Ok(ProviderSearchOutput {
        results: parse_results(&parsed, request.max_results as usize),
        ai_syntheses: Vec::new(),
    })
}

pub async fn extract(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    let endpoint = build_scrape_endpoint(base_url)?;
    let body = json!({
        "url": request.url,
        "formats": ["markdown"],
        "onlyMainContent": true,
        "onlyCleanContent": false,
        "timeout": request.timeout_ms.clamp(1_000, 300_000),
        "parsers": ["pdf"],
        "proxy": if request.hard_page { "auto" } else { "basic" },
        "storeInCache": false,
        "blockAds": true,
        "removeBase64Images": true,
    });
    let builder = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&body);
    let text = send_checked(
        builder,
        "Firecrawl scrape",
        api_key,
        Some(MAX_RESPONSE_BYTES),
        320,
        "...",
    )
    .await?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Firecrawl scrape returned invalid JSON: {error}; body={}",
            truncate(&text, 200)
        ))
    })?;
    if value.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(ProviderError::Retryable(
            response_error(&value).unwrap_or_else(|| "Firecrawl scrape failed".to_string()),
        ));
    }
    Ok(parse_extract_output(&value, &request.url))
}

fn build_endpoint(base_url: &str) -> Result<reqwest::Url, ProviderError> {
    let mut endpoint = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .map_err(|error| ProviderError::Fatal(format!("Firecrawl base URL is invalid: {error}")))?;
    let path = endpoint.path().trim_end_matches('/');
    let path = if path.ends_with("/search") {
        path.to_string()
    } else if path.ends_with("/v1") || path.ends_with("/v2") {
        format!("{path}/search")
    } else if path.is_empty() {
        "/v2/search".to_string()
    } else {
        format!("{path}/v2/search")
    };
    endpoint.set_path(&path);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn build_scrape_endpoint(base_url: &str) -> Result<reqwest::Url, ProviderError> {
    let mut endpoint = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .map_err(|error| ProviderError::Fatal(format!("Firecrawl base URL is invalid: {error}")))?;
    let path = endpoint.path().trim_end_matches('/');
    let path = if path.ends_with("/scrape") {
        path.to_string()
    } else if path.ends_with("/v1") || path.ends_with("/v2") {
        format!("{path}/scrape")
    } else if path.is_empty() {
        "/v2/scrape".to_string()
    } else {
        format!("{path}/v2/scrape")
    };
    endpoint.set_path(&path);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn parse_extract_output(value: &Value, requested_url: &str) -> ProviderExtractOutput {
    let mut output = ProviderExtractOutput::default();
    let data = value.get("data").unwrap_or(value);
    let Some(object) = data.as_object() else {
        output.failures.push(ProviderExtractFailure {
            requested_url: requested_url.to_string(),
            message: "Firecrawl returned no scrape document".to_string(),
        });
        return output;
    };
    let metadata = object.get("metadata").and_then(Value::as_object);
    let content = string_field(object, &["markdown", "content", "text"])
        .unwrap_or("")
        .trim();
    if content.is_empty() {
        output.failures.push(ProviderExtractFailure {
            requested_url: requested_url.to_string(),
            message: metadata
                .and_then(|value| string_field(value, &["error"]))
                .unwrap_or("Firecrawl returned empty markdown")
                .to_string(),
        });
        return output;
    }
    let final_url = metadata
        .and_then(|value| string_field(value, &["url", "sourceURL"]))
        .unwrap_or(requested_url)
        .to_string();
    output.documents.push(ProviderExtractDocument {
        requested_url: requested_url.to_string(),
        title: metadata
            .and_then(|value| string_field(value, &["title"]))
            .unwrap_or(&final_url)
            .to_string(),
        final_url,
        content: content.to_string(),
        content_format: "markdown".to_string(),
        published_at: metadata
            .and_then(|value| {
                string_field(
                    value,
                    &["publishedTime", "published_at", "publishedDate", "date"],
                )
            })
            .map(ToOwned::to_owned),
        ai_derived: false,
    });
    output
}

fn build_request_body(request: &ProviderSearchRequest) -> Value {
    let deep = request.mode == SearchMode::Deep;
    let limit = if deep {
        request.max_results.min(MAX_DEEP_RESULTS)
    } else {
        request.max_results
    };
    let mut sources = vec!["web"];
    if !deep && should_include_news(request) {
        sources.push("news");
    }
    let mut body = json!({
        "query": request.query,
        "limit": limit,
        "sources": sources,
        "highlights": true,
    });
    let Some(object) = body.as_object_mut() else {
        return body;
    };
    if !request.allowed_domains.is_empty() {
        object.insert("includeDomains".to_string(), json!(request.allowed_domains));
    } else if !request.blocked_domains.is_empty() {
        object.insert("excludeDomains".to_string(), json!(request.blocked_domains));
    }
    if let Some(days) = request.recency_days {
        object.insert("tbs".to_string(), Value::String(recency_filter(days)));
    }
    if deep {
        object.insert(
            "scrapeOptions".to_string(),
            json!({
                "formats": ["markdown"],
                "onlyMainContent": true,
            }),
        );
    }
    body
}

fn should_include_news(request: &ProviderSearchRequest) -> bool {
    if request.recency_days.is_some() {
        return true;
    }
    let query = request.query.to_lowercase();
    [
        "latest", "current", "today", "news", "breaking", "最新", "当前", "今天", "新闻", "近期",
        "刚刚",
    ]
    .iter()
    .any(|marker| query.contains(marker))
}

fn recency_filter(days: u32) -> String {
    let end = chrono::Utc::now().date_naive();
    let start = end
        .checked_sub_signed(chrono::Duration::days(i64::from(days.max(1))))
        .unwrap_or(end);
    format!(
        "sbd:1,cdr:1,cd_min:{}/{}/{},cd_max:{}/{}/{}",
        start.month(),
        start.day(),
        start.year(),
        end.month(),
        end.day(),
        end.year()
    )
}

fn parse_results(value: &Value, max_results: usize) -> Vec<WebSearchHit> {
    if max_results == 0 {
        return Vec::new();
    }
    let data = value.get("data").unwrap_or(value);
    let mut candidates = Vec::new();
    match data {
        Value::Array(items) => candidates.extend(items.iter()),
        Value::Object(object) => {
            let web = object
                .get("web")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let news = object
                .get("news")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if !web.is_empty() || !news.is_empty() {
                for index in 0..web.len().max(news.len()) {
                    if let Some(item) = web.get(index) {
                        candidates.push(item);
                    }
                    if let Some(item) = news.get(index) {
                        candidates.push(item);
                    }
                }
            } else if let Some(items) = object.get("results").and_then(Value::as_array) {
                candidates.extend(items.iter());
            }
        }
        _ => {}
    }
    if candidates.is_empty()
        && let Some(items) = value.get("results").and_then(Value::as_array)
    {
        candidates.extend(items.iter());
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(parse_hit)
        .filter(|hit| seen.insert(hit.url.clone()))
        .take(max_results)
        .collect()
}

fn parse_hit(value: &Value) -> Option<WebSearchHit> {
    let object = value.as_object()?;
    let metadata = object.get("metadata").and_then(Value::as_object);
    let url = string_field(object, &["url", "link"])
        .or_else(|| metadata.and_then(|value| string_field(value, &["sourceURL", "url"])))?
        .to_string();
    let title = string_field(object, &["title", "name"])
        .or_else(|| metadata.and_then(|value| string_field(value, &["title"])))
        .map(normalize_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url.clone());
    let snippet = highlight_text(object)
        .or_else(|| {
            string_field(object, &["description", "snippet", "content"]).map(ToOwned::to_owned)
        })
        .or_else(|| {
            metadata.and_then(|value| {
                string_field(value, &["description", "snippet"]).map(ToOwned::to_owned)
            })
        })
        .or_else(|| string_field(object, &["markdown"]).map(ToOwned::to_owned))
        .map(|value| truncate(&normalize_text(&value), MAX_SNIPPET_CHARS))
        .unwrap_or_default();
    let published_at = string_field(
        object,
        &["date", "publishedDate", "published_at", "publishedAt"],
    )
    .or_else(|| {
        metadata.and_then(|value| {
            string_field(
                value,
                &["date", "publishedDate", "published_at", "publishedAt"],
            )
        })
    })
    .map(ToOwned::to_owned);
    let content_excerpt = string_field(object, &["markdown"])
        .map(normalize_text)
        .filter(|value| !value.is_empty())
        .map(|value| truncate(&value, MAX_CONTENT_EXCERPT_CHARS));
    Some(WebSearchHit {
        title,
        url,
        snippet,
        content_excerpt,
        kind: SearchHitKind::WebPage,
        published_at,
        score: object.get("score").and_then(Value::as_f64),
    })
}

fn highlight_text(object: &Map<String, Value>) -> Option<String> {
    let value = object
        .get("highlights")
        .or_else(|| object.get("highlight"))?;
    match value {
        Value::String(value) => (!value.trim().is_empty()).then(|| value.clone()),
        Value::Array(values) => {
            let parts = values
                .iter()
                .filter_map(|value| match value {
                    Value::String(value) => Some(value.trim()),
                    Value::Object(object) => string_field(object, &["text", "content", "snippet"]),
                    _ => None,
                })
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(" ... "))
        }
        _ => None,
    }
}

fn response_error(value: &Value) -> Option<String> {
    for key in ["error", "message", "warning"] {
        let Some(value) = value.get(key) else {
            continue;
        };
        match value {
            Value::String(value) if !value.trim().is_empty() => return Some(value.clone()),
            Value::Object(object) => {
                if let Some(message) = string_field(object, &["message", "error", "detail"]) {
                    return Some(message.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub fn classify_key_status(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("http 401")
        || lower.contains("invalid api key")
        || lower.contains("missing api key")
        || lower.contains("unauthorized")
    {
        return "invalid_key";
    }
    if lower.contains("http 402")
        || lower.contains("insufficient credits")
        || lower.contains("credits exhausted")
        || lower.contains("payment required")
        || lower.contains("quota exceeded")
    {
        return "quota_exhausted";
    }
    if lower.contains("http 429")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
    {
        return "rate_limited";
    }
    if lower.contains("http 403") || lower.contains("forbidden") {
        return "permission_denied";
    }
    if lower.contains("http 400")
        || lower.contains("invalid request")
        || lower.contains("base url is invalid")
    {
        return "configuration_error";
    }
    classify_common_status(&lower)
}

fn normalize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Firecrawl historically truncates with an ASCII `...` suffix (asserted by
/// tests and surfaced in error messages), unlike the other providers' `…`.
fn truncate(value: &str, max: usize) -> String {
    truncate_with(value, max, "...")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(mode: SearchMode) -> ProviderSearchRequest {
        ProviderSearchRequest {
            query: "latest Rust release".to_string(),
            max_results: 4,
            mode,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            recency_days: None,
        }
    }

    #[test]
    fn builds_v2_search_endpoint() {
        assert_eq!(
            build_endpoint("https://api.firecrawl.dev/v2")
                .unwrap()
                .as_str(),
            "https://api.firecrawl.dev/v2/search"
        );
        assert_eq!(
            build_endpoint("https://example.com").unwrap().as_str(),
            "https://example.com/v2/search"
        );
        assert_eq!(
            build_endpoint("https://example.com/custom/v1/search")
                .unwrap()
                .as_str(),
            "https://example.com/custom/v1/search"
        );
    }

    #[test]
    fn current_search_requests_web_and_news() {
        let body = build_request_body(&request(SearchMode::Balanced));
        assert_eq!(body["sources"], json!(["web", "news"]));
        assert!(body.get("scrapeOptions").is_none());
    }

    #[test]
    fn deep_search_scrapes_a_bounded_web_result_set() {
        let mut request = request(SearchMode::Deep);
        request.max_results = 20;
        request.allowed_domains = vec!["rust-lang.org".to_string()];
        request.blocked_domains = vec!["example.com".to_string()];
        request.recency_days = Some(7);
        let body = build_request_body(&request);
        assert_eq!(body["limit"], 5);
        assert_eq!(body["sources"], json!(["web"]));
        assert_eq!(body["includeDomains"], json!(["rust-lang.org"]));
        assert!(body.get("excludeDomains").is_none());
        assert_eq!(body["scrapeOptions"]["formats"], json!(["markdown"]));
        assert!(body["tbs"].as_str().unwrap().contains("cdr:1"));
    }

    #[test]
    fn maps_grouped_web_and_news_results() {
        let parsed = json!({
            "success": true,
            "data": {
                "web": [
                    {"title":"Rust","url":"https://rust-lang.org","highlights":["Memory safety","No GC"]},
                    {"title":"Book","url":"https://doc.rust-lang.org/book","description":"The Rust book"}
                ],
                "news": [
                    {"title":"Rust news","url":"https://example.com/news","snippet":"New release","date":"2 hours ago"}
                ]
            }
        });
        let results = parse_results(&parsed, 3);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].url, "https://rust-lang.org");
        assert_eq!(results[0].snippet, "Memory safety ... No GC");
        assert_eq!(results[1].url, "https://example.com/news");
        assert_eq!(results[1].published_at.as_deref(), Some("2 hours ago"));
        assert_eq!(results[2].url, "https://doc.rust-lang.org/book");
    }

    #[test]
    fn maps_flat_scraped_results_and_uses_markdown() {
        let parsed = json!({
            "success": true,
            "data": [
                {
                    "url":"https://example.com/page",
                    "markdown":"# Heading\n\nFull page content",
                    "metadata":{"title":"Example page","publishedDate":"2026-07-23"}
                }
            ]
        });
        let results = parse_results(&parsed, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example page");
        assert_eq!(results[0].snippet, "# Heading Full page content");
        assert_eq!(
            results[0].content_excerpt.as_deref(),
            Some("# Heading Full page content")
        );
        assert_eq!(results[0].published_at.as_deref(), Some("2026-07-23"));
    }

    #[test]
    fn classifies_firecrawl_key_statuses() {
        assert_eq!(
            classify_key_status("Firecrawl HTTP 401: Unauthorized"),
            "invalid_key"
        );
        assert_eq!(
            classify_key_status("Firecrawl HTTP 402: Insufficient credits"),
            "quota_exhausted"
        );
        assert_eq!(
            classify_key_status("Firecrawl HTTP 429: Rate limit exceeded"),
            "rate_limited"
        );
    }

    #[test]
    fn builds_scrape_endpoint() {
        assert_eq!(
            build_scrape_endpoint("https://api.firecrawl.dev/v2")
                .unwrap()
                .as_str(),
            "https://api.firecrawl.dev/v2/scrape"
        );
    }

    #[test]
    fn maps_firecrawl_scrape_json() {
        let output = parse_extract_output(
            &json!({
                "success": true,
                "data": {
                    "markdown": "# Heading\n\nFull page",
                    "metadata": {
                        "title": "Heading",
                        "url": "https://example.com/final"
                    }
                }
            }),
            "https://example.com",
        );
        assert_eq!(output.documents.len(), 1);
        assert_eq!(output.documents[0].title, "Heading");
    }
}
