use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Deserialize)]
pub struct TestIpProxiesRequest {
    #[serde(default)]
    pub ids: Vec<Uuid>,
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
