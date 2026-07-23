use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::{
        CreateIpProxyRequest, ImportIpProxiesRequest, ImportIpProxiesResponse, IpProxy,
        IpProxyQuality, IpProxyQualityProbe, IpProxyQualityStatus, IpProxyQualityTestUpdate,
        IpProxyStatus, IpProxyTestUpdate, TestIpProxiesRequest, UpdateIpProxyRequest,
    },
    error::AppResult,
};

const PROXY_GEO_TEST_URL: &str = "http://ip-api.com/json/?lang=zh-CN";
const PROXY_IP_TEST_URL: &str = "https://api64.ipify.org?format=json";
const PROXY_TEST_TIMEOUT: Duration = Duration::from_secs(12);
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_CONCURRENT_TESTS: usize = 8;
const QUALITY_GOOGLE_URL: &str =
    "https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta";
const QUALITY_OPENAI_URL: &str = "https://api.openai.com/v1/models";
const QUALITY_ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const QUALITY_GROK_URL: &str = "https://api.x.ai/v1/models";

pub async fn list_ip_proxies(State(state): State<AppState>) -> AppResult<Json<Vec<IpProxy>>> {
    Ok(Json(state.store.list_ip_proxies().await?))
}

pub async fn create_ip_proxy(
    State(state): State<AppState>,
    Json(request): Json<CreateIpProxyRequest>,
) -> AppResult<Json<IpProxy>> {
    Ok(Json(state.store.create_ip_proxy(request).await?))
}

pub async fn import_ip_proxies(
    State(state): State<AppState>,
    Json(request): Json<ImportIpProxiesRequest>,
) -> AppResult<Json<ImportIpProxiesResponse>> {
    Ok(Json(state.store.import_ip_proxies(request).await?))
}

pub async fn update_ip_proxy(
    State(state): State<AppState>,
    Path(proxy_id): Path<Uuid>,
    Json(request): Json<UpdateIpProxyRequest>,
) -> AppResult<Json<IpProxy>> {
    Ok(Json(state.store.update_ip_proxy(proxy_id, request).await?))
}

pub async fn delete_ip_proxy(
    State(state): State<AppState>,
    Path(proxy_id): Path<Uuid>,
) -> AppResult<()> {
    state.store.delete_ip_proxy(proxy_id).await
}

pub async fn test_ip_proxy(
    State(state): State<AppState>,
    Path(proxy_id): Path<Uuid>,
) -> AppResult<Json<IpProxy>> {
    let proxies = state.store.ip_proxies_for_test(&[proxy_id]).await?;
    let mut updated = run_proxy_tests_with_quality(&state, proxies).await?;
    Ok(Json(updated.remove(0)))
}

pub async fn test_ip_proxies(
    State(state): State<AppState>,
    Json(request): Json<TestIpProxiesRequest>,
) -> AppResult<Json<Vec<IpProxy>>> {
    let proxies = state.store.ip_proxies_for_test(&request.ids).await?;
    Ok(Json(run_proxy_tests_with_quality(&state, proxies).await?))
}

async fn run_proxy_tests_with_quality(
    state: &AppState,
    proxies: Vec<IpProxy>,
) -> AppResult<Vec<IpProxy>> {
    if proxies.is_empty() {
        return Ok(Vec::new());
    }
    let updates = stream::iter(proxies)
        .map(|proxy| async move {
            let quality_proxy = proxy.clone();
            tokio::join!(
                test_proxy_connection(proxy),
                test_proxy_quality(quality_proxy),
            )
        })
        .buffer_unordered(MAX_CONCURRENT_TESTS)
        .collect::<Vec<_>>()
        .await;
    let (test_updates, quality_updates): (Vec<_>, Vec<_>) = updates.into_iter().unzip();
    state
        .store
        .apply_ip_proxy_test_updates(&test_updates)
        .await?;
    state
        .store
        .apply_ip_proxy_quality_updates(&quality_updates)
        .await
}

