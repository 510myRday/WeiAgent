use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use wreq::StatusCode;
use wreq::header::{HeaderMap, HeaderName, HeaderValue};

use crate::tools::web_search::types::{
    AiSynthesis, ProviderError, ProviderSearchOutput, ProviderSearchRequest, SearchHitKind,
    WebSearchHit, sanitize_error_message,
};

const DEFAULT_MODE: &str = "fast";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
// Prefer remote/manual 70-byte x-statsig-id; if unavailable, use frontend failure fallback (x0:TypeError...).
// Public remote signer is optional only if credential sets statsig_signer explicitly.
// Not auto-probed anymore — it is CF-blocked and burns the request budget.
/// Browser middleware fallback when statsig module throws: btoa("x0:" + error) without quotes around childNodes.
const X0_FALLBACK_STATSIG_ID: &str = "eDA6VHlwZUVycm9yOiBDYW5ub3QgcmVhZCBwcm9wZXJ0aWVzIG9mIHVuZGVmaW5lZCAocmVhZGluZyBjaGlsZE5vZGVzKQ==";
const MAX_FRAME_BYTES: usize = 8 << 20;
const MAX_SNIPPET_CHARS: usize = 800;
const STATSIG_CACHE_TTL: Duration = Duration::from_secs(50 * 60);

#[derive(Debug, Clone)]
struct GrokCredential {
    sso: String,
    extra_cookies: BTreeMap<String, String>,
    statsig_id: Option<String>,
    user_agent: Option<String>,
    statsig_signer: Option<String>,
}

#[derive(Debug, Default)]
struct ParsedChat {
    text: String,
    sources: Vec<WebSearchHit>,
    seen_urls: HashSet<String>,
    error: Option<String>,
}

struct StatsigCacheEntry {
    value: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub struct GrokSearchOptions<'a> {
    pub source_id: &'a str,
    pub flaresolverr_url: Option<&'a str>,
    pub flaresolverr_proxy_url: Option<&'a str>,
    pub proxy_url: Option<&'a str>,
}

fn statsig_cache() -> &'static Mutex<BTreeMap<String, StatsigCacheEntry>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, StatsigCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub async fn search(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
    options: GrokSearchOptions<'_>,
) -> Result<ProviderSearchOutput, ProviderError> {
    let mut cred = parse_credential(api_key)
        .map_err(|message| ProviderError::Retryable(sanitize_error_message(&message, api_key)))?;
    if cred.sso.is_empty() {
        return Err(ProviderError::Retryable(
            "Grok SSO token is empty".to_string(),
        ));
    }

    let base = base_url.trim_end_matches('/');
    let endpoint = format!("{base}/rest/app-chat/conversations/new");
    let prompt = build_search_prompt(request);
    let body = build_web_chat_payload(&prompt, DEFAULT_MODE);
    let flaresolverr = super::grok_support::normalize_flaresolverr_url(options.flaresolverr_url);
    let flaresolverr_proxy =
        super::grok_support::normalize_optional_url(options.flaresolverr_proxy_url);
    // API traffic proxy is applied on wreq client in web_search::mod; also used for statsig signer egress.
    let proxy = super::grok_support::normalize_optional_url(options.proxy_url);

    // NOTE: Do NOT inject FlareSolverr cf_clearance into wreq chat.
    // FS runs a real browser fingerprint; wreq emulates Chrome TLS separately.
    // Mixing them is the main cause of code=7 anti-bot after "clearance ready".
    let _ = (flaresolverr.as_deref(), flaresolverr_proxy.as_deref());
    if flaresolverr.is_some() {
        tracing::info!(
            "Grok FlareSolverr is configured for browser-side checks; chat uses the same-proxy wreq session"
        );
        // Cookies minted by FlareSolverr belong to its browser TLS/UA session.
        // Replaying them on wreq is a common source of code=7, so start with a
        // clean wreq session and let the same client warm its own CF cookies.
        strip_foreign_cloudflare_cookies(&mut cred);
    }

    let mut last_error = String::new();
    for attempt in 0..3 {
        let force_refresh = attempt > 0;

        // On anti-bot retry, drop only foreign clearance cookies and keep the
        // __cf_bm/device identity minted by this wreq session.
        if force_refresh {
            drop_foreign_clearance_cookie(&mut cred);
            invalidate_statsig_cache(base, &endpoint);
        }

        // Warm homepage cookies (__cf_bm / device ids) on the SAME TLS+proxy client as chat.
        if let Err(error) = warm_session_cookies(client, base, &mut cred).await {
            tracing::debug!(%error, "Grok session warm failed; continuing with SSO cookies only");
        }

        let statsig = resolve_statsig_id(
            client,
            control_client,
            base,
            &cred,
            &endpoint,
            force_refresh,
            proxy.as_deref(),
            None,
            None,
        )
        .await
        .ok()
        .flatten();

        match execute_chat(client, base, &endpoint, &cred, statsig.as_deref(), &body).await {
            Ok(bytes) => {
                let parsed = consume_upstream(&bytes).map_err(|error| {
                    ProviderError::Retryable(sanitize_error_message(&error, api_key))
                })?;
                if let Some(error) = parsed.error {
                    if is_anti_bot_message(&error) {
                        tracing::warn!(%error, "Grok chat rejected by anti-bot; skipping same-key retry");
                    }
                    return Err(classify_upstream_error(&error, api_key));
                }
                return map_hits(
                    parsed,
                    &request.query,
                    request.max_results,
                    options.source_id,
                );
            }
            Err(error) => {
                let message = error.message().to_string();
                let retryable_transport = is_transport_retryable(&message);
                if retryable_transport && attempt < 2 {
                    tracing::warn!(
                        attempt,
                        %message,
                        transport = retryable_transport,
                        "Grok chat transport failed; retrying"
                    );
                    last_error = message;
                    tokio::time::sleep(Duration::from_millis(350 * (attempt as u64 + 1))).await;
                    continue;
                }
                return Err(error);
            }
        }
    }

    Err(ProviderError::Retryable(sanitize_error_message(
        &format!(
            "Grok request rejected (anti-bot or transport). Prefer stable proxy_url, longer timeout_ms (>=60000), SSO cookies, and usable x-statsig-id (70-byte or x0 fallback). FlareSolverr is optional. detail={last_error}"
        ),
        api_key,
    )))
}

