use serde_json::Value;
use std::time::Duration;
use wreq::StatusCode;

use super::grok_support::{GrokError, is_transport_retryable};
use super::support::truncate;
use crate::tools::web_search::canonicalize_url;
use crate::tools::web_search::types::{
    AiSynthesis, ProviderError, ProviderExtractDocument, ProviderExtractFailure,
    ProviderExtractOutput, ProviderExtractRequest, ProviderSearchOutput, ProviderSearchRequest,
    SearchHitKind, sanitize_error_message,
};

mod classify;
mod constants;
mod credentials;
mod parse;
mod prompts;
mod statsig;

use classify::{classify_upstream_error, is_anti_bot_message};
use constants::{CONNECTIVITY_PROBE_PROMPT, DEFAULT_MODE};
use credentials::{
    GrokCredential, build_request_headers, drop_foreign_clearance_cookie, parse_credential,
};
use parse::{
    ParsedChat, consume_upstream, extract_json_object, parse_structured_answer,
    parse_structured_hits, strip_markdown_fence,
};
use prompts::{build_extract_prompt, build_search_prompt, build_web_chat_payload};
use statsig::{invalidate_statsig_cache, resolve_statsig_id, warm_session_cookies};

pub use classify::classify_key_status;

#[derive(Debug, Clone, Copy)]
pub struct GrokSearchOptions<'a> {
    pub source_id: &'a str,
    pub proxy_url: Option<&'a str>,
}

pub async fn search(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
    options: GrokSearchOptions<'_>,
) -> Result<ProviderSearchOutput, ProviderError> {
    let prompt = build_search_prompt(request);
    let parsed = execute_prompt(
        client,
        control_client,
        base_url,
        api_key,
        &prompt,
        options,
        false,
    )
    .await?;
    map_hits(
        parsed,
        &request.query,
        request.max_results,
        options.source_id,
    )
}

pub async fn extract(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
    options: GrokSearchOptions<'_>,
) -> Result<ProviderExtractOutput, ProviderError> {
    let prompt = build_extract_prompt(request);
    let parsed = execute_prompt(
        client,
        control_client,
        base_url,
        api_key,
        &prompt,
        options,
        false,
    )
    .await?;
    map_extract(parsed, request)
}

/// Lightweight authenticated-chat probe used by the Key settings screen.
/// It keeps the full Grok session, cookie, Statsig, proxy and anti-bot path,
/// while disabling web search so a connectivity check does not wait for a
/// search result or consume a search operation.
pub async fn probe(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    options: GrokSearchOptions<'_>,
) -> Result<(), ProviderError> {
    execute_prompt(
        client,
        control_client,
        base_url,
        api_key,
        CONNECTIVITY_PROBE_PROMPT,
        options,
        true,
    )
    .await
    .map(|_| ())
}

