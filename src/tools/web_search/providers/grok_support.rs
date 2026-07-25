use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wreq_util::Emulation;

use crate::tools::web_search::types::{ProviderError, sanitize_error_message};

use super::support::truncate as truncate_for_error;

const DEFAULT_FLARESOLVERR_TIMEOUT_MS: u64 = 90_000;

/// Structured error for the Grok provider (grok.rs + grok_support.rs internals).
///
/// Every variant stores the exact user-visible message so that `Display` (and
/// therefore logs and the final `ProviderError` text) stays byte-identical to
/// the previous `Result<_, String>` plumbing. The variant itself carries the
/// classification that used to be recovered via string matching.
#[derive(Debug, Clone)]
pub(crate) enum GrokError {
    /// Non-success HTTP status from Grok or a statsig signer endpoint.
    Http {
        /// Kept for structured matching/diagnostics; classification currently
        /// treats every Grok error as retryable, so nothing reads it yet.
        #[allow(dead_code)]
        status: u16,
        message: String,
    },
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
const FLARESOLVERR_CLIENT_TIMEOUT: Duration = Duration::from_secs(120);
const CLEARANCE_CACHE_TTL: Duration = Duration::from_secs(45 * 60);

#[derive(Debug, Clone)]
pub struct ClearanceBundle {
    pub cookies: BTreeMap<String, String>,
    pub user_agent: Option<String>,
}

struct ClearanceCacheEntry {
    bundle: ClearanceBundle,
    expires_at: Instant,
}

fn clearance_cache() -> &'static Mutex<BTreeMap<String, ClearanceCacheEntry>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, ClearanceCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn normalize_optional_url(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Normalize FlareSolverr base URL. Host-only values default to port 8191.
pub fn normalize_flaresolverr_url(value: Option<&str>) -> Option<String> {
    let raw = normalize_optional_url(value)?;
    let with_scheme = if raw.contains("://") {
        raw
    } else {
        format!("http://{raw}")
    };
    let trimmed = with_scheme.trim_end_matches('/').to_string();
    let Ok(url) = reqwest::Url::parse(&trimmed) else {
        return Some(trimmed);
    };
    if url.port().is_some() {
        return Some(trimmed);
    }
    let host = url.host_str().unwrap_or("").to_string();
    if host.is_empty() {
        return Some(trimmed);
    }
    // http://192.168.2.123 or http://192.168.2.123/ -> http://192.168.2.123:8191
    if url.path() == "/" || url.path().is_empty() {
        return Some(format!("{}://{}:8191", url.scheme(), host));
    }
    Some(trimmed)
}

/// Browser-fingerprint HTTP client (Chrome TLS/JA3/H2) for Grok API traffic.
/// FlareSolverr remains only for cf_clearance; this client is for conversations/new.
pub fn build_emulated_client(
    timeout: Duration,
    proxy_url: Option<&str>,
) -> Result<wreq::Client, String> {
    // Cache by (timeout, proxy): the Chrome TLS emulation client is expensive
    // to build and per-search rebuilds would also discard its connection pool.
    static CACHE: OnceLock<Mutex<BTreeMap<(u64, Option<String>), wreq::Client>>> = OnceLock::new();
    const MAX_CACHED_CLIENTS: usize = 16;
    // Grok chat via local proxy is slow; never use a sub-60s overall budget.
    let timeout = timeout.max(Duration::from_secs(90));
    let key = (
        timeout.as_millis() as u64,
        proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
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

pub async fn resolve_clearance(
    control_client: &reqwest::Client,
    flaresolverr_url: &str,
    target_url: &str,
    flaresolverr_proxy_url: Option<&str>,
    force_refresh: bool,
) -> Result<ClearanceBundle, GrokError> {
    // Proxy here is ONLY for FlareSolverr's headless Chrome when opening target_url.
    // Use the proxy URL exactly as configured (no host rewriting).
    // WeiAgent's own Grok API traffic uses a separate proxy on the emulated wreq client.
    let flaresolverr_url = flaresolverr_url.trim().trim_end_matches('/');
    if flaresolverr_url.is_empty() {
        return Err(GrokError::Other("FlareSolverr URL is empty".to_string()));
    }
    let fs_proxy = flaresolverr_proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string());
    let cache_key = format!(
        "{flaresolverr_url}\0{target_url}\0{}",
        fs_proxy.as_deref().unwrap_or("")
    );
    if !force_refresh && let Some(cached) = get_cached_clearance(&cache_key) {
        return Ok(cached);
    }

    let endpoint = if flaresolverr_url.ends_with("/v1") {
        flaresolverr_url.to_string()
    } else {
        format!("{flaresolverr_url}/v1")
    };

    let mut payload = json!({
        "cmd": "request.get",
        "url": target_url,
        "maxTimeout": DEFAULT_FLARESOLVERR_TIMEOUT_MS,
    });
    if let Some(proxy) = fs_proxy.as_deref() {
        payload
            .as_object_mut()
            .expect("object")
            .insert("proxy".to_string(), json!({ "url": proxy }));
        tracing::info!(%proxy, "FlareSolverr will open target via proxy");
    }

    // Use a dedicated long-timeout client. The shared web_search client is often 15s,
    // but FlareSolverr regularly needs 20-60s to open grok.com through a proxy.
    let fs_client = reqwest::Client::builder()
        .timeout(FLARESOLVERR_CLIENT_TIMEOUT)
        .connect_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(2)
        .build()
        .map_err(|error| {
            GrokError::Other(format!("failed to build FlareSolverr client: {error}"))
        })?;
    let _ = control_client; // keep signature for callers; FS call is independent

    let response = fs_client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GrokError::transport(format!(
                    "FlareSolverr timed out after {}s waiting for {endpoint}. FS often needs 20-60s; check service is reachable.",
                    FLARESOLVERR_CLIENT_TIMEOUT.as_secs()
                ))
            } else if error.is_connect() {
                GrokError::transport(format!(
                    "FlareSolverr connect failed for {endpoint}: {error}"
                ))
            } else {
                GrokError::transport(format!(
                    "FlareSolverr request failed for {endpoint}: {error}"
                ))
            }
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| GrokError::transport(format!("FlareSolverr body failed: {error}")))?;
    if !status.is_success() {
        return Err(format_flaresolverr_http_error(status, &text));
    }

