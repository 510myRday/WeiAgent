use std::borrow::Cow;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

pub(crate) fn proxy_url_with_remote_dns(value: &str) -> Cow<'_, str> {
    let value = value.trim();
    let Some(separator) = value.find("://") else {
        return Cow::Borrowed(value);
    };
    let scheme = &value[..separator];
    if !scheme.eq_ignore_ascii_case("socks") && !scheme.eq_ignore_ascii_case("socks5") {
        return Cow::Borrowed(value);
    }

    Cow::Owned(format!("socks5h://{}", &value[separator + 3..]))
}

fn serialize_proxy_url<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(proxy_url_with_remote_dns(value).as_ref())
}

pub(crate) fn serialize_optional_proxy_url<S>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(proxy_url_with_remote_dns(value).as_ref()),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpProxyProtocol {
    Http,
    Https,
    Socks4,
    Socks5,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpProxyStatus {
    #[default]
    Unchecked,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpProxyQualityStatus {
    #[default]
    Unchecked,
    Available,
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpProxyQualityProbe {
    #[serde(default)]
    pub status: IpProxyQualityStatus,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpProxyQuality {
    #[serde(default)]
    pub google: IpProxyQualityProbe,
    #[serde(default)]
    pub openai: IpProxyQualityProbe,
    #[serde(default)]
    pub anthropic: IpProxyQualityProbe,
    #[serde(default)]
    pub grok: IpProxyQualityProbe,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpProxy {
    pub id: Uuid,
    pub name: String,
    #[serde(serialize_with = "serialize_proxy_url")]
    pub url: String,
    pub protocol: IpProxyProtocol,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub status: IpProxyStatus,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub last_checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub ip_address: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub api_quality: IpProxyQuality,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateIpProxyRequest {
    pub name: String,
    pub url: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateIpProxyRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub is_pinned: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ImportIpProxyEntry {
    pub name: Option<String>,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportIpProxiesRequest {
    #[serde(default)]
    pub entries: Vec<ImportIpProxyEntry>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportIpProxiesResponse {
    pub imported_count: usize,
    pub duplicate_count: usize,
    pub invalid_count: usize,
    pub proxies: Vec<IpProxy>,
}

#[derive(Debug, Clone)]
pub struct IpProxyTestUpdate {
    pub id: Uuid,
    pub status: IpProxyStatus,
    pub latency_ms: Option<u64>,
    pub checked_at: DateTime<Utc>,
    pub error: Option<String>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IpProxyQualityTestUpdate {
    pub id: Uuid,
    pub api_quality: IpProxyQuality,
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::{IpProxy, IpProxyProtocol, proxy_url_with_remote_dns};

    #[test]
    fn requests_and_api_output_use_proxy_side_dns() {
        assert_eq!(
            proxy_url_with_remote_dns("socks5://proxy.test:1080"),
            "socks5h://proxy.test:1080"
        );
        assert_eq!(
            proxy_url_with_remote_dns(" SOCKS://proxy.test:1080 "),
            "socks5h://proxy.test:1080"
        );
        assert_eq!(
            proxy_url_with_remote_dns("socks5h://proxy.test:1080"),
            "socks5h://proxy.test:1080"
        );
        assert_eq!(
            proxy_url_with_remote_dns("https://proxy.test:8443"),
            "https://proxy.test:8443"
        );
    }

    #[test]
    fn api_serializes_socks5_as_socks5h_without_changing_stored_value() {
        let now = Utc::now();
        let proxy = IpProxy {
            id: Uuid::new_v4(),
            name: "proxy".to_string(),
            url: "socks5://proxy.test:1080".to_string(),
            protocol: IpProxyProtocol::Socks5,
            enabled: true,
            is_pinned: false,
            status: Default::default(),
            latency_ms: None,
            last_checked_at: None,
            last_error: None,
            ip_address: None,
            country: None,
            country_code: None,
            region: None,
            city: None,
            api_quality: Default::default(),
            created_at: now,
            updated_at: now,
        };

        let json = serde_json::to_value(&proxy).unwrap();
        assert_eq!(proxy.url, "socks5://proxy.test:1080");
        assert_eq!(json["url"], "socks5h://proxy.test:1080");
    }
}
