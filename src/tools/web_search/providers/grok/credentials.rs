// Grok provider internals.
use serde_json::Value;
use std::collections::BTreeMap;

use wreq::header::{HeaderMap, HeaderName, HeaderValue};

use super::super::grok_support::GrokError;
use super::constants::{DEFAULT_USER_AGENT, usable_statsig_id};

#[derive(Debug, Clone)]
pub(super) struct GrokCredential {
    pub(super) sso: String,
    pub(super) extra_cookies: BTreeMap<String, String>,
    pub(super) statsig_id: Option<String>,
    pub(super) user_agent: Option<String>,
    pub(super) statsig_signer: Option<String>,
}

pub(super) fn parse_credential(raw: &str) -> Result<GrokCredential, GrokError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(GrokError::InvalidCredential("empty credential".to_string()));
    }

    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed).map_err(|error| {
            GrokError::InvalidCredential(format!("invalid Grok credential JSON: {error}"))
        })?;
        let obj = value.as_object().ok_or_else(|| {
            GrokError::InvalidCredential("Grok credential JSON must be an object".to_string())
        })?;
        let sso =
            super::super::support::string_field(obj, &["sso", "token", "auth_token", "api_key"])
                .map(normalize_sso_token)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    GrokError::InvalidCredential(
                        "Grok credential JSON missing sso/token".to_string(),
                    )
                })?;
        let mut extra = BTreeMap::new();
        if let Some(cookies) = super::super::support::string_field(
            obj,
            &["cookies", "cookie", "cf_cookies", "cloudflare"],
        ) {
            merge_cookie_header(&mut extra, cookies);
        }
        if let Some(cf) = super::super::support::string_field(obj, &["cf_clearance"]) {
            extra.insert("cf_clearance".to_string(), cf.to_string());
        }
        if let Some(bm) = super::super::support::string_field(obj, &["__cf_bm"]) {
            extra.insert("__cf_bm".to_string(), bm.to_string());
        }
        let statsig_id = super::super::support::string_field(
            obj,
            &["x_statsig_id", "x-statsig-id", "statsig_id", "statsig"],
        )
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
        let user_agent =
            super::super::support::string_field(obj, &["user_agent", "ua", "User-Agent"])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        let statsig_signer = super::super::support::string_field(
            obj,
            &["statsig_signer", "signer_url", "statsig_url"],
        )
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

pub(super) fn merge_cookie_header(map: &mut BTreeMap<String, String>, header: &str) {
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

pub(super) fn peel_cookie_pairs(value: &str) -> (String, BTreeMap<String, String>) {
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

pub(super) fn normalize_sso_token(value: &str) -> String {
    let mut value = value.trim().to_string();
    if value.to_ascii_lowercase().starts_with("sso=") {
        value = value[4..].trim().to_string();
    }
    if let Some((token, _)) = value.split_once(';') {
        value = token.trim().to_string();
    }
    value.replace(['\r', '\n', '\0'], "").trim().to_string()
}

pub(super) fn build_sso_cookie(cred: &GrokCredential) -> String {
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

pub(super) fn is_cloudflare_cookie_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "cf_clearance"
        || lower == "__cf_bm"
        || lower == "_cfuvid"
        || lower.starts_with("cf_chl_")
}

pub(super) fn is_session_identity_cookie_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "x-userid" | "grok_device_id"
    )
}

pub(super) fn is_allowed_extra_cookie(name: &str) -> bool {
    is_cloudflare_cookie_name(name) || is_session_identity_cookie_name(name)
}

/// Keep the __cf_bm/device cookies minted by this wreq client across retries.
/// Only clearance/challenge cookies are tied to a different browser session.
pub(super) fn drop_foreign_clearance_cookie(cred: &mut GrokCredential) {
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

pub(super) fn build_request_headers(
    base: &str,
    cred: &GrokCredential,
    statsig: Option<&str>,
) -> Result<HeaderMap, GrokError> {
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

pub(super) fn apply_chromium_client_hints(
    headers: &mut HeaderMap,
    user_agent: &str,
) -> Result<(), GrokError> {
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

pub(super) fn capture_version(user_agent: &str, markers: &[&str]) -> Option<String> {
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

pub(super) fn set_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
) -> Result<(), GrokError> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|error| GrokError::Other(format!("invalid header name {name}: {error}")))?;
    let header_value = HeaderValue::from_str(value)
        .map_err(|error| GrokError::Other(format!("invalid header value for {name}: {error}")))?;
    headers.insert(header_name, header_value);
    Ok(())
}