    let value: Value = serde_json::from_str(&text)
        .map_err(|error| GrokError::Other(format!("FlareSolverr invalid JSON: {error}")))?;
    let status_text = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if status_text != "ok" {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(GrokError::Other(format!(
            "FlareSolverr status={status_text}: {message}"
        )));
    }

    let solution = value
        .get("solution")
        .and_then(Value::as_object)
        .ok_or_else(|| GrokError::Other("FlareSolverr missing solution".to_string()))?;
    let mut cookies = BTreeMap::new();
    if let Some(items) = solution.get("cookies").and_then(Value::as_array) {
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let value = obj
                .get("value")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let (Some(name), Some(value)) = (name, value) {
                cookies.insert(name.to_string(), value.to_string());
            }
        }
    }
    if cookies.is_empty() {
        let http_status = solution
            .get("status")
            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
            .unwrap_or(-1);
        return Err(GrokError::Other(format!(
            "FlareSolverr returned no cookies (proxy={}, target_http={http_status}). Proxy is passed exactly as configured.",
            fs_proxy.as_deref().unwrap_or("none")
        )));
    }
    if !cookies
        .keys()
        .any(|k| k.eq_ignore_ascii_case("cf_clearance"))
    {
        tracing::warn!(
            cookies = %cookies.keys().cloned().collect::<Vec<_>>().join(","),
            "FlareSolverr cookies missing cf_clearance"
        );
    } else {
        tracing::info!(
            cookies = %cookies.keys().cloned().collect::<Vec<_>>().join(","),
            "FlareSolverr clearance cookies ready"
        );
    }
    let user_agent = solution
        .get("userAgent")
        .or_else(|| solution.get("user_agent"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let bundle = ClearanceBundle {
        cookies,
        user_agent,
    };
    put_cached_clearance(&cache_key, &bundle);
    Ok(bundle)
}

#[derive(Debug, Clone)]
pub struct FlareSolverrHttpResult {
    pub status: u16,
    pub cookies: BTreeMap<String, String>,
    /// Browser UA that minted the response (kept for future header binding).
    #[allow(dead_code)]
    pub user_agent: Option<String>,
    pub body: String,
}

/// Browser-level POST through FlareSolverr (correct TLS fingerprint).
pub async fn flaresolverr_request_post(
    flaresolverr_url: &str,
    target_url: &str,
    flaresolverr_proxy_url: Option<&str>,
    headers: &BTreeMap<String, String>,
    body: &str,
    max_timeout_ms: u64,
) -> Result<FlareSolverrHttpResult, GrokError> {
    let flaresolverr_url = flaresolverr_url.trim().trim_end_matches('/');
    if flaresolverr_url.is_empty() {
        return Err(GrokError::Other("FlareSolverr URL is empty".to_string()));
    }
    let endpoint = if flaresolverr_url.ends_with("/v1") {
        flaresolverr_url.to_string()
    } else {
        format!("{flaresolverr_url}/v1")
    };
    let fs_proxy = flaresolverr_proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string());

    let mut payload = json!({
        "cmd": "request.post",
        "url": target_url,
        "maxTimeout": max_timeout_ms.max(30_000),
        "postData": body,
    });
    if let Some(proxy) = fs_proxy.as_deref() {
        payload
            .as_object_mut()
            .expect("object")
            .insert("proxy".to_string(), json!({ "url": proxy }));
    }
    if !headers.is_empty() {
        let mut header_map = serde_json::Map::new();
        for (k, v) in headers {
            header_map.insert(k.clone(), Value::String(v.clone()));
        }
        payload
            .as_object_mut()
            .expect("object")
            .insert("headers".to_string(), Value::Object(header_map));
    }

    let fs_client = reqwest::Client::builder()
        .timeout(FLARESOLVERR_CLIENT_TIMEOUT)
        .connect_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(2)
        .build()
        .map_err(|error| {
            GrokError::Other(format!("failed to build FlareSolverr client: {error}"))
        })?;

    tracing::info!(
        %endpoint,
        %target_url,
        proxy = fs_proxy.as_deref().unwrap_or("none"),
        "FlareSolverr browser POST"
    );

    let response = fs_client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GrokError::transport(format!(
                    "FlareSolverr POST timed out after {}s for {endpoint}",
                    FLARESOLVERR_CLIENT_TIMEOUT.as_secs()
                ))
            } else {
                GrokError::transport(format!(
                    "FlareSolverr POST request failed for {endpoint}: {error}"
                ))
            }
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| GrokError::transport(format!("FlareSolverr POST body failed: {error}")))?;
    if !status.is_success() {
        return Err(format_flaresolverr_http_error(status, &text));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| GrokError::Other(format!("FlareSolverr POST invalid JSON: {error}")))?;
    let status_text = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if status_text != "ok" {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(GrokError::Other(format!(
            "FlareSolverr POST status={status_text}: {message}"
        )));
    }
    let solution = value
        .get("solution")
        .and_then(Value::as_object)
        .ok_or_else(|| GrokError::Other("FlareSolverr POST missing solution".to_string()))?;
    let mut cookies = BTreeMap::new();
    if let Some(items) = solution.get("cookies").and_then(Value::as_array) {
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let value = obj
                .get("value")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty());
            if let (Some(name), Some(value)) = (name, value) {
                cookies.insert(name.to_string(), value.to_string());
            }
        }
    }
    let http_status = solution
        .get("status")
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n as u64)))
        .unwrap_or(0) as u16;
    let body = solution
        .get("response")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let user_agent = solution
        .get("userAgent")
        .or_else(|| solution.get("user_agent"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned);
    Ok(FlareSolverrHttpResult {
        status: http_status,
        cookies,
        user_agent,
        body,
    })
}