async fn execute_chat(
    client: &wreq::Client,
    base: &str,
    endpoint: &str,
    cred: &GrokCredential,
    statsig: Option<&str>,
    body: &Value,
) -> Result<Vec<u8>, ProviderError> {
    let headers = build_request_headers(base, cred, statsig)
        .map_err(|message| ProviderError::Retryable(sanitize_error_message(&message, &cred.sso)))?;

    // Chrome TLS/JA3/H2 via wreq emulation (scheme B); cookies still from SSO + FlareSolverr.
    // Proxy/idle-pool flakiness shows up as Connect/timeout — retry a few times before bubbling up.
    let mut last_send_error = String::new();
    let response = {
        let mut response = None;
        for send_attempt in 0..3u32 {
            match client
                .post(endpoint)
                .headers(headers.clone())
                .json(body)
                .send()
                .await
            {
                Ok(resp) => {
                    response = Some(resp);
                    break;
                }
                Err(error) => {
                    last_send_error = error.to_string();
                    if is_transport_retryable(&last_send_error) && send_attempt < 2 {
                        tracing::warn!(
                            send_attempt,
                            error = %last_send_error,
                            "Grok connect/send failed; retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(350 * (send_attempt as u64 + 1)))
                            .await;
                        continue;
                    }
                    return Err(ProviderError::Retryable(sanitize_error_message(
                        &format!("Grok request failed: {last_send_error}"),
                        &cred.sso,
                    )));
                }
            }
        }
        response.ok_or_else(|| {
            ProviderError::Retryable(sanitize_error_message(
                &format!("Grok request failed: {last_send_error}"),
                &cred.sso,
            ))
        })?
    };

    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| {
        ProviderError::Retryable(sanitize_error_message(
            &format!("Grok response body error: {error}"),
            &cred.sso,
        ))
    })?;
    let body_text = String::from_utf8_lossy(&bytes);

    if status == StatusCode::UNAUTHORIZED {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!(
                "Grok HTTP {status}: SSO unauthorized; body={}",
                truncate(&body_text, 240)
            ),
            &cred.sso,
        )));
    }
    if status == StatusCode::FORBIDDEN || is_anti_bot_message(&body_text) {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!(
                "Grok HTTP {status}: anti-bot/forbidden; body={}",
                truncate(&body_text, 240)
            ),
            &cred.sso,
        )));
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Grok HTTP {status}: {}", truncate(&body_text, 240)),
            &cred.sso,
        )));
    }
    if !status.is_success() {
        return Err(ProviderError::Retryable(sanitize_error_message(
            &format!("Grok HTTP {status}: {}", truncate(&body_text, 240)),
            &cred.sso,
        )));
    }
    Ok(bytes.to_vec())
}

fn map_hits(
    parsed: ParsedChat,
    query: &str,
    max_results: u32,
    source_id: &str,
) -> Result<ProviderSearchOutput, ProviderError> {
    let mut hits = parse_structured_hits(&parsed.text, max_results);
    let answer = parse_structured_answer(&parsed.text)
        .or_else(|| (!parsed.text.trim().is_empty()).then(|| parsed.text.trim().to_string()));
    for source in parsed.sources {
        if let Some(existing) = hits.iter_mut().find(|hit| hit.url == source.url) {
            existing.kind = source.kind;
            if source.snippet.chars().count() > existing.snippet.chars().count() {
                existing.snippet = source.snippet;
            }
            continue;
        }
        if (hits.len() as u32) < max_results {
            hits.push(source);
        }
    }
    hits.truncate(max_results as usize);
    if hits.is_empty() && answer.is_none() {
        return Err(ProviderError::Retryable(
            "Grok returned no search results".to_string(),
        ));
    }
    let ai_syntheses = answer
        .map(|answer| {
            vec![AiSynthesis {
                source_id: source_id.to_string(),
                provider: "grok".to_string(),
                query: query.to_string(),
                answer: truncate(&answer, 1_200),
                verification_status: "unverified".to_string(),
                evidence_counted: false,
            }]
        })
        .unwrap_or_default();
    Ok(ProviderSearchOutput {
        results: hits,
        ai_syntheses,
    })
}

fn classify_upstream_error(error: &str, api_key: &str) -> ProviderError {
    ProviderError::Retryable(sanitize_error_message(
        &format!("Grok upstream error: {error}"),
        api_key,
    ))
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

fn is_anti_bot_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("anti-bot")
        || lower.contains("antibot")
        || lower.contains("\"code\":7")
        || lower.contains("request rejected by anti-bot")
}

