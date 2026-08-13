//! Shared helpers for the web-search provider implementations.
//!
//! Everything here is pure plumbing (HTTP send/check boilerplate, string
//! truncation, JSON field lookup, common error classification) so each
//! provider file only keeps its provider-specific logic.

use crate::tools::web_search::types::{ProviderError, sanitize_error_message};

/// Upper bound applied to extract/scrape response bodies before parsing.
pub const MAX_EXTRACT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub use crate::textutil::{truncate_chars as truncate, truncate_with};

/// Returns the first non-empty trimmed string value found under `keys`.
pub fn string_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// Common tail of every provider's `classify_key_status`: 403/400/5xx and
/// network-level failures. Callers handle provider-specific 401/402/429
/// (and similar) branches first, then fall through to this.
pub fn classify_common_status(lower: &str) -> &'static str {
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

/// Sends a fully prepared request and returns the response body as text,
/// mapping transport errors, oversized bodies, and non-2xx statuses to
/// `ProviderError::Retryable` with sanitized, `label`-prefixed messages.
///
/// * `label` — message prefix, e.g. "Tavily" / "Exa contents" / "Jina Reader".
/// * `max_response_bytes` — when `Some`, the body is read as bytes, size
///   checked, then decoded lossily as UTF-8 (matching the previous per-file
///   behavior); when `None`, `Response::text()` is used.
/// * `status_preview_chars` / `ellipsis` — how much of an error body to quote
///   in the HTTP-status error message and which truncation suffix to use.
pub async fn send_checked(
    builder: reqwest::RequestBuilder,
    label: &str,
    api_key: &str,
    max_response_bytes: Option<usize>,
    status_preview_chars: usize,
    ellipsis: &str,
) -> Result<String, ProviderError> {
    let response = builder.send().await.map_err(|error| {
        ProviderError::Retryable(sanitize_error_message(
            &format!("{label} request failed: {error}"),
            api_key,
        ))
    })?;
    let status = response.status();
    let text = if let Some(max_bytes) = max_response_bytes {
        let bytes = response.bytes().await.map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("{label} response body error: {error}"),
                api_key,
            ))
        })?;
        if bytes.len() > max_bytes {
            return Err(ProviderError::Retryable(format!(
                "{label} response exceeds {max_bytes} bytes"
            )));
        }
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        response.text().await.map_err(|error| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("{label} response body error: {error}"),
                api_key,
            ))
        })?
    };
    if !status.is_success() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!(
                "{label} HTTP {status}: {}",
                truncate_with(&text, status_preview_chars, ellipsis)
            ),
            api_key,
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_appends_ellipsis_only_when_trimmed() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate_with("abcdef", 3, "..."), "abc...");
    }

    #[test]
    fn string_field_prefers_first_non_empty_key() {
        let value = serde_json::json!({"a": "  ", "b": " hit ", "c": "other"});
        let object = value.as_object().unwrap();
        assert_eq!(string_field(object, &["a", "b", "c"]), Some("hit"));
        assert_eq!(string_field(object, &["missing"]), None);
    }

    #[test]
    fn classifies_common_statuses() {
        assert_eq!(
            classify_common_status("http 403 forbidden"),
            "permission_denied"
        );
        assert_eq!(
            classify_common_status("http 400 invalid request"),
            "configuration_error"
        );
        assert_eq!(
            classify_common_status("http 503 bad gateway"),
            "service_error"
        );
        assert_eq!(
            classify_common_status("request failed: dns"),
            "network_error"
        );
        assert_eq!(classify_common_status("something else"), "error");
    }
}
