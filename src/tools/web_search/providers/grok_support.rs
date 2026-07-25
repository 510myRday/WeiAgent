use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wreq_util::Emulation;

use crate::tools::web_search::types::{ProviderError, sanitize_error_message};

/// Structured error for the Grok provider (grok.rs + grok_support.rs internals).
///
/// Every variant stores the exact user-visible message so that `Display` (and
/// therefore logs and the final `ProviderError` text) stays byte-identical to
/// the previous `Result<_, String>` plumbing. The variant itself carries the
/// classification that used to be recovered via string matching.
#[derive(Debug, Clone)]
pub(crate) enum GrokError {
    /// Non-success HTTP status from Grok or a statsig signer endpoint.
    Http { message: String },
    /// Anti-bot / Cloudflare rejection (HTTP 403, code=7 payload, challenge HTML).
    AntiBot(String),
    /// Connection-level failure. `retryable` mirrors `is_transport_retryable`
    /// computed once on the full message at construction time.
    Transport { message: String, retryable: bool },
    /// Credential could not be parsed or is missing the SSO token.
    InvalidCredential(String),
    /// Error frame reported by the Grok chat stream.
    Upstream(String),
    /// Uncategorized; retry classification falls back to string matching.
    Other(String),
}

impl GrokError {
    /// Connection-level failure; retryability is derived from the message once.
    pub(crate) fn transport(message: impl Into<String>) -> Self {
        let message = message.into();
        let retryable = is_transport_retryable(&message);
        Self::Transport { message, retryable }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Http { message, .. } | Self::Transport { message, .. } => message,
            Self::AntiBot(message)
            | Self::InvalidCredential(message)
            | Self::Upstream(message)
            | Self::Other(message) => message,
        }
    }

    /// Should the caller retry this error as proxy/TLS flakiness?
    /// Variant-based first; `Http`/`Other` keep the string fallback so the
    /// retry decision matches the previous message-matching behavior.
    pub(crate) fn is_retryable_transport(&self) -> bool {
        match self {
            Self::Transport { retryable, .. } => *retryable,
            Self::AntiBot(_) | Self::InvalidCredential(_) | Self::Upstream(_) => false,
            Self::Http { message, .. } | Self::Other(message) => is_transport_retryable(message),
        }
    }

    /// Module boundary: every Grok failure is surfaced as a retryable
    /// `ProviderError` (as before), with the secret redacted from the text.
    pub(crate) fn into_provider_error(self, secret: &str) -> ProviderError {
        ProviderError::Retryable(sanitize_error_message(self.message(), secret))
    }
}

impl std::fmt::Display for GrokError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Proxy/TLS flakiness: Connect, timeout, reset, broken pipe, etc.
pub(crate) fn is_transport_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("client error (connect)")
        || lower.contains("error sending request")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("connection closed")
        || lower.contains("connection aborted")
        || lower.contains("broken pipe")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("temporarily unavailable")
        || lower.contains("network is unreachable")
        || lower.contains("dns error")
        || lower.contains("proxy")
            && (lower.contains("connect") || lower.contains("tunnel") || lower.contains("failed"))
}
pub fn normalize_optional_url(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| crate::domain::ip_proxies::proxy_url_with_remote_dns(value).into_owned())
}

/// Browser-fingerprint HTTP client (Chrome TLS/JA3/H2) for Grok API traffic.
pub fn build_emulated_client(
    timeout: Duration,
    proxy_url: Option<&str>,
) -> Result<wreq::Client, String> {
    // Cache by (timeout, proxy): the Chrome TLS emulation client is expensive
    // to build and per-search rebuilds would also discard its connection pool.
    type ClientCacheKey = (u64, Option<String>);
    static CACHE: OnceLock<Mutex<BTreeMap<ClientCacheKey, wreq::Client>>> = OnceLock::new();
    const MAX_CACHED_CLIENTS: usize = 16;
    // Grok chat via local proxy is slow; never use a sub-60s overall budget.
    let timeout = timeout.max(Duration::from_secs(90));
    let key = (
        timeout.as_millis() as u64,
        normalize_optional_url(proxy_url),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(client) = guard.get(&key)
    {
        return Ok(client.clone());
    }
    let client = build_emulated_client_uncached(timeout, key.1.as_deref())?;
    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= MAX_CACHED_CLIENTS {
            guard.clear();
        }
        guard.insert(key, client.clone());
    }
    Ok(client)
}

fn build_emulated_client_uncached(
    timeout: Duration,
    proxy_url: Option<&str>,
) -> Result<wreq::Client, String> {
    let mut builder = wreq::Client::builder()
        .emulation(Emulation::Chrome146)
        // Keep the warm-up and chat request on a reusable short-lived connection.
        // A zero-size idle pool makes the homepage and conversations/new requests
        // look like unrelated TLS sessions to Cloudflare.
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(45))
        .pool_max_idle_per_host(2)
        .pool_idle_timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(30))
        .redirect(wreq::redirect::Policy::limited(10));

    if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        // Pass proxy exactly as configured (no host rewrite).
        let proxy = wreq::Proxy::all(proxy_url)
            .map_err(|error| format!("invalid Grok proxy URL '{proxy_url}': {error}"))?;
        builder = builder.proxy(proxy);
        tracing::info!(%proxy_url, timeout_secs = timeout.as_secs(), "Grok API client will use proxy with Chrome146 TLS emulation");
    } else {
        tracing::info!(
            timeout_secs = timeout.as_secs(),
            "Grok API client using Chrome146 TLS emulation without explicit proxy"
        );
    }

    builder
        .build()
        .map_err(|error| format!("failed to build emulated Grok HTTP client: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_optional_url_trims() {
        assert_eq!(
            normalize_optional_url(Some("  http://127.0.0.1:8191  ")).as_deref(),
            Some("http://127.0.0.1:8191")
        );
        assert_eq!(
            normalize_optional_url(Some("socks5://proxy.test:1080")).as_deref(),
            Some("socks5h://proxy.test:1080")
        );
        assert_eq!(normalize_optional_url(Some("   ")), None);
    }
}
