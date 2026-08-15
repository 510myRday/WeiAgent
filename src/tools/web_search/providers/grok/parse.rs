// Grok provider internals.
use serde_json::Value;
use std::collections::HashSet;

use super::super::grok_support::GrokError;
use super::super::support::truncate;
use super::constants::{MAX_FRAME_BYTES, MAX_SNIPPET_CHARS};
use crate::tools::web_search::types::{SearchHitKind, WebSearchHit};

#[derive(Debug, Default)]
pub(super) struct ParsedChat {
    pub(super) text: String,
    pub(super) sources: Vec<WebSearchHit>,
    pub(super) seen_urls: HashSet<String>,
    pub(super) error: Option<String>,
}

pub(super) fn consume_upstream(bytes: &[u8]) -> Result<ParsedChat, GrokError> {
    let mut parsed = ParsedChat::default();
    for frame in extract_json_objects(bytes) {
        apply_frame(&mut parsed, &frame)?;
    }
    Ok(parsed)
}

pub(super) fn extract_json_objects(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut objects = Vec::new();
    let mut frame = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for &value in bytes {
        if depth == 0 {
            if value != b'{' {
                continue;
            }
            frame.clear();
            depth = 1;
            in_string = false;
            escaped = false;
            frame.push(value);
            continue;
        }
        frame.push(value);
        if frame.len() > MAX_FRAME_BYTES {
            break;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if value == b'\\' {
                escaped = true;
            } else if value == b'"' {
                in_string = false;
            }
            continue;
        }
        match value {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    objects.push(frame.clone());
                }
            }
            _ => {}
        }
    }
    objects
}

pub(super) fn apply_frame(parsed: &mut ParsedChat, data: &[u8]) -> Result<(), GrokError> {
    let Ok(root) = serde_json::from_slice::<Value>(data) else {
        return Ok(());
    };
    let Some(root) = root.as_object() else {
        return Ok(());
    };
    if let Some(error) = root.get("error")
        && let Some(message) = error_message(error)
    {
        return Err(GrokError::Upstream(message));
    }
    let Some(result) = root.get("result").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(response) = result.get("response").and_then(Value::as_object) else {
        return Ok(());
    };
    if let Some(error) = response.get("error")
        && let Some(message) = error_message(error)
    {
        return Err(GrokError::Upstream(message));
    }
    collect_search_sources(parsed, &Value::Object(response.clone()));
    let thinking = response
        .get("isThinking")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tag = response
        .get("messageTag")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tag == "tool_usage_card" {
        return Ok(());
    }
    if let Some(token) = response.get("token").and_then(Value::as_str)
        && !thinking
        && (tag == "final" || tag.is_empty())
    {
        parsed.text.push_str(token);
    }
    if let Some(model_response) = response.get("modelResponse").and_then(Value::as_object) {
        if let Some(errors) = model_response.get("streamErrors").and_then(Value::as_array) {
            for item in errors {
                if let Some(message) = error_message(item) {
                    return Err(GrokError::Upstream(message));
                }
            }
        }
        collect_search_sources(parsed, &Value::Object(model_response.clone()));
        if let Some(message) = model_response.get("message").and_then(Value::as_str) {
            merge_model_text(&mut parsed.text, message);
        }
    }
    Ok(())
}

pub(super) fn merge_model_text(current: &mut String, message: &str) {
    if message.is_empty() {
        return;
    }
    if current.is_empty() {
        current.push_str(message);
        return;
    }
    if message.starts_with(current.as_str()) {
        current.push_str(&message[current.len()..]);
    }
}

pub(super) fn collect_search_sources(parsed: &mut ParsedChat, response: &Value) {
    collect_web_search_results(parsed, response.get("webSearchResults"));
    collect_web_search_results(parsed, response.get("citedWebSearchResults"));
    collect_x_search_results(parsed, response.get("xSearchResults"));
    collect_x_search_results(parsed, response.get("xposts"));
    collect_x_search_results(parsed, response.get("citedXposts"));
}

