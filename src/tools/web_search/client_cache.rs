//! Cached HTTP clients for web-search providers.
//!
//! Search fans out over sources × queries; building a fresh client per request
//! throws away the connection pool and forces a full TCP+TLS handshake every
//! time. Clients are cached by their effective configuration instead.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

const MAX_CACHED_CLIENTS: usize = 32;

#[derive(Clone, PartialEq, Eq, Hash)]
struct ClientKey {
    profile: &'static str,
    timeout_ms: u64,
    proxy: Option<String>,
}

fn cache() -> &'static Mutex<HashMap<ClientKey, reqwest::Client>> {
    static CACHE: OnceLock<Mutex<HashMap<ClientKey, reqwest::Client>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_or_build(
    key: ClientKey,
    build: impl FnOnce() -> Result<reqwest::Client, String>,
) -> Result<reqwest::Client, String> {
    if let Ok(guard) = cache().lock()
        && let Some(client) = guard.get(&key)
    {
        return Ok(client.clone());
    }
    let client = build()?;
    if let Ok(mut guard) = cache().lock() {
        if guard.len() >= MAX_CACHED_CLIENTS {
            guard.clear();
        }
        guard.insert(key, client.clone());
    }
    Ok(client)
}

fn normalized_proxy(proxy_url: Option<&str>) -> Option<String> {
    proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| crate::domain::ip_proxies::proxy_url_with_remote_dns(value).into_owned())
}

/// Provider control-plane client (search/extract REST APIs).
pub fn control_client(
    timeout: Duration,
    proxy_url: Option<&str>,
) -> Result<reqwest::Client, String> {
    let proxy = normalized_proxy(proxy_url);
    let key = ClientKey {
        profile: "control",
        timeout_ms: timeout.as_millis() as u64,
        proxy: proxy.clone(),
    };
    get_or_build(key, || {
        crate::http_client::build_reqwest_client(
            crate::http_client::HttpClientConfig::new()
                .timeout(timeout)
                .proxy_url(proxy.as_deref()),
        )
    })
}

/// Direct local fetch client used by web_extract (no redirects).
pub fn local_extract_client() -> Result<reqwest::Client, String> {
    let key = ClientKey {
        profile: "local_extract",
        timeout_ms: 45_000,
        proxy: None,
    };
    get_or_build(key, || {
        crate::http_client::build_reqwest_client(
            crate::http_client::HttpClientConfig::new()
                .timeout(Duration::from_secs(45))
                .no_redirects()
                .user_agent("WeiAgent/1.0 web_extract"),
        )
    })
}

/// Remote-provider extraction client without proxy.
pub fn remote_extract_client(timeout: Duration) -> Result<reqwest::Client, String> {
    let key = ClientKey {
        profile: "remote_extract",
        timeout_ms: timeout.as_millis() as u64,
        proxy: None,
    };
    get_or_build(key, || {
        crate::http_client::build_reqwest_client(
            crate::http_client::HttpClientConfig::new().timeout(timeout),
        )
    })
}