async fn test_proxy_connection(proxy: IpProxy) -> IpProxyTestUpdate {
    let checked_at = Utc::now();
    let result = async {
        let reqwest_proxy = reqwest::Proxy::all(&proxy.url).map_err(|error| error.to_string())?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest_proxy)
            .connect_timeout(PROXY_CONNECT_TIMEOUT)
            .timeout(PROXY_TEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|error| error.to_string())?;
        probe_proxy(&client).await
    }
    .await;

    match result {
        Ok(result) => IpProxyTestUpdate {
            id: proxy.id,
            status: IpProxyStatus::Available,
            latency_ms: Some(result.latency_ms),
            checked_at,
            error: None,
            ip_address: result.ip_address,
            country: result.country,
            country_code: result.country_code,
            region: result.region,
            city: result.city,
        },
        Err(error) => IpProxyTestUpdate {
            id: proxy.id,
            status: IpProxyStatus::Unavailable,
            latency_ms: None,
            checked_at,
            error: Some(compact_error(error)),
            ip_address: None,
            country: None,
            country_code: None,
            region: None,
            city: None,
        },
    }
}

#[derive(Clone, Copy)]
enum QualityApi {
    Google,
    OpenAi,
    Anthropic,
    Grok,
}

impl QualityApi {
    fn url(self) -> &'static str {
        match self {
            Self::Google => QUALITY_GOOGLE_URL,
            Self::OpenAi => QUALITY_OPENAI_URL,
            Self::Anthropic => QUALITY_ANTHROPIC_URL,
            Self::Grok => QUALITY_GROK_URL,
        }
    }

    fn reachable_status(self, status: reqwest::StatusCode) -> bool {
        if status.is_success() {
            return true;
        }
        match self {
            Self::Google => status == reqwest::StatusCode::FORBIDDEN,
            Self::OpenAi | Self::Grok => status == reqwest::StatusCode::UNAUTHORIZED,
            Self::Anthropic => matches!(
                status,
                reqwest::StatusCode::BAD_REQUEST
                    | reqwest::StatusCode::UNAUTHORIZED
                    | reqwest::StatusCode::NOT_FOUND
                    | reqwest::StatusCode::METHOD_NOT_ALLOWED
            ),
        }
    }
}