pub(super) fn collect_web_search_results(parsed: &mut ParsedChat, value: Option<&Value>) {
    let Some(value) = value else {
        return;
    };
    let items = match value {
        Value::Object(map) => map.get("results").and_then(Value::as_array),
        Value::Array(items) => Some(items),
        _ => None,
    };
    let Some(items) = items else {
        return;
    };
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(url) = obj.get("url").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if url.is_empty() {
            continue;
        }
        let title = obj
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(url);
        let snippet = obj
            .get("snippet")
            .or_else(|| obj.get("description"))
            .or_else(|| obj.get("content"))
            .or_else(|| obj.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        append_hit(
            parsed,
            WebSearchHit {
                title: title.to_string(),
                url: url.to_string(),
                snippet: truncate(snippet, MAX_SNIPPET_CHARS),
                content_excerpt: None,
                kind: SearchHitKind::GrokNativeSearchResult,
                published_at: None,
                score: None,
            },
        );
    }
}

pub(super) fn collect_x_search_results(parsed: &mut ParsedChat, value: Option<&Value>) {
    let Some(value) = value else {
        return;
    };
    let items = match value {
        Value::Object(map) => map.get("results").and_then(Value::as_array),
        Value::Array(items) => Some(items),
        _ => None,
    };
    let Some(items) = items else {
        return;
    };
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let username = obj
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let post_id = obj
            .get("postId")
            .or_else(|| obj.get("post_id"))
            .and_then(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        if username.is_empty() || post_id.is_empty() {
            continue;
        }
        let url = format!("https://x.com/{username}/status/{post_id}");
        let text = obj
            .get("text")
            .or_else(|| obj.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        append_hit(
            parsed,
            WebSearchHit {
                title: if text.is_empty() {
                    format!("@{username}")
                } else {
                    truncate(text, 120)
                },
                url,
                snippet: truncate(text, MAX_SNIPPET_CHARS),
                content_excerpt: None,
                kind: SearchHitKind::XSearchResult,
                published_at: None,
                score: None,
            },
        );
    }
}

pub(super) fn append_hit(parsed: &mut ParsedChat, hit: WebSearchHit) {
    if !parsed.seen_urls.insert(hit.url.clone()) {
        return;
    }
    parsed.sources.push(hit);
}

pub(super) fn parse_structured_hits(text: &str, max_results: u32) -> Vec<WebSearchHit> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let candidates = [
        trimmed.to_string(),
        strip_markdown_fence(trimmed),
        extract_json_array(trimmed).unwrap_or_default(),
    ];
    for candidate in candidates {
        if candidate.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(candidate.trim()) {
            let hits = value_to_hits(&value, max_results);
            if !hits.is_empty() {
                return hits;
            }
        }
    }
    Vec::new()
}

pub(super) fn value_to_hits(value: &Value, max_results: u32) -> Vec<WebSearchHit> {
    let mut hits = Vec::new();
    let mut seen = HashSet::new();

    let items = match value {
        Value::Array(items) => items.clone(),
        Value::Object(map) => map
            .get("sources")
            .or_else(|| map.get("results"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for item in items {
        if hits.len() as u32 >= max_results {
            break;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(url) = obj
            .get("url")
            .or_else(|| obj.get("link"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        let title = obj
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(url);
        let snippet = obj
            .get("snippet")
            .or_else(|| obj.get("content"))
            .or_else(|| obj.get("description"))
            .or_else(|| obj.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        hits.push(WebSearchHit {
            title: title.to_string(),
            url: url.to_string(),
            snippet: truncate(snippet, MAX_SNIPPET_CHARS),
            content_excerpt: None,
            kind: SearchHitKind::GrokModelProvidedUrl,
            published_at: None,
            score: None,
        });
    }
    hits
}

pub(super) fn parse_structured_answer(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidates = [
        trimmed.to_string(),
        strip_markdown_fence(trimmed),
        extract_json_object(trimmed).unwrap_or_default(),
    ];
    candidates.into_iter().find_map(|candidate| {
        let value = serde_json::from_str::<Value>(candidate.trim()).ok()?;
        value
            .get("answer")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|answer| !answer.is_empty())
            .map(str::to_string)
    })
}

pub(super) fn strip_markdown_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines();
    let _ = lines.next();
    let body: Vec<&str> = lines.collect();
    if body.is_empty() {
        return trimmed.to_string();
    }
    if body
        .last()
        .is_some_and(|line| line.trim().starts_with("```"))
    {
        return body[..body.len() - 1].join("\n");
    }
    body.join("\n")
}

pub(super) fn extract_json_array(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

pub(super) fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

pub(super) fn error_message(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        Value::Object(map) => {
            for key in ["message", "error", "detail", "code"] {
                if let Some(text) = map.get(key).and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
            None
        }
        _ => None,
    }
}
