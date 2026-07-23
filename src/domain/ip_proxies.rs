use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpProxyProtocol {
    Http,
    Https,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpProxy {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub protocol: IpProxyProtocol,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
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

fn default_enabled() -> bool {
    true
}