/// Proxy/TLS flakiness: Connect, timeout, reset, broken pipe, etc.
fn is_transport_retryable(message: &str) -> bool {
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

fn build_search_prompt(request: &ProviderSearchRequest) -> String {
    let domain_instruction = if request.allowed_domains.is_empty() {
        String::new()
    } else {
        format!(
            " Restrict sources to these domains: {}.",
            request.allowed_domains.join(", ")
        )
    };
    let blocked_instruction = if request.blocked_domains.is_empty() {
        String::new()
    } else {
        format!(
            " Exclude these domains: {}.",
            request.blocked_domains.join(", ")
        )
    };
    let recency_instruction = request
        .recency_days
        .map(|days| format!(" Prefer information published within the last {days} days."))
        .unwrap_or_default();
    format!(
        "You are the web research backend for another AI. Search the public web for the query, then synthesize the most useful direct answer. Return ONLY valid JSON (no markdown fences or commentary) in exactly this shape: {{\"answer\":\"...\",\"sources\":[{{\"title\":\"...\",\"url\":\"...\",\"snippet\":\"...\"}}]}}. The answer is required, concise, self-contained, and must contain the actual conclusion or explanation rather than a list of links. The sources array is optional and may be empty; include at most {max_results} high-quality supporting sources when useful. Sources supplement the answer and must never replace it. Prefer primary sources and do not invent facts or URLs.{domain_instruction}{blocked_instruction}{recency_instruction} Query: {query}",
        max_results = request.max_results,
        query = request.query,
    )
}

fn build_web_chat_payload(message: &str, mode: &str) -> Value {
    json!({
        "collectionIds": [],
        "disabledConnectorIds": [],
        "deviceEnvInfo": {
            "darkModeEnabled": false,
            "devicePixelRatio": 2,
            "screenHeight": 1328,
            "screenWidth": 2056,
            "viewportHeight": 1083,
            "viewportWidth": 2056
        },
        "disableMemory": true,
        "disableSearch": false,
        "disableSelfHarmShortCircuit": false,
        "disableTextFollowUps": true,
        "enableImageGeneration": false,
        "enableImageStreaming": false,
        "enableSideBySide": false,
        "fileAttachments": [],
        "forceConcise": true,
        "forceSideBySide": false,
        "imageAttachments": [],
        "imageGenerationCount": 0,
        "isAsyncChat": false,
        "message": message,
        "modeId": mode,
        "responseMetadata": {},
        "returnImageBytes": false,
        "returnRawGrokInXaiRequest": false,
        "sendFinalMetadata": true,
        "temporary": true
    })
}

fn parse_credential(raw: &str) -> Result<GrokCredential, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty credential".to_string());
    }

    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|error| format!("invalid Grok credential JSON: {error}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| "Grok credential JSON must be an object".to_string())?;
        let sso = first_string(obj, &["sso", "token", "auth_token", "api_key"])
            .map(normalize_sso_token)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Grok credential JSON missing sso/token".to_string())?;
        let mut extra = BTreeMap::new();
        if let Some(cookies) = first_string(obj, &["cookies", "cookie", "cf_cookies", "cloudflare"])
        {
            merge_cookie_header(&mut extra, cookies);
        }
        if let Some(cf) = first_string(obj, &["cf_clearance"]) {
            extra.insert("cf_clearance".to_string(), cf.to_string());
        }
        if let Some(bm) = first_string(obj, &["__cf_bm"]) {
            extra.insert("__cf_bm".to_string(), bm.to_string());
        }
        let statsig_id = first_string(
            obj,
            &["x_statsig_id", "x-statsig-id", "statsig_id", "statsig"],
        )
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
        let user_agent = first_string(obj, &["user_agent", "ua", "User-Agent"])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let statsig_signer = first_string(obj, &["statsig_signer", "signer_url", "statsig_url"])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let (sso, peeled) = peel_cookie_pairs(&sso);
        for (k, v) in peeled {
            extra.entry(k).or_insert(v);
        }
        return Ok(GrokCredential {
            sso,
            extra_cookies: extra,
            statsig_id,
            user_agent,
            statsig_signer,
        });
    }

    if trimmed.contains('=') && (trimmed.contains("sso=") || trimmed.contains(';')) {
        let mut map = BTreeMap::new();
        merge_cookie_header(&mut map, trimmed);
        let sso = map
            .remove("sso")
            .or_else(|| map.remove("sso-rw"))
            .map(|value| normalize_sso_token(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| normalize_sso_token(trimmed));
        map.remove("sso-rw");
        return Ok(GrokCredential {
            sso,
            extra_cookies: map,
            statsig_id: None,
            user_agent: None,
            statsig_signer: None,
        });
    }

    Ok(GrokCredential {
        sso: normalize_sso_token(trimmed),
        extra_cookies: BTreeMap::new(),
        statsig_id: None,
        user_agent: None,
        statsig_signer: None,
    })
}

fn first_string<'a>(obj: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(value) = obj.get(*key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn merge_cookie_header(map: &mut BTreeMap<String, String>, header: &str) {
    for part in header.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((name, value)) = part.split_once('=') {
            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "path" | "domain" | "expires" | "max-age" | "secure" | "httponly" | "samesite"
            ) {
                continue;
            }
            map.insert(name.to_string(), value.to_string());
        }
    }
}

fn peel_cookie_pairs(value: &str) -> (String, BTreeMap<String, String>) {
    if !value.contains(';') {
        return (normalize_sso_token(value), BTreeMap::new());
    }
    let mut map = BTreeMap::new();
    merge_cookie_header(&mut map, value);
    let sso = map
        .remove("sso")
        .or_else(|| map.remove("sso-rw"))
        .map(|v| normalize_sso_token(&v))
        .unwrap_or_else(|| normalize_sso_token(value));
    map.remove("sso-rw");
    (sso, map)
}

fn normalize_sso_token(value: &str) -> String {
    let mut value = value.trim().to_string();
    if value.to_ascii_lowercase().starts_with("sso=") {
        value = value[4..].trim().to_string();
    }
    if let Some((token, _)) = value.split_once(';') {
        value = token.trim().to_string();
    }
    value.replace(['\r', '\n', '\0'], "").trim().to_string()
}

