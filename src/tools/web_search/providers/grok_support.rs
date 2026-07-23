use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wreq_util::Emulation;

const DEFAULT_FLARESOLVERR_TIMEOUT_MS: u64 = 90_000;
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
    // Grok chat via local proxy is slow; never use a sub-60s overall budget.
    let timeout = timeout.max(Duration::from_secs(90));
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
) -> Result<ClearanceBundle, String> {
    // Proxy here is ONLY for FlareSolverr's headless Chrome when opening target_url.
    // Use the proxy URL exactly as configured (no host rewriting).
    // WeiAgent's own Grok API traffic uses a separate proxy on the emulated wreq client.
    let flaresolverr_url = flaresolverr_url.trim().trim_end_matches('/');
    if flaresolverr_url.is_empty() {
        return Err("FlareSolverr URL is empty".to_string());
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
        .map_err(|error| format!("failed to build FlareSolverr client: {error}"))?;
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
                format!(
                    "FlareSolverr timed out after {}s waiting for {endpoint}. FS often needs 20-60s; check service is reachable.",
                    FLARESOLVERR_CLIENT_TIMEOUT.as_secs()
                )
            } else if error.is_connect() {
                format!("FlareSolverr connect failed for {endpoint}: {error}")
            } else {
                format!("FlareSolverr request failed for {endpoint}: {error}")
            }
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("FlareSolverr body failed: {error}"))?;
    if !status.is_success() {
        return Err(format_flaresolverr_http_error(status, &text));
    }

    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("FlareSolverr invalid JSON: {error}"))?;
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
        return Err(format!("FlareSolverr status={status_text}: {message}"));
    }

    let solution = value
        .get("solution")
        .and_then(Value::as_object)
        .ok_or_else(|| "FlareSolverr missing solution".to_string())?;
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
        return Err(format!(
            "FlareSolverr returned no cookies (proxy={}, target_http={http_status}). Proxy is passed exactly as configured.",
            fs_proxy.as_deref().unwrap_or("none")
        ));
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
) -> Result<FlareSolverrHttpResult, String> {
    let flaresolverr_url = flaresolverr_url.trim().trim_end_matches('/');
    if flaresolverr_url.is_empty() {
        return Err("FlareSolverr URL is empty".to_string());
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
        .map_err(|error| format!("failed to build FlareSolverr client: {error}"))?;

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
                format!(
                    "FlareSolverr POST timed out after {}s for {endpoint}",
                    FLARESOLVERR_CLIENT_TIMEOUT.as_secs()
                )
            } else {
                format!("FlareSolverr POST request failed for {endpoint}: {error}")
            }
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("FlareSolverr POST body failed: {error}"))?;
    if !status.is_success() {
        return Err(format_flaresolverr_http_error(status, &text));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("FlareSolverr POST invalid JSON: {error}"))?;
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
        return Err(format!("FlareSolverr POST status={status_text}: {message}"));
    }
    let solution = value
        .get("solution")
        .and_then(Value::as_object)
        .ok_or_else(|| "FlareSolverr POST missing solution".to_string())?;
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

fn format_flaresolverr_http_error(status: reqwest::StatusCode, text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("err_proxy_connection_failed") {
        return format!(
            "FlareSolverr HTTP {status}: cannot reach flaresolverr_proxy_url from FlareSolverr host. Proxy must be reachable FROM the FlareSolverr machine using the exact URL you filled. detail={}",
            truncate_for_error(text, 160)
        );
    }
    if lower.contains("err_connection_reset") || lower.contains("err_connection_timed_out") {
        return format!(
            "FlareSolverr HTTP {status}: FlareSolverr host cannot open target (likely blocked). Set flaresolverr_proxy_url so FlareSolverr browser goes through your proxy. detail={}",
            truncate_for_error(text, 160)
        );
    }
    format!(
        "FlareSolverr HTTP {status}: {}",
        truncate_for_error(text, 240)
    )
}

#[allow(dead_code)]
pub fn local_dynamic_statsig(method: &str, path: &str, meta_content: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let method = method.trim().to_ascii_uppercase();
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let material = format!("{method}\n{path}\n{meta_content}\n{ts}");
    let mut hasher = Sha256::new();
    hasher.update(material.as_bytes());
    hasher.update(b"|");
    hasher.update(meta_content.as_bytes());
    let digest = hasher.finalize();
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    // Compact browser-like token: version.ts.signature
    format!("4.{ts}.{signature}")
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

fn truncate_for_error(value: &str, max: usize) -> String {
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
    fn local_statsig_is_stable_shape() {
        let token = local_dynamic_statsig("POST", "/rest/app-chat/conversations/new", "meta");
        let parts: Vec<_> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "4");
        assert!(!parts[1].is_empty());
        assert!(!parts[2].is_empty());
    }

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