fn format_flaresolverr_http_error(status: reqwest::StatusCode, text: &str) -> GrokError {
    let lower = text.to_ascii_lowercase();
    if lower.contains("err_proxy_connection_failed") {
        return GrokError::Http {
            status: status.as_u16(),
            message: format!(
                "FlareSolverr HTTP {status}: cannot reach flaresolverr_proxy_url from FlareSolverr host. Proxy must be reachable FROM the FlareSolverr machine using the exact URL you filled. detail={}",
                truncate_for_error(text, 160)
            ),
        };
    }
    if lower.contains("err_connection_reset") || lower.contains("err_connection_timed_out") {
        return GrokError::Http {
            status: status.as_u16(),
            message: format!(
                "FlareSolverr HTTP {status}: FlareSolverr host cannot open target (likely blocked). Set flaresolverr_proxy_url so FlareSolverr browser goes through your proxy. detail={}",
                truncate_for_error(text, 160)
            ),
        };
    }
    GrokError::Http {
        status: status.as_u16(),
        message: format!(
            "FlareSolverr HTTP {status}: {}",
            truncate_for_error(text, 240)
        ),
    }
}

fn get_cached_clearance(key: &str) -> Option<ClearanceBundle> {
    let guard = clearance_cache().lock().ok()?;
    let entry = guard.get(key)?;
    if Instant::now() < entry.expires_at {
        Some(entry.bundle.clone())
    } else {
        None
    }
}

fn put_cached_clearance(key: &str, bundle: &ClearanceBundle) {
    if let Ok(mut guard) = clearance_cache().lock() {
        guard.insert(
            key.to_string(),
            ClearanceCacheEntry {
                bundle: bundle.clone(),
                expires_at: Instant::now() + CLEARANCE_CACHE_TTL,
            },
        );
    }
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
        assert_eq!(normalize_optional_url(Some("   ")), None);
    }

    #[test]
    fn normalize_flaresolverr_adds_default_port() {
        assert_eq!(
            normalize_flaresolverr_url(Some("http://192.168.2.123")).as_deref(),
            Some("http://192.168.2.123:8191")
        );
        assert_eq!(
            normalize_flaresolverr_url(Some("192.168.2.123")).as_deref(),
            Some("http://192.168.2.123:8191")
        );
        assert_eq!(
            normalize_flaresolverr_url(Some("http://192.168.2.123:8191")).as_deref(),
            Some("http://192.168.2.123:8191")
        );
    }
}