async fn execute_prompt(
    client: &wreq::Client,
    control_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    prompt: &str,
    options: GrokSearchOptions<'_>,
    disable_search: bool,
) -> Result<ParsedChat, ProviderError> {
    let mut cred = parse_credential(api_key).map_err(|error| error.into_provider_error(api_key))?;
    if cred.sso.is_empty() {
        return Err(ProviderError::Retryable(
            "Grok SSO token is empty".to_string(),
        ));
    }

    let base = base_url.trim_end_matches('/');
    let endpoint = format!("{base}/rest/app-chat/conversations/new");
    let body = build_web_chat_payload(prompt, DEFAULT_MODE, disable_search);
    // API traffic proxy is applied on wreq client in web_search::mod; also used for statsig signer egress.
    let proxy = super::grok_support::normalize_optional_url(options.proxy_url);

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
        )
        .await
        .ok()
        .flatten();

        match execute_chat(client, base, &endpoint, &cred, statsig.as_deref(), &body).await {
            Ok(bytes) => {
                let parsed =
                    consume_upstream(&bytes).map_err(|error| error.into_provider_error(api_key))?;
                if let Some(error) = parsed.error.as_deref() {
                    if is_anti_bot_message(error) {
                        tracing::warn!(%error, "Grok chat rejected by anti-bot; skipping same-key retry");
                    }
                    return Err(classify_upstream_error(error, api_key));
                }
                return Ok(parsed);
            }
            Err(error) => {
                if error.is_retryable_transport() && attempt < 2 {
                    tracing::warn!(
                        attempt,
                        message = %error,
                        transport = true,
                        "Grok chat transport failed; retrying"
                    );
                    last_error = error.message().to_string();
                    tokio::time::sleep(Duration::from_millis(350 * (attempt as u64 + 1))).await;
                    continue;
                }
                return Err(error.into_provider_error(&cred.sso));
            }
        }
    }

    Err(ProviderError::Retryable(sanitize_error_message(
        &format!(
            "Grok request rejected (anti-bot or transport). Prefer stable proxy_url, longer timeout_ms (>=60000), SSO cookies, and usable x-statsig-id (70-byte or x0 fallback). detail={last_error}"
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
) -> Result<Vec<u8>, GrokError> {
    let headers = build_request_headers(base, cred, statsig)?;

    // Chrome TLS/JA3/H2 via wreq emulation (scheme B); cookies come from the SSO credential.
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
                    return Err(GrokError::transport(format!(
                        "Grok request failed: {last_send_error}"
                    )));
                }
            }
        }
        response.ok_or_else(|| {
            GrokError::transport(format!("Grok request failed: {last_send_error}"))
        })?
    };

    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| GrokError::transport(format!("Grok response body error: {error}")))?;
    let body_text = String::from_utf8_lossy(&bytes);

    if status == StatusCode::UNAUTHORIZED {
        return Err(GrokError::Http {
            message: format!(
                "Grok HTTP {status}: SSO unauthorized; body={}",
                truncate(&body_text, 240)
            ),
        });
    }
    if status == StatusCode::FORBIDDEN || is_anti_bot_message(&body_text) {
        return Err(GrokError::AntiBot(format!(
            "Grok HTTP {status}: anti-bot/forbidden; body={}",
            truncate(&body_text, 240)
        )));
    }
    // 429, 5xx and any other non-success status share the same message shape.
    if !status.is_success() {
        return Err(GrokError::Http {
            message: format!("Grok HTTP {status}: {}", truncate(&body_text, 240)),
        });
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

fn map_extract(
    parsed: ParsedChat,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    let target = canonicalize_url(&request.url).unwrap_or_else(|| request.url.clone());
    let cited_target = parsed.sources.iter().any(|source| {
        source.kind == SearchHitKind::GrokNativeSearchResult
            && canonicalize_url(&source.url).as_deref() == Some(target.as_str())
    });
    if !cited_target {
        return Ok(ProviderExtractOutput {
            documents: Vec::new(),
            failures: vec![ProviderExtractFailure {
                requested_url: request.url.clone(),
                message: "Grok did not cite the exact target URL from native web browsing"
                    .to_string(),
            }],
        });
    }
    let Some(mut document) = parse_structured_extract(&parsed.text, request) else {
        return Ok(ProviderExtractOutput {
            documents: Vec::new(),
            failures: vec![ProviderExtractFailure {
                requested_url: request.url.clone(),
                message: "Grok returned no valid structured extraction".to_string(),
            }],
        });
    };
    if canonicalize_url(&document.final_url).as_deref() != Some(target.as_str()) {
        return Ok(ProviderExtractOutput {
            documents: Vec::new(),
            failures: vec![ProviderExtractFailure {
                requested_url: request.url.clone(),
                message: "Grok structured result referenced a different URL".to_string(),
            }],
        });
    }
    document.final_url = request.url.clone();
    Ok(ProviderExtractOutput {
        documents: vec![document],
        failures: Vec::new(),
    })
}

fn parse_structured_extract(
    text: &str,
    request: &ProviderExtractRequest,
) -> Option<ProviderExtractDocument> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidates = [
        trimmed.to_string(),
        strip_markdown_fence(trimmed),
        extract_json_object(trimmed).unwrap_or_default(),
    ];
    for candidate in candidates {
        let Ok(value) = serde_json::from_str::<Value>(candidate.trim()) else {
            continue;
        };
        let Some(object) = value.as_object() else {
            continue;
        };
        if object
            .get("access_status")
            .and_then(Value::as_str)
            .is_some_and(|status| !status.eq_ignore_ascii_case("success"))
        {
            continue;
        }
        let final_url = object
            .get("visited_url")
            .or_else(|| object.get("url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&request.url)
            .to_string();
        let summary = object
            .get("summary")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let excerpts = object
            .get("excerpts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::String(value) => Some(value.trim().to_string()),
                        Value::Object(object) => object
                            .get("quote")
                            .or_else(|| object.get("text"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned),
                        _ => None,
                    })
                    .filter(|value| !value.is_empty())
                    .take(6)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut parts = Vec::new();
        if let Some(summary) = summary {
            parts.push(truncate(summary, 1_200));
        }
        parts.extend(
            excerpts
                .into_iter()
                .map(|value| format!("Quote: {}", truncate(&value, 1_000))),
        );
        if parts.is_empty() {
            continue;
        }
        return Some(ProviderExtractDocument {
            requested_url: request.url.clone(),
            final_url,
            title: object
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&request.url)
                .to_string(),
            content: parts.join("\n\n"),
            content_format: "text".to_string(),
            published_at: object
                .get("published_at")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            ai_derived: true,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::constants::{
        X0_FALLBACK_STATSIG_ID, is_x0_fallback_statsig_id, usable_statsig_id, valid_statsig_id,
    };
    use super::credentials::{
        build_sso_cookie, is_allowed_extra_cookie, is_cloudflare_cookie_name, normalize_sso_token,
    };
    use super::statsig::{
        extract_statsig_meta_content, looks_like_cloudflare_challenge, parse_statsig_signature_body,
    };
    use super::*;
    use crate::tools::web_search::types::WebSearchHit;
    use base64::Engine as _;
    use std::collections::BTreeMap;

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
    fn grok_extract_requires_native_target_citation() {
        let request = ProviderExtractRequest {
            url: "https://example.com/article".to_string(),
            query: "important facts".to_string(),
            hard_page: true,
            timeout_ms: 90_000,
        };
        let mut parsed = ParsedChat {
            text: r#"{"access_status":"success","visited_url":"https://example.com/article","title":"Article","summary":"Summary","excerpts":[{"quote":"Exact quote"}]}"#.to_string(),
            ..ParsedChat::default()
        };
        parsed.sources.push(WebSearchHit {
            title: "Article".to_string(),
            url: request.url.clone(),
            snippet: String::new(),
            content_excerpt: None,
            kind: SearchHitKind::GrokNativeSearchResult,
            published_at: None,
            score: None,
        });
        let output = map_extract(parsed, &request).unwrap();
        assert_eq!(output.documents.len(), 1);
        assert!(output.documents[0].ai_derived);

        let missing_citation = map_extract(
            ParsedChat {
                text: r#"{"access_status":"success","visited_url":"https://example.com/article","title":"Article","summary":"Summary","excerpts":[]}"#.to_string(),
                ..ParsedChat::default()
            },
            &request,
        )
        .unwrap();
        assert!(missing_citation.documents.is_empty());
        assert!(!missing_citation.failures.is_empty());
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