fn build_sso_cookie(cred: &GrokCredential) -> String {
    let mut parts = vec![format!("sso={}", cred.sso), format!("sso-rw={}", cred.sso)];
    // Cloudflare clearance + stable session identity cookies observed on successful chat.
    for (name, value) in &cred.extra_cookies {
        if !is_allowed_extra_cookie(name) {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        parts.push(format!("{}={}", name, value));
    }
    parts.join("; ")
}

fn is_cloudflare_cookie_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "cf_clearance"
        || lower == "__cf_bm"
        || lower == "_cfuvid"
        || lower.starts_with("cf_chl_")
}

fn is_session_identity_cookie_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "x-userid" | "grok_device_id"
    )
}

fn is_allowed_extra_cookie(name: &str) -> bool {
    is_cloudflare_cookie_name(name) || is_session_identity_cookie_name(name)
}

/// Drop Cloudflare cookies that did not come from this wreq TLS session.
/// Foreign cf_clearance (e.g. FlareSolverr browser) causes code=7 with emulated clients.
fn strip_foreign_cloudflare_cookies(cred: &mut GrokCredential) {
    let before = cred.extra_cookies.len();
    cred.extra_cookies
        .retain(|name, _| !is_cloudflare_cookie_name(name));
    // Also drop UA forced by FlareSolverr so DEFAULT_USER_AGENT / Chrome146 stay aligned.
    cred.user_agent = None;
    let removed = before.saturating_sub(cred.extra_cookies.len());
    if removed > 0 {
        tracing::info!(
            removed,
            "Grok stripped foreign Cloudflare cookies before re-warm"
        );
    }
}

/// Keep the __cf_bm/device cookies minted by this wreq client across retries.
/// Only clearance/challenge cookies are tied to a different browser session.
fn drop_foreign_clearance_cookie(cred: &mut GrokCredential) {
    let before = cred.extra_cookies.len();
    cred.extra_cookies.retain(|name, _| {
        let lower = name.trim().to_ascii_lowercase();
        lower != "cf_clearance" && lower != "_cfuvid" && !lower.starts_with("cf_chl_")
    });
    let removed = before.saturating_sub(cred.extra_cookies.len());
    if removed > 0 {
        tracing::info!(
            removed,
            "Grok dropped foreign clearance cookies before retry"
        );
    }
}

fn build_request_headers(
    base: &str,
    cred: &GrokCredential,
    statsig: Option<&str>,
) -> Result<HeaderMap, String> {
    let ua = cred
        .user_agent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_USER_AGENT);
    let mut headers = HeaderMap::new();
    // Match verified curl_cffi chrome146 live path (grok_probe).
    set_header(&mut headers, "Content-Type", "application/json")?;
    set_header(&mut headers, "Accept", "*/*")?;
    set_header(&mut headers, "Accept-Encoding", "gzip, deflate, br, zstd")?;
    set_header(&mut headers, "Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")?;
    set_header(&mut headers, "User-Agent", ua)?;
    apply_chromium_client_hints(&mut headers, ua)?;
    set_header(&mut headers, "Origin", base)?;
    set_header(&mut headers, "Referer", &format!("{base}/"))?;
    set_header(&mut headers, "Cache-Control", "no-cache")?;
    set_header(&mut headers, "Pragma", "no-cache")?;
    set_header(&mut headers, "Priority", "u=1, i")?;
    set_header(&mut headers, "Sec-Fetch-Dest", "empty")?;
    set_header(&mut headers, "Sec-Fetch-Mode", "cors")?;
    set_header(&mut headers, "Sec-Fetch-Site", "same-origin")?;
    set_header(&mut headers, "Cookie", &build_sso_cookie(cred))?;
    set_header(
        &mut headers,
        "x-xai-request-id",
        &uuid::Uuid::new_v4().to_string(),
    )?;
    if let Some(statsig) = statsig
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| usable_statsig_id(value))
    {
        set_header(&mut headers, "x-statsig-id", statsig)?;
    }
    Ok(headers)
}

fn apply_chromium_client_hints(headers: &mut HeaderMap, user_agent: &str) -> Result<(), String> {
    let lower = user_agent.to_ascii_lowercase();
    let version = capture_version(user_agent, &["Chrome/", "Chromium/", "Edg/"])
        .unwrap_or_else(|| "146".to_string());
    // curl_cffi chrome146 order: Chromium, Not-A.Brand, Google Chrome
    set_header(
        headers,
        "sec-ch-ua",
        &format!(
            "\"Chromium\";v=\"{version}\", \"Not-A.Brand\";v=\"24\", \"Google Chrome\";v=\"{version}\""
        ),
    )?;
    set_header(
        headers,
        "sec-ch-ua-mobile",
        if lower.contains("mobile") { "?1" } else { "?0" },
    )?;
    let platform = if lower.contains("windows") {
        "\"Windows\""
    } else if lower.contains("mac os x") || lower.contains("macintosh") {
        "\"macOS\""
    } else if lower.contains("android") {
        "\"Android\""
    } else if lower.contains("linux") {
        "\"Linux\""
    } else {
        "\"macOS\""
    };
    set_header(headers, "sec-ch-ua-platform", platform)?;
    Ok(())
}

fn capture_version(user_agent: &str, markers: &[&str]) -> Option<String> {
    for marker in markers {
        if let Some(idx) = user_agent.find(marker) {
            let rest = &user_agent[idx + marker.len()..];
            let ver: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !ver.is_empty() {
                return Some(ver);
            }
        }
    }
    None
}

fn set_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), String> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|error| format!("invalid header name {name}: {error}"))?;
    let header_value = HeaderValue::from_str(value)
        .map_err(|error| format!("invalid header value for {name}: {error}"))?;
    headers.insert(header_name, header_value);
    Ok(())
}

