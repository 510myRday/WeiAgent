use serde::Deserialize;
use serde_json::{Value, json};

use super::support::{
    MAX_EXTRACT_RESPONSE_BYTES, classify_common_status, send_checked, string_field, truncate,
};
use crate::tools::web_search::types::{
    ProviderError, ProviderExtractDocument, ProviderExtractFailure, ProviderExtractOutput,
    ProviderExtractRequest, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind,
    WebSearchHit,
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

    let builder = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body);
    let text = send_checked(builder, "Tavily", api_key, None, 240, "…").await?;

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
                content_excerpt: None,
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
    let endpoint = format!("{}/extract", base_url.trim_end_matches('/'));
    let body = build_extract_body(request);
    let builder = client
        .post(&endpoint)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&body);
    let text = send_checked(
        builder,
        "Tavily extract",
        api_key,
        Some(MAX_EXTRACT_RESPONSE_BYTES),
        240,
        "…",
    )
    .await?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Tavily extract returned invalid JSON: {error}; body={}",
            truncate(&text, 160)
        ))
    })?;
    Ok(parse_extract_output(&value, &request.url))
}

fn build_extract_body(request: &ProviderExtractRequest) -> Value {
    let mut body = json!({
        "urls": [request.url],
        "extract_depth": if request.hard_page { "advanced" } else { "basic" },
        "format": "markdown",
        "timeout": (request.timeout_ms as f64 / 1000.0).clamp(1.0, 60.0),
        "include_usage": true,
    });
    if !request.query.trim().is_empty() {
        let object = body.as_object_mut().expect("extract body is an object");
        object.insert(
            "query".to_string(),
            Value::String(request.query.trim().to_string()),
        );
        object.insert("chunks_per_source".to_string(), Value::from(5));
    }
    body
}

fn parse_extract_output(value: &serde_json::Value, requested_url: &str) -> ProviderExtractOutput {
    let mut output = ProviderExtractOutput::default();
    if let Some(results) = value.get("results").and_then(serde_json::Value::as_array) {
        for result in results {
            let Some(object) = result.as_object() else {
                continue;
            };
            let content = string_field(object, &["raw_content", "content", "markdown"])
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                continue;
            }
            let final_url = string_field(object, &["url"])
                .unwrap_or(requested_url)
                .to_string();
            output.documents.push(ProviderExtractDocument {
                requested_url: requested_url.to_string(),
                title: string_field(object, &["title"])
                    .unwrap_or(&final_url)
                    .to_string(),
                final_url,
                content: content.to_string(),
                content_format: "markdown".to_string(),
                published_at: string_field(
                    object,
                    &["published_at", "publishedAt", "published_date"],
                )
                .map(ToOwned::to_owned),
            });
        }
    }
    if let Some(failures) = value
        .get("failed_results")
        .and_then(serde_json::Value::as_array)
    {
        for failure in failures {
            match failure {
                serde_json::Value::String(message) => {
                    output.failures.push(ProviderExtractFailure {
                        requested_url: requested_url.to_string(),
                        message: message.clone(),
                    })
                }
                serde_json::Value::Object(object) => {
                    output.failures.push(ProviderExtractFailure {
                        requested_url: string_field(object, &["url"])
                            .unwrap_or(requested_url)
                            .to_string(),
                        message: string_field(object, &["error", "message", "detail"])
                            .unwrap_or("Tavily extraction failed")
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    output
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

    #[test]
    fn maps_tavily_extract_json() {
        let output = parse_extract_output(
            &serde_json::json!({
                "results": [{
                    "url": "https://example.com/final",
                    "raw_content": "# Heading\n\nArticle body"
                }],
                "failed_results": []
            }),
            "https://example.com",
        );
        assert_eq!(output.documents.len(), 1);
        assert_eq!(output.documents[0].content_format, "markdown");
        assert!(output.documents[0].content.contains("Article body"));
    }

    #[test]
    fn extract_query_enables_relevant_chunk_reranking() {
        let body = build_extract_body(&ProviderExtractRequest {
            url: "https://example.com".to_string(),
            query: "specific research question".to_string(),
            hard_page: false,
            timeout_ms: 45_000,
        });
        assert_eq!(body["query"], "specific research question");
        assert_eq!(body["chunks_per_source"], 5);
        assert_eq!(body["extract_depth"], "basic");
    }
}
