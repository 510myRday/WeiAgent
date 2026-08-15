// Grok provider internals.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::super::grok_support::GrokError;
use super::super::support::truncate;
use super::constants::{
    DEFAULT_USER_AGENT, STATSIG_CACHE_TTL, X0_FALLBACK_STATSIG_ID, usable_statsig_id,
    valid_statsig_id,
};
use super::credentials::{GrokCredential, build_sso_cookie, is_allowed_extra_cookie};

pub(super) struct StatsigCacheEntry {
    pub(super) value: String,
    pub(super) expires_at: Instant,
}

pub(super) fn statsig_cache() -> &'static Mutex<BTreeMap<String, StatsigCacheEntry>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, StatsigCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) async fn resolve_statsig_id(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base: &str,
    cred: &GrokCredential,
    endpoint: &str,
    force_refresh: bool,
    proxy_url: Option<&str>,
) -> Result<Option<String>, GrokError> {
    // 1) Explicit credential override (manual 70-byte or x0 string in JSON/cookie config).
    if let Some(manual) = cred
        .statsig_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if usable_statsig_id(manual) {
            return Ok(Some(manual.to_string()));
        }
        tracing::warn!(
            "Grok credential x_statsig_id is not usable (need 70-byte base64 or x0 fallback); ignoring"
        );
    }

    let path = url_path(endpoint);
    let cache_key = format!("{base}\0POST\0{path}");
    if !force_refresh && let Some(cached) = get_cached_statsig(&cache_key) {
        if usable_statsig_id(&cached) {
            return Ok(Some(cached));
        }
        invalidate_statsig_cache(base, endpoint);
    }

    // 2) Optional explicit local/remote signer ONLY if credential sets statsig_signer.
    // Do NOT auto-probe public signers: those burn the request budget (often
    // 60–90s) and the request dies before ever reaching the working x0 path.
    if let Some(signer) = cred
        .statsig_signer
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let meta = match fetch_statsig_meta(client, base, cred).await {
            Ok(meta) => meta,
            Err(error) => {
                tracing::debug!(%error, "Grok statsig meta fetch failed for custom signer");
                String::new()
            }
        };
        match request_statsig_signature_resilient(
            client,
            control_client,
            signer,
            "POST",
            &path,
            &meta,
            proxy_url,
        )
        .await
        {
            Ok(value) => {
                put_cached_statsig(&cache_key, &value);
                tracing::info!(%signer, "Grok remote x-statsig-id signed (credential signer)");
                return Ok(Some(value));
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    %signer,
                    "Grok credential statsig_signer failed; falling back to built-in x0"
                );
            }
        }
    }

    // 3) Built-in x0 frontend-fallback (const in this file — not data/). Verified live chat path.
    let fallback = X0_FALLBACK_STATSIG_ID.to_string();
    put_cached_statsig(&cache_key, &fallback);
    tracing::info!(
        statsig = %fallback,
        "Grok using built-in x0 x-statsig-id (code constant, not data/)"
    );
    Ok(Some(fallback))
}

/// Grok2API validStatsigID: base64-decoded payload must be exactly 70 bytes.
/// Hit grok.com homepage on the API client session to collect CF/session cookies.
pub(super) async fn warm_session_cookies(
    client: &wreq::Client,
    base: &str,
    cred: &mut GrokCredential,
) -> Result<(), GrokError> {
    let ua = cred
        .user_agent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_USER_AGENT);
    let url = format!("{}/", base.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "none")
        .header("Upgrade-Insecure-Requests", "1")
        .header("User-Agent", ua)
        .header("Cookie", build_sso_cookie(cred))
        .send()
        .await
        .map_err(|error| GrokError::transport(format!("warm Grok session failed: {error}")))?;
    if !response.status().is_success() {
        return Err(GrokError::Http {
            message: format!("Grok warm HTTP {}", response.status()),
        });
    }
    let mut kept = 0usize;
    for value in response.headers().get_all(wreq::header::SET_COOKIE) {
        let Ok(raw) = value.to_str() else {
            continue;
        };
        let Some((name, rest)) = raw.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !is_allowed_extra_cookie(name) {
            continue;
        }
        let cookie_value = rest.split(';').next().unwrap_or("").trim().to_string();
        if cookie_value.is_empty() {
            continue;
        }
        cred.extra_cookies.insert(name.to_string(), cookie_value);
        kept += 1;
    }
    tracing::debug!(kept, "Grok session warm applied Set-Cookie whitelist");
    Ok(())
}

