// Grok provider internals.
use super::super::grok_support::{GrokError, is_transport_retryable};
use crate::tools::web_search::types::ProviderError;

pub(super) fn classify_upstream_error(error: &str, api_key: &str) -> ProviderError {
    GrokError::Upstream(format!("Grok upstream error: {error}")).into_provider_error(api_key)
}

pub fn classify_key_status(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("blocked-user")
        || lower.contains("user is blocked")
        || lower.contains("account is blocked")
        || lower.contains("account banned")
        || lower.contains("account suspended")
    {
        return "officially_blocked";
    }
    if lower.contains("quota")
        || lower.contains("usage limit")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("limit exceeded")
        || lower.contains("http 429")
    {
        return "quota_limited";
    }
    if lower.contains("http 401")
        || lower.contains("unauthorized")
        || lower.contains("invalid token")
    {
        return "unauthorized";
    }
    if is_anti_bot_message(message) {
        return "anti_bot";
    }
    if is_transport_retryable(message) {
        return "network_error";
    }
    "error"
}

pub(super) fn is_anti_bot_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("anti-bot")
        || lower.contains("antibot")
        || lower.contains("\"code\":7")
        || lower.contains("request rejected by anti-bot")
}
