use serde_json::{Value, json};

use super::support::{MAX_EXTRACT_RESPONSE_BYTES, classify_common_status, send_checked, truncate};
use crate::tools::web_search::types::{
    ProviderError, ProviderExtractDocument, ProviderExtractFailure, ProviderExtractOutput,
    ProviderExtractRequest, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind,
    WebSearchHit,
};

const PUBLIC_APP_TITLE: &str = "WeiAgent";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub async fn search(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
) -> Result<ProviderSearchOutput, ProviderError> {
    let endpoint = endpoint(base_url, api_key, "search")?;
    let mut body = json!({
        "query": request.query,
        "max_results": request.max_results.clamp(1, 50),
        "snippet_max_length": 1200,
    });
    if let Some(object) = body.as_object_mut() {
        if let Some(site) = request
            .allowed_domains
            .first()
            .filter(|value| !value.trim().is_empty())
        {
            object.insert("site".to_string(), Value::String(site.clone()));
        }
        if let Some(days) = request.recency_days {
            object.insert(
                "published_after".to_string(),
                Value::String(format!("{days}d")),
            );
        }
    }
    let builder = add_auth(
        client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&body),
        api_key,
    );
    let text = send_checked(
        builder,
        "Keenable",
        api_key,
        Some(MAX_RESPONSE_BYTES),
        320,
        "…",
    )
    .await?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Keenable returned invalid JSON: {error}; body={}",
            truncate(&text, 200)
        ))
    })?;
    Ok(ProviderSearchOutput {
        results: parse_results(&value, request.max_results as usize),
        ai_syntheses: Vec::new(),
    })
}

pub async fn extract(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    let mut endpoint = reqwest::Url::parse(base_url.trim_end_matches('/')).map_err(|error| {
        ProviderError::Retryable(format!("Keenable base URL is invalid: {error}"))
    })?;
    let suffix = if api_key.trim().is_empty() {
        "/fetch/public"
    } else {
        "/fetch"
    };
    endpoint.set_path(&format!(
        "{}/{suffix}",
        endpoint.path().trim_end_matches('/')
    ));
    endpoint.query_pairs_mut().append_pair("url", &request.url);
    endpoint.query_pairs_mut().append_pair("max_chars", "50000");
    if request.hard_page {
        endpoint.query_pairs_mut().append_pair("live", "true");
    }
    let builder = add_auth(client.get(endpoint), api_key);
    let text = send_checked(
        builder,
        "Keenable fetch",
        api_key,
        Some(MAX_EXTRACT_RESPONSE_BYTES),
        320,
        "…",
    )
    .await?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        ProviderError::Retryable(format!(
            "Keenable fetch returned invalid JSON: {error}; body={}",
            truncate(&text, 200)
        ))
    })?;
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut output = ProviderExtractOutput::default();
    if content.is_empty() {
        output.failures.push(ProviderExtractFailure {
            requested_url: request.url.clone(),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Keenable returned empty content")
                .to_string(),
        });
    } else {
        output.documents.push(ProviderExtractDocument {
            requested_url: request.url.clone(),
            final_url: value
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or(&request.url)
                .to_string(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(&request.url)
                .to_string(),
            content: content.to_string(),
            content_format: "markdown".to_string(),
            published_at: value.get("published_at").map(format_published_at),
            ai_derived: false,
        });
    }
    Ok(output)
}

fn endpoint(base_url: &str, api_key: &str, suffix: &str) -> Result<reqwest::Url, ProviderError> {
    let mut endpoint = reqwest::Url::parse(base_url.trim_end_matches('/')).map_err(|error| {
        ProviderError::Retryable(format!("Keenable base URL is invalid: {error}"))
    })?;
    let public_suffix = if api_key.trim().is_empty() {
        "/public"
    } else {
        ""
    };
    endpoint.set_path(&format!(
        "{}/{suffix}{public_suffix}",
        endpoint.path().trim_end_matches('/')
    ));
    Ok(endpoint)
}

fn add_auth(builder: reqwest::RequestBuilder, api_key: &str) -> reqwest::RequestBuilder {
    if api_key.trim().is_empty() {
        builder.header("X-Keenable-Title", PUBLIC_APP_TITLE)
    } else {
        builder.header("X-API-Key", api_key.trim())
    }
}

fn parse_results(value: &Value, max_results: usize) -> Vec<WebSearchHit> {
    value
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let object = item.as_object()?;
            let url = object.get("url").and_then(Value::as_str)?.trim();
            if url.is_empty() {
                return None;
            }
            Some(WebSearchHit {
                title: object
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(url)
                    .to_string(),
                url: url.to_string(),
                snippet: object
                    .get("snippet")
                    .and_then(Value::as_str)
                    .or_else(|| object.get("description").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string(),
                content_excerpt: None,
                kind: SearchHitKind::WebPage,
                published_at: object.get("published_at").map(format_published_at),
                score: None,
            })
        })
        .take(max_results)
        .collect()
}

fn format_published_at(value: &Value) -> String {
    value.as_str().map(ToOwned::to_owned).unwrap_or_else(|| {
        value
            .as_i64()
            .map(|seconds| seconds.to_string())
            .unwrap_or_default()
    })
}

pub fn classify_key_status(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("http 401")
        || lower.contains("missing or invalid api key")
        || lower.contains("invalid api key")
    {
        return "invalid_key";
    }
    if lower.contains("http 402")
        || lower.contains("no credits")
        || (lower.contains("credits") && lower.contains("spent"))
    {
        return "quota_exhausted";
    }
    if lower.contains("http 429")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
    {
        return "rate_limited";
    }
    classify_common_status(&lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_public_search_endpoint_without_a_key() {
        assert_eq!(
            endpoint("https://api.keenable.ai/v1", "", "search")
                .unwrap()
                .as_str(),
            "https://api.keenable.ai/v1/search/public"
        );
    }

    #[test]
    fn maps_search_results() {
        let results = parse_results(
            &json!({"results":[{"title":"Rust","url":"https://rust-lang.org","description":"Language"}]}),
            5,
        );
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].snippet, "Language");
    }
}