async fn resolve_statsig_id(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base: &str,
    cred: &GrokCredential,
    endpoint: &str,
    force_refresh: bool,
    proxy_url: Option<&str>,
    flaresolverr_url: Option<&str>,
    flaresolverr_proxy_url: Option<&str>,
) -> Result<Option<String>, String> {
    // Silence unused when we skip slow remote/FS signing by default.
    let _ = (
        client,
        control_client,
        proxy_url,
        flaresolverr_url,
        flaresolverr_proxy_url,
    );

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
    if !force_refresh {
        if let Some(cached) = get_cached_statsig(&cache_key) {
            if usable_statsig_id(&cached) {
                return Ok(Some(cached));
            }
            invalidate_statsig_cache(base, endpoint);
        }
    }

    // 2) Optional explicit local/remote signer ONLY if credential sets statsig_signer.
    // Do NOT auto-probe wodf.de / 8788 / FlareSolverr: those burn the request budget
    // (often 60–90s) and the request dies before ever reaching the working x0 path.
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
            // Never route signing through FlareSolverr on the hot path.
            None,
            None,
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
    let fallback = x0_fallback_statsig_id();
    put_cached_statsig(&cache_key, &fallback);
    tracing::info!(
        statsig = %fallback,
        "Grok using built-in x0 x-statsig-id (code constant, not data/)"
    );
    Ok(Some(fallback))
}

/// Grok2API validStatsigID: base64-decoded payload must be exactly 70 bytes.
fn valid_statsig_id(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value));
    matches!(decoded, Ok(bytes) if bytes.len() == 70)
}

/// Frontend failure fallback used when module 4629918 signing fails: btoa("x0:" + error).
fn x0_fallback_statsig_id() -> String {
    X0_FALLBACK_STATSIG_ID.to_string()
}

fn is_x0_fallback_statsig_id(value: &str) -> bool {
    value.trim() == X0_FALLBACK_STATSIG_ID
}

fn usable_statsig_id(value: &str) -> bool {
    valid_statsig_id(value) || is_x0_fallback_statsig_id(value)
}