async fn test_proxy_quality(proxy: IpProxy) -> IpProxyQualityTestUpdate {
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(match reqwest::Proxy::all(&proxy.url) {
            Ok(proxy) => proxy,
            Err(error) => {
                let message = compact_error(error.to_string());
                return IpProxyQualityTestUpdate {
                    id: proxy.id,
                    api_quality: IpProxyQuality {
                        google: quality_error(message.clone()),
                        openai: quality_error(message.clone()),
                        anthropic: quality_error(message.clone()),
                        grok: quality_error(message),
                    },
                };
            }
        })
        .connect_timeout(PROXY_CONNECT_TIMEOUT)
        .timeout(PROXY_TEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build();

    let api_quality = match client {
        Ok(client) => {
            let (google, openai, anthropic, grok) = tokio::join!(
                probe_quality_api(&client, QualityApi::Google),
                probe_quality_api(&client, QualityApi::OpenAi),
                probe_quality_api(&client, QualityApi::Anthropic),
                probe_quality_api(&client, QualityApi::Grok),
            );
            IpProxyQuality {
                google,
                openai,
                anthropic,
                grok,
            }
        }
        Err(error) => {
            let message = compact_error(error.to_string());
            IpProxyQuality {
                google: quality_error(message.clone()),
                openai: quality_error(message.clone()),
                anthropic: quality_error(message.clone()),
                grok: quality_error(message),
            }
        }
    };

    IpProxyQualityTestUpdate {
        id: proxy.id,
        api_quality,
    }
}

async fn probe_quality_api(client: &reqwest::Client, api: QualityApi) -> IpProxyQualityProbe {
    let request = client
        .get(api.url())
        .header(reqwest::header::USER_AGENT, "WeiAgent proxy quality check");

    let started = Instant::now();
    match request.send().await {
        Ok(response) if api.reachable_status(response.status()) => {
            let status = response.status();
            if let Err(error) = response.bytes().await {
                return quality_error(compact_error(error.to_string()));
            }
            IpProxyQualityProbe {
                status: IpProxyQualityStatus::Available,
                latency_ms: Some(elapsed_millis(started)),
                error: (!status.is_success()).then(|| format!("HTTP {status}（目标可达，无认证）")),
            }
        }
        Ok(response) => quality_error(format!("HTTP {}", response.status())),
        Err(error) => quality_error(compact_error(error.to_string())),
    }
}

fn quality_error(error: String) -> IpProxyQualityProbe {
    IpProxyQualityProbe {
        status: IpProxyQualityStatus::Error,
        latency_ms: None,
        error: Some(error),
    }
}

#[derive(Debug)]
struct ProxyProbeResult {
    latency_ms: u64,
    ip_address: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    city: Option<String>,
}

#[derive(Deserialize)]
struct IpApiProbeResponse {
    status: String,
    message: Option<String>,
    query: Option<String>,
    country: Option<String>,
    #[serde(rename = "countryCode")]
    country_code: Option<String>,
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    region: Option<String>,
    city: Option<String>,
}

#[derive(Deserialize)]
struct IpifyProbeResponse {
    ip: String,
}

async fn probe_proxy(client: &reqwest::Client) -> Result<ProxyProbeResult, String> {
    match probe_ip_api(client).await {
        Ok(result) => Ok(result),
        Err(geo_error) => probe_ipify(client)
            .await
            .map_err(|ip_error| format!("{geo_error}; fallback failed: {ip_error}")),
    }
}

async fn probe_ip_api(client: &reqwest::Client) -> Result<ProxyProbeResult, String> {
    let started = Instant::now();
    let response = client
        .get(PROXY_GEO_TEST_URL)
        .header(reqwest::header::USER_AGENT, "WeiAgent proxy check")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("geo probe returned HTTP {status}"));
    }
    let payload = response
        .json::<IpApiProbeResponse>()
        .await
        .map_err(|error| format!("invalid geo response: {error}"))?;
    if !payload.status.eq_ignore_ascii_case("success") {
        return Err(payload
            .message
            .unwrap_or_else(|| "geo probe reported failure".to_string()));
    }
    Ok(ProxyProbeResult {
        latency_ms: elapsed_millis(started),
        ip_address: clean_optional(payload.query),
        country: clean_optional(payload.country),
        country_code: clean_optional(payload.country_code),
        region: clean_optional(payload.region_name).or_else(|| clean_optional(payload.region)),
        city: clean_optional(payload.city),
    })
}

async fn probe_ipify(client: &reqwest::Client) -> Result<ProxyProbeResult, String> {
    let started = Instant::now();
    let response = client
        .get(PROXY_IP_TEST_URL)
        .header(reqwest::header::USER_AGENT, "WeiAgent proxy check")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("IP probe returned HTTP {status}"));
    }
    let payload = response
        .json::<IpifyProbeResponse>()
        .await
        .map_err(|error| format!("invalid IP response: {error}"))?;
    Ok(ProxyProbeResult {
        latency_ms: elapsed_millis(started),
        ip_address: clean_optional(Some(payload.ip)),
        country: None,
        country_code: None,
        region: None,
        city: None,
    })
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn compact_error(value: String) -> String {
    const MAX_CHARS: usize = 240;
    let mut chars = value.chars();
    let compact = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{compact}...")
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::{
        QUALITY_ANTHROPIC_URL, QUALITY_GOOGLE_URL, QUALITY_GROK_URL, QUALITY_OPENAI_URL, QualityApi,
    };

    #[test]
    fn quality_targets_use_official_no_key_endpoints() {
        assert_eq!(QualityApi::Google.url(), QUALITY_GOOGLE_URL);
        assert_eq!(QualityApi::OpenAi.url(), QUALITY_OPENAI_URL);
        assert_eq!(QualityApi::Anthropic.url(), QUALITY_ANTHROPIC_URL);
        assert_eq!(QualityApi::Grok.url(), QUALITY_GROK_URL);
    }

    #[test]
    fn expected_auth_responses_count_as_reachable() {
        assert!(QualityApi::Google.reachable_status(reqwest::StatusCode::FORBIDDEN));
        assert!(QualityApi::OpenAi.reachable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(QualityApi::Anthropic.reachable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(QualityApi::Grok.reachable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!QualityApi::OpenAi.reachable_status(reqwest::StatusCode::BAD_GATEWAY));
    }
}