pub(super) fn url_path(endpoint: &str) -> String {
    reqwest::Url::parse(endpoint)
        .map(|url| {
            let path = url.path();
            if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            }
        })
        .unwrap_or_else(|_| "/rest/app-chat/conversations/new".to_string())
}

pub(super) fn get_cached_statsig(key: &str) -> Option<String> {
    let guard = statsig_cache().lock().ok()?;
    let entry = guard.get(key)?;
    if Instant::now() < entry.expires_at {
        Some(entry.value.clone())
    } else {
        None
    }
}

pub(super) fn put_cached_statsig(key: &str, value: &str) {
    if let Ok(mut guard) = statsig_cache().lock() {
        guard.insert(
            key.to_string(),
            StatsigCacheEntry {
                value: value.to_string(),
                expires_at: Instant::now() + STATSIG_CACHE_TTL,
            },
        );
    }
}

pub(super) fn invalidate_statsig_cache(base: &str, endpoint: &str) {
    let path = url_path(endpoint);
    let key = format!("{base}\0POST\0{path}");
    if let Ok(mut guard) = statsig_cache().lock() {
        guard.remove(&key);
    }
}

pub(super) async fn fetch_statsig_meta(
    client: &wreq::Client,
    base: &str,
    cred: &GrokCredential,
) -> Result<String, GrokError> {
    let ua = cred
        .user_agent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_USER_AGENT);
    let url = format!("{}/", base.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "none")
        .header("Upgrade-Insecure-Requests", "1")
        .header("User-Agent", ua)
        .header("Cookie", build_sso_cookie(cred))
        .send()
        .await
        .map_err(|error| GrokError::transport(format!("fetch Grok index failed: {error}")))?;
    if !response.status().is_success() {
        return Err(GrokError::Http {
            message: format!("Grok index HTTP {}", response.status()),
        });
    }
    let html = response
        .text()
        .await
        .map_err(|error| GrokError::transport(format!("read Grok index failed: {error}")))?;
    extract_statsig_meta_content(&html).ok_or_else(|| {
        GrokError::Other("Grok index missing grok-site-verification meta".to_string())
    })
}

pub(super) fn extract_statsig_meta_content(html: &str) -> Option<String> {
    let normalized = html.replace(
        [
            '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}',
        ],
        "-",
    );
    extract_meta_by_name(&normalized, "grok-site-verification")
}

pub(super) fn extract_meta_by_name(html: &str, target_name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("<meta") {
        let start = search_from + rel;
        let Some(end_rel) = html[start..].find('>') else {
            break;
        };
        let end = start + end_rel + 1;
        let tag = &html[start..end];
        let tag_lower = tag.to_ascii_lowercase();
        if (tag_lower.contains(&format!("name=\"{target_name}\""))
            || tag_lower.contains(&format!("name='{target_name}'")))
            && let Some(content) = attr_value(tag, "content")
        {
            let content = content.trim();
            if !content.is_empty() {
                return Some(content.to_string());
            }
        }
        search_from = end;
    }
    None
}

pub(super) fn attr_value<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{attr}=");
    let idx = lower.find(&key)?;
    let rest = tag[idx + key.len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &rest[1..];
    let end = body.find(quote)?;
    Some(&body[..end])
}

pub(super) fn statsig_sign_payload(method: &str, path: &str, meta_content: &str) -> Value {
    json!({
        "method": method.to_ascii_uppercase(),
        "path": path,
        "environment": { "metaContent": meta_content }
    })
}