/// Hit grok.com homepage on the API client session to collect CF/session cookies.
async fn warm_session_cookies(
    client: &wreq::Client,
    base: &str,
    cred: &mut GrokCredential,
) -> Result<(), String> {
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
        .map_err(|error| format!("warm Grok session failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Grok warm HTTP {}", response.status()));
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

fn url_path(endpoint: &str) -> String {
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

fn get_cached_statsig(key: &str) -> Option<String> {
    let guard = statsig_cache().lock().ok()?;
    let entry = guard.get(key)?;
    if Instant::now() < entry.expires_at {
        Some(entry.value.clone())
    } else {
        None
    }
}

fn put_cached_statsig(key: &str, value: &str) {
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

fn invalidate_statsig_cache(base: &str, endpoint: &str) {
    let path = url_path(endpoint);
    let key = format!("{base}\0POST\0{path}");
    if let Ok(mut guard) = statsig_cache().lock() {
        guard.remove(&key);
    }
}

async fn fetch_statsig_meta(
    client: &wreq::Client,
    base: &str,
    cred: &GrokCredential,
) -> Result<String, String> {
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
        .map_err(|error| format!("fetch Grok index failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Grok index HTTP {}", response.status()));
    }
    let html = response
        .text()
        .await
        .map_err(|error| format!("read Grok index failed: {error}"))?;
    extract_statsig_meta_content(&html)
        .ok_or_else(|| "Grok index missing grok-site-verification meta".to_string())
}

fn extract_statsig_meta_content(html: &str) -> Option<String> {
    let normalized = html
        .replace('\u{2010}', "-")
        .replace('\u{2011}', "-")
        .replace('\u{2012}', "-")
        .replace('\u{2013}', "-")
        .replace('\u{2014}', "-")
        .replace('\u{2015}', "-");
    extract_meta_by_name(&normalized, "grok-site-verification")
}

fn extract_meta_by_name(html: &str, target_name: &str) -> Option<String> {
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
        if tag_lower.contains(&format!("name=\"{target_name}\""))
            || tag_lower.contains(&format!("name='{target_name}'"))
        {
            if let Some(content) = attr_value(tag, "content") {
                let content = content.trim();
                if !content.is_empty() {
                    return Some(content.to_string());
                }
            }
        }
        search_from = end;
    }
    None
}

fn attr_value<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
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

fn statsig_sign_payload(method: &str, path: &str, meta_content: &str) -> Value {
    json!({
        "method": method.to_ascii_uppercase(),
        "path": path,
        "environment": { "metaContent": meta_content }
    })
}

fn parse_statsig_signature_body(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("statsig signer empty body".to_string());
    }
    if looks_like_cloudflare_challenge(trimmed) {
        return Err(
            "statsig signer returned Cloudflare challenge HTML (Just a moment...)".to_string(),
        );
    }
    // Prefer pure JSON; some browser-proxy wrappers embed JSON in HTML.
    let json_slice = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else if let Some(start) = trimmed.find('{') {
        let end = trimmed.rfind('}').unwrap_or(trimmed.len() - 1);
        if end > start {
            trimmed[start..=end].to_string()
        } else {
            return Err("statsig signer body has no JSON object".to_string());
        }
    } else {
        return Err(format!(
            "statsig signer non-JSON body: {}",
            truncate(trimmed, 160)
        ));
    };
    let value: Value = serde_json::from_str(&json_slice)
        .map_err(|error| format!("statsig signer invalid JSON: {error}"))?;
    let statsig = value
        .get("x-statsig-id")
        .or_else(|| value.get("x_statsig_id"))
        .or_else(|| value.get("statsigId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "statsig signer missing x-statsig-id".to_string())?;
    if !valid_statsig_id(statsig) {
        return Err("statsig signer returned invalid x-statsig-id shape".to_string());
    }
    Ok(statsig.to_string())
}

fn looks_like_cloudflare_challenge(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("just a moment")
        || lower.contains("cf-browser-verification")
        || lower.contains("cf-challenge")
        || lower.contains("cdn-cgi/challenge")
        || (lower.contains("cloudflare") && lower.contains("challenge-platform"))
}

/// Try multiple egress paths. Public signer is often Cloudflare-blocked for plain HTTP clients.
async fn request_statsig_signature_resilient(
    emulated_client: &wreq::Client,
    control_client: &reqwest::Client,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
    proxy_url: Option<&str>,
    flaresolverr_url: Option<&str>,
    flaresolverr_proxy_url: Option<&str>,
) -> Result<String, String> {
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

    // 3) Same proxy as Grok API traffic (and optional flaresolverr proxy as extra hop)
    let mut proxies: Vec<String> = Vec::new();
    for candidate in [proxy_url, flaresolverr_proxy_url] {
        if let Some(proxy) = candidate.map(str::trim).filter(|v| !v.is_empty()) {
            if !proxies.iter().any(|p| p == proxy) {
                proxies.push(proxy.to_string());
            }
        }
    }
    for proxy in &proxies {
        if is_local {
            break;
        }
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

    // 4) Browser POST via FlareSolverr (solve CF on public signer site)
    if !is_local {
        if let Some(fs) = flaresolverr_url.map(str::trim).filter(|v| !v.is_empty()) {
            match request_statsig_signature_via_flaresolverr(
                fs,
                flaresolverr_proxy_url.or(proxy_url),
                signer,
                method,
                path,
                meta_content,
            )
            .await
            {
                Ok(value) => {
                    tracing::info!(channel = "flaresolverr", %signer, "statsig signer ok");
                    return Ok(value);
                }
                Err(error) => {
                    tracing::debug!(%error, channel = "flaresolverr", "statsig signer path failed");
                    errors.push(format!("flaresolverr: {error}"));
                }
            }
        }
    }

    Err(errors.join(" | "))
}

fn signer_is_loopback(signer: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(signer) else {
        return false;
    };
    match url.host_str().unwrap_or("").to_ascii_lowercase().as_str() {
        "127.0.0.1" | "localhost" | "::1" | "0.0.0.0" | "host.docker.internal" => true,
        _ => false,
    }
}

async fn request_statsig_signature(
    client: &reqwest::Client,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, String> {
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
        .map_err(|error| format!("statsig signer request failed: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("statsig signer body failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "statsig signer HTTP {status}: {}",
            truncate(&text, 160)
        ));
    }
    parse_statsig_signature_body(&text)
}

async fn request_statsig_signature_emulated(
    client: &wreq::Client,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, String> {
    let payload = statsig_sign_payload(method, path, meta_content);
    let response = client
        .post(signer)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", DEFAULT_USER_AGENT)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("statsig emulated signer request failed: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("statsig emulated signer body failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "statsig emulated signer HTTP {status}: {}",
            truncate(&text, 160)
        ));
    }
    parse_statsig_signature_body(&text)
}

async fn request_statsig_signature_with_proxy(
    proxy_url: &str,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, String> {
    let proxy = reqwest::Proxy::all(proxy_url)
        .map_err(|error| format!("invalid statsig proxy '{proxy_url}': {error}"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .proxy(proxy)
        .pool_max_idle_per_host(2)
        .build()
        .map_err(|error| format!("failed to build proxied statsig client: {error}"))?;
    request_statsig_signature(&client, signer, method, path, meta_content).await
}

async fn request_statsig_signature_via_flaresolverr(
    flaresolverr_url: &str,
    flaresolverr_proxy_url: Option<&str>,
    signer: &str,
    method: &str,
    path: &str,
    meta_content: &str,
) -> Result<String, String> {
    let payload = statsig_sign_payload(method, path, meta_content);
    let body = serde_json::to_string(&payload)
        .map_err(|error| format!("encode statsig payload: {error}"))?;

    // Important: grok.com clearance cookies do NOT apply to grok.wodf.de.
    // FlareSolverr is strongest on GET navigations. Warm the signer origin first,
    // then POST with that domain's Cloudflare cookies + matching UA.
    let origin = signer_origin(signer);
    let control = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("build control client for signer clearance: {error}"))?;
    let warm_url = format!("{origin}/");
    let clearance = super::grok_support::resolve_clearance(
        &control,
        flaresolverr_url,
        &warm_url,
        flaresolverr_proxy_url,
        false,
    )
    .await
    .map_err(|error| format!("FlareSolverr warm GET {warm_url} failed: {error}"))?;

    let cookie_header = cloudflare_cookie_header(&clearance.cookies);
    let ua = clearance
        .user_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_USER_AGENT);

    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert(
        "Accept".to_string(),
        "application/json, text/plain, */*".to_string(),
    );
    headers.insert("User-Agent".to_string(), ua.to_string());
    headers.insert("Origin".to_string(), origin.clone());
    headers.insert("Referer".to_string(), format!("{origin}/"));
    if !cookie_header.is_empty() {
        headers.insert("Cookie".to_string(), cookie_header);
    }

    tracing::info!(
        %signer,
        %origin,
        cookie_names = %clearance
            .cookies
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(","),
        "FlareSolverr signing via warm-GET + POST (domain-local clearance)"
    );

    let result = super::grok_support::flaresolverr_request_post(
        flaresolverr_url,
        signer,
        flaresolverr_proxy_url,
        &headers,
        &body,
        90_000,
    )
    .await?;

    tracing::info!(
        status = result.status,
        cookie_count = result.cookies.len(),
        body_preview = %truncate(&result.body, 120),
        "FlareSolverr statsig signer response"
    );

    if !(200..300).contains(&result.status) {
        return Err(format!(
            "FlareSolverr signer target HTTP {}: {}",
            result.status,
            truncate(&result.body, 240)
        ));
    }
    match parse_statsig_signature_body(&result.body) {
        Ok(value) => Ok(value),
        Err(error) => Err(format!(
            "{error}; http={}; body={}",
            result.status,
            truncate(&result.body, 240)
        )),
    }
}

fn signer_origin(signer: &str) -> String {
    reqwest::Url::parse(signer)
        .ok()
        .map(|url| {
            let host = url.host_str().unwrap_or("grok.wodf.de");
            match url.port() {
                Some(port) => format!("{}://{}:{port}", url.scheme(), host),
                None => format!("{}://{}", url.scheme(), host),
            }
        })
        .unwrap_or_else(|| "https://grok.wodf.de".to_string())
}

fn cloudflare_cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .filter(|(name, _)| is_cloudflare_cookie_name(name))
        .map(|(name, value)| format!("{}={}", name.to_ascii_lowercase(), value.trim()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn consume_upstream(bytes: &[u8]) -> Result<ParsedChat, String> {
    let mut parsed = ParsedChat::default();
    for frame in extract_json_objects(bytes) {
        apply_frame(&mut parsed, &frame)?;
    }
    Ok(parsed)
}

fn extract_json_objects(bytes: &[u8]) -> Vec<Vec<u8>> {
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

fn apply_frame(parsed: &mut ParsedChat, data: &[u8]) -> Result<(), String> {
    let Ok(root) = serde_json::from_slice::<Value>(data) else {
        return Ok(());
    };
    let Some(root) = root.as_object() else {
        return Ok(());
    };
    if let Some(error) = root.get("error") {
        if let Some(message) = error_message(error) {
            return Err(message);
        }
    }
    let Some(result) = root.get("result").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(response) = result.get("response").and_then(Value::as_object) else {
        return Ok(());
    };
    if let Some(error) = response.get("error") {
        if let Some(message) = error_message(error) {
            return Err(message);
        }
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
    if let Some(token) = response.get("token").and_then(Value::as_str) {
        if !thinking && (tag == "final" || tag.is_empty()) {
            parsed.text.push_str(token);
        }
    }
    if let Some(model_response) = response.get("modelResponse").and_then(Value::as_object) {
        if let Some(errors) = model_response.get("streamErrors").and_then(Value::as_array) {
            for item in errors {
                if let Some(message) = error_message(item) {
                    return Err(message);
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

fn merge_model_text(current: &mut String, message: &str) {
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

fn collect_search_sources(parsed: &mut ParsedChat, response: &Value) {
    collect_web_search_results(parsed, response.get("webSearchResults"));
    collect_web_search_results(parsed, response.get("citedWebSearchResults"));
    collect_x_search_results(parsed, response.get("xSearchResults"));
    collect_x_search_results(parsed, response.get("xposts"));
    collect_x_search_results(parsed, response.get("citedXposts"));
}

fn collect_web_search_results(parsed: &mut ParsedChat, value: Option<&Value>) {
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

fn collect_x_search_results(parsed: &mut ParsedChat, value: Option<&Value>) {
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

fn append_hit(parsed: &mut ParsedChat, hit: WebSearchHit) {
    if !parsed.seen_urls.insert(hit.url.clone()) {
        return;
    }
    parsed.sources.push(hit);
}

fn parse_structured_hits(text: &str, max_results: u32) -> Vec<WebSearchHit> {
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

fn value_to_hits(value: &Value, max_results: u32) -> Vec<WebSearchHit> {
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

fn parse_structured_answer(text: &str) -> Option<String> {
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

fn strip_markdown_fence(text: &str) -> String {
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

fn extract_json_array(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

fn error_message(value: &Value) -> Option<String> {
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

fn truncate(value: &str, max: usize) -> String {
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
    fn normalizes_sso_variants() {
        let jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.abc.def";
        assert_eq!(normalize_sso_token(jwt), jwt);
        assert_eq!(normalize_sso_token(&format!("sso={jwt}")), jwt);
    }

    #[test]
    fn parses_cookie_header_credential() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.aaa.bbb";
        let raw = format!("sso={jwt}; sso-rw={jwt}; cf_clearance=abc123; __cf_bm=zzz");
        let cred = parse_credential(&raw).unwrap();
        assert_eq!(cred.sso, jwt);
        assert_eq!(
            cred.extra_cookies.get("cf_clearance").map(String::as_str),
            Some("abc123")
        );
        let cookie = build_sso_cookie(&cred);
        assert!(cookie.contains("cf_clearance=abc123"));
    }

    #[test]
    fn parses_json_credential() {
        let raw = r#"{"sso":"eyJhbGciOiJIUzI1NiJ9.x.y","cookies":"cf_clearance=ccc","x_statsig_id":"abc","user_agent":"Mozilla/5.0"}"#;
        let cred = parse_credential(raw).unwrap();
        assert_eq!(cred.sso, "eyJhbGciOiJIUzI1NiJ9.x.y");
        assert_eq!(
            cred.extra_cookies.get("cf_clearance").map(String::as_str),
            Some("ccc")
        );
        assert_eq!(cred.statsig_id.as_deref(), Some("abc"));
    }

    #[test]
    fn extracts_meta_with_unicode_hyphen() {
        let html = r#"<meta name="grok-site―verification" content="META123"/>"#;
        assert_eq!(
            extract_statsig_meta_content(html).as_deref(),
            Some("META123")
        );
    }

    #[test]
    fn parses_structured_json_array() {
        let text = r#"[{"title":"Rust","url":"https://www.rust-lang.org","snippet":"lang"}]"#;
        let hits = parse_structured_hits(text, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust");
    }

    #[test]
    fn parses_grok_answer_with_optional_sources() {
        let text = r#"{"answer":"Rust is memory safe.","sources":[{"title":"Rust","url":"https://www.rust-lang.org","snippet":"Official site"}]}"#;
        let hits = parse_structured_hits(text, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://www.rust-lang.org");
        assert_eq!(
            parse_structured_answer(text).as_deref(),
            Some("Rust is memory safe.")
        );
    }

    #[test]
    fn search_prompt_prioritizes_answer_over_links() {
        let prompt = build_search_prompt(&ProviderSearchRequest {
            query: "Rust".to_string(),
            max_results: 3,
            mode: crate::tools::web_search::types::SearchMode::Fast,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            recency_days: None,
        });
        assert!(prompt.contains("\"answer\""));
        assert!(prompt.contains("sources array is optional"));
        assert!(prompt.contains("must never replace it"));
    }

    #[test]
    fn extracts_native_web_results_from_frames() {
        let frame = br#"{"result":{"response":{"webSearchResults":{"results":[{"title":"Example","url":"https://example.com","snippet":"hello"}]},"token":"done","messageTag":"final"}}}"#;
        let parsed = consume_upstream(frame).unwrap();
        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].url, "https://example.com");
    }

    #[test]
    fn extracts_x_posts() {
        let frame = br#"{"result":{"response":{"xSearchResults":{"results":[{"username":"openai","postId":"123","text":"hello x"}]}}}}"#;
        let parsed = consume_upstream(frame).unwrap();
        assert_eq!(parsed.sources[0].url, "https://x.com/openai/status/123");
    }

    #[test]
    fn detects_anti_bot_payload() {
        assert!(is_anti_bot_message(
            r#"{"error":{"code":7,"message":"Request rejected by anti-bot rules."}}"#
        ));
    }

    #[test]
    fn cloudflare_cookie_whitelist_only() {
        assert!(is_cloudflare_cookie_name("cf_clearance"));
        assert!(is_cloudflare_cookie_name("__cf_bm"));
        assert!(is_cloudflare_cookie_name("_cfuvid"));
        assert!(is_cloudflare_cookie_name("cf_chl_2"));
        assert!(!is_cloudflare_cookie_name("x-challenge"));
        assert!(!is_cloudflare_cookie_name("x-signature"));
        assert!(!is_cloudflare_cookie_name("x-anonuserid"));
        assert!(!is_cloudflare_cookie_name("grok_device_id"));
        assert!(is_allowed_extra_cookie("grok_device_id"));
        assert!(is_allowed_extra_cookie("x-userid"));

        let jwt = "eyJhbGciOiJIUzI1NiJ9.aaa.bbb";
        let cred = GrokCredential {
            sso: jwt.to_string(),
            extra_cookies: BTreeMap::from([
                ("cf_clearance".to_string(), "abc".to_string()),
                ("x-challenge".to_string(), "nope".to_string()),
                ("x-signature".to_string(), "nope".to_string()),
                ("__cf_bm".to_string(), "bm".to_string()),
                ("grok_device_id".to_string(), "dev".to_string()),
                ("x-userid".to_string(), "uid".to_string()),
            ]),
            statsig_id: None,
            user_agent: None,
            statsig_signer: None,
        };
        let cookie = build_sso_cookie(&cred);
        assert!(cookie.contains("cf_clearance=abc"));
        assert!(cookie.contains("__cf_bm=bm"));
        assert!(cookie.contains("grok_device_id=dev"));
        assert!(cookie.contains("x-userid=uid"));
        assert!(!cookie.contains("x-challenge"));
        assert!(!cookie.contains("x-signature"));
    }

    #[test]
    fn validates_statsig_id_length() {
        assert!(!valid_statsig_id("abc"));
        assert!(!valid_statsig_id("4.123.notreal"));
        let raw = vec![0u8; 70];
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&raw);
        assert!(valid_statsig_id(&encoded));
        let padded = base64::engine::general_purpose::STANDARD.encode(&raw);
        assert!(valid_statsig_id(&padded));
        assert!(is_x0_fallback_statsig_id(X0_FALLBACK_STATSIG_ID));
        assert!(usable_statsig_id(X0_FALLBACK_STATSIG_ID));
        // x0 payload is exactly 70 bytes after base64 decode, so it is also valid_statsig_id.
        assert!(valid_statsig_id(X0_FALLBACK_STATSIG_ID));
        assert_eq!(x0_fallback_statsig_id(), X0_FALLBACK_STATSIG_ID);
    }

    #[test]
    fn detects_transport_retryable_connect() {
        assert!(is_transport_retryable(
            "Grok request failed: error sending request for uri (https://grok.com/rest/app-chat/conversations/new): client error (Connect)"
        ));
        assert!(is_transport_retryable("operation timed out"));
        assert!(!is_transport_retryable("Grok HTTP 403: anti-bot"));
    }

    #[test]
    fn classifies_key_statuses_from_upstream_messages() {
        assert_eq!(
            classify_key_status(
                "Grok HTTP 403: body={\"message\":\"User is blocked [WKE=unauthorized:blocked-user]\"}"
            ),
            "officially_blocked"
        );
        assert_eq!(
            classify_key_status("Grok HTTP 429: quota exceeded"),
            "quota_limited"
        );
        assert_eq!(
            classify_key_status("Grok HTTP 401: unauthorized"),
            "unauthorized"
        );
        assert_eq!(
            classify_key_status("Grok HTTP 403: Request rejected by anti-bot rules."),
            "anti_bot"
        );
    }

    #[test]
    fn detects_cloudflare_challenge_html() {
        assert!(looks_like_cloudflare_challenge(
            "<!DOCTYPE html><html><title>Just a moment...</title></html>"
        ));
        let raw = vec![1u8; 70];
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&raw);
        let body = format!(r#"{{"x-statsig-id":"{encoded}"}}"#);
        assert_eq!(parse_statsig_signature_body(&body).unwrap(), encoded);
    }
}