pub(super) fn parse_statsig_signature_body(text: &str) -> Result<String, GrokError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(GrokError::Other("statsig signer empty body".to_string()));
    }
    if looks_like_cloudflare_challenge(trimmed) {
        return Err(GrokError::AntiBot(
            "statsig signer returned Cloudflare challenge HTML (Just a moment...)".to_string(),
        ));
    }
    // Prefer pure JSON; some browser-proxy wrappers embed JSON in HTML.
    let json_slice = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else if let Some(start) = trimmed.find('{') {
        let end = trimmed.rfind('}').unwrap_or(trimmed.len() - 1);
        if end > start {
            trimmed[start..=end].to_string()
        } else {
            return Err(GrokError::Other(
                "statsig signer body has no JSON object".to_string(),
            ));
        }
    } else {
        return Err(GrokError::Other(format!(
            "statsig signer non-JSON body: {}",
            truncate(trimmed, 160)
        )));
    };
    let value: Value = serde_json::from_str(&json_slice)
        .map_err(|error| GrokError::Other(format!("statsig signer invalid JSON: {error}")))?;
    let statsig = value
        .get("x-statsig-id")
        .or_else(|| value.get("x_statsig_id"))
        .or_else(|| value.get("statsigId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GrokError::Other("statsig signer missing x-statsig-id".to_string()))?;
    if !valid_statsig_id(statsig) {
        return Err(GrokError::Other(
            "statsig signer returned invalid x-statsig-id shape".to_string(),
        ));
    }
    Ok(statsig.to_string())
}

pub(super) fn looks_like_cloudflare_challenge(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("just a moment")
        || lower.contains("cf-browser-verification")
        || lower.contains("cf-challenge")
        || lower.contains("cdn-cgi/challenge")
        || (lower.contains("cloudflare") && lower.contains("challenge-platform"))
}

/// Try multiple egress paths. Public signer is often Cloudflare-blocked for plain HTTP clients.
pub(super) async fn request_statsig_signature_resilient(
    emulated_client: &wreq::Client,
    control_client: &reqwest::Client,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
    proxy_url: Option<&str>,
) -> Result<String, GrokError> {
    let mut errors = Vec::new();
    let is_local = signer_is_loopback(signer);

    // 1) Direct (local signer / open network)
    match request_statsig_signature(control_client, signer, method, path, meta_content).await {
        Ok(value) => {
            tracing::info!(channel = "direct", %signer, "statsig signer ok");
            return Ok(value);
        }
        Err(error) => {
            tracing::debug!(%error, channel = "direct", "statsig signer path failed");
            errors.push(format!("direct: {error}"));
        }
    }

    // 2) Chrome-TLS emulated client (same stack as Grok chat). Skip for loopback.
    if !is_local {
        match request_statsig_signature_emulated(
            emulated_client,
            signer,
            method,
            path,
            meta_content,
        )
        .await
        {
            Ok(value) => {
                tracing::info!(channel = "emulated", %signer, "statsig signer ok");
                return Ok(value);
            }
            Err(error) => {
                tracing::debug!(%error, channel = "emulated", "statsig signer path failed");
                errors.push(format!("emulated: {error}"));
            }
        }
    }

    // 3) Same proxy as Grok API traffic
    if !is_local && let Some(proxy) = proxy_url.map(str::trim).filter(|v| !v.is_empty()) {
        match request_statsig_signature_with_proxy(proxy, signer, method, path, meta_content).await
        {
            Ok(value) => {
                tracing::info!(channel = "proxy", %signer, %proxy, "statsig signer ok");
                return Ok(value);
            }
            Err(error) => {
                tracing::debug!(%error, channel = "proxy", "statsig signer path failed");
                errors.push(format!("proxy({proxy}): {error}"));
            }
        }
    }

    Err(GrokError::Other(errors.join(" | ")))
}

pub(super) fn signer_is_loopback(signer: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(signer) else {
        return false;
    };
    matches!(
        url.host_str().unwrap_or("").to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1" | "0.0.0.0" | "host.docker.internal"
    )
}

pub(super) async fn request_statsig_signature(
    client: &reqwest::Client,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, GrokError> {
    let payload = statsig_sign_payload(method, path, meta_content);
    let response = client
        .post(signer)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", DEFAULT_USER_AGENT)
        .json(&payload)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|error| GrokError::transport(format!("statsig signer request failed: {error}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| GrokError::transport(format!("statsig signer body failed: {error}")))?;
    if !status.is_success() {
        return Err(GrokError::Http {
            message: format!("statsig signer HTTP {status}: {}", truncate(&text, 160)),
        });
    }
    parse_statsig_signature_body(&text)
}

pub(super) async fn request_statsig_signature_emulated(
    client: &wreq::Client,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, GrokError> {
    let payload = statsig_sign_payload(method, path, meta_content);
    let response = client
        .post(signer)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", DEFAULT_USER_AGENT)
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            GrokError::transport(format!("statsig emulated signer request failed: {error}"))
        })?;
    let status = response.status();
    let text = response.text().await.map_err(|error| {
        GrokError::transport(format!("statsig emulated signer body failed: {error}"))
    })?;
    if !status.is_success() {
        return Err(GrokError::Http {
            message: format!(
                "statsig emulated signer HTTP {status}: {}",
                truncate(&text, 160)
            ),
        });
    }
    parse_statsig_signature_body(&text)
}

pub(super) async fn request_statsig_signature_with_proxy(
    proxy_url: &str,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, GrokError> {
    let client = crate::http_client::build_reqwest_client(
        crate::http_client::HttpClientConfig::new()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(2)
            .proxy_url(Some(proxy_url)),
    )
    .map_err(|error| {
        GrokError::Other(format!("failed to build proxied statsig client: {error}"))
    })?;
    request_statsig_signature(&client, signer, method, path, meta_content).await
}
