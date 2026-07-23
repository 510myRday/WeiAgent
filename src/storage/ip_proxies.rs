use std::collections::{HashMap, HashSet};

use chrono::Utc;
use reqwest::Url;
use uuid::Uuid;

use crate::{
    domain::{
        CreateIpProxyRequest, ImportIpProxiesRequest, ImportIpProxiesResponse, IpProxy,
        IpProxyProtocol, IpProxyStatus, IpProxyTestUpdate, UpdateIpProxyRequest,
    },
    error::{AppError, AppResult},
    storage::{db, json_store::JsonStore, validation::validate_non_empty},
};

impl JsonStore {
    pub async fn list_ip_proxies(&self) -> AppResult<Vec<IpProxy>> {
        let _guard = self.lock.lock().await;
        db::read_ip_proxies(&self.data_dir)
    }

    pub async fn create_ip_proxy(&self, request: CreateIpProxyRequest) -> AppResult<IpProxy> {
        let name = validate_non_empty("proxy name", &request.name)?;
        let (url, protocol) = normalize_proxy_url(&request.url)?;

        let _guard = self.lock.lock().await;
        let mut proxies = db::read_ip_proxies(&self.data_dir)?;
        ensure_unique_url(&proxies, &url, None)?;
        ensure_unique_name(&proxies, &name, None)?;

        let now = Utc::now();
        let proxy = IpProxy {
            id: Uuid::new_v4(),
            name,
            url,
            protocol,
            enabled: request.enabled,
            status: IpProxyStatus::Unchecked,
            latency_ms: None,
            last_checked_at: None,
            last_error: None,
            ip_address: None,
            country: None,
            country_code: None,
            region: None,
            city: None,
            created_at: now,
            updated_at: now,
        };
        proxies.push(proxy.clone());
        db::write_ip_proxies(&self.data_dir, &proxies)?;
        Ok(proxy)
    }

    pub async fn import_ip_proxies(
        &self,
        request: ImportIpProxiesRequest,
    ) -> AppResult<ImportIpProxiesResponse> {
        let _guard = self.lock.lock().await;
        let mut proxies = db::read_ip_proxies(&self.data_dir)?;
        let mut known_urls = proxies
            .iter()
            .map(|proxy| proxy.url.clone())
            .collect::<HashSet<_>>();
        let mut known_names = proxies
            .iter()
            .map(|proxy| proxy.name.trim().to_lowercase())
            .collect::<HashSet<_>>();
        let mut imported = Vec::new();

        for entry in request.entries {
            let (url, protocol) = normalize_proxy_url(&entry.url)?;
            if !known_urls.insert(url.clone()) {
                return Err(AppError::Conflict(format!(
                    "proxy address '{url}' is already configured"
                )));
            }

            let requested_name = entry
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let name = if let Some(requested_name) = requested_name {
                let name = validate_non_empty("proxy name", requested_name)?;
                if !known_names.insert(name.to_lowercase()) {
                    return Err(AppError::Conflict(format!(
                        "proxy name '{name}' is already configured"
                    )));
                }
                name
            } else {
                unique_proxy_name(default_proxy_name(&url), &mut known_names)
            };
            let now = Utc::now();
            let proxy = IpProxy {
                id: Uuid::new_v4(),
                name,
                url,
                protocol,
                enabled: request.enabled,
                status: IpProxyStatus::Unchecked,
                latency_ms: None,
                last_checked_at: None,
                last_error: None,
                ip_address: None,
                country: None,
                country_code: None,
                region: None,
                city: None,
                created_at: now,
                updated_at: now,
            };
            proxies.push(proxy.clone());
            imported.push(proxy);
        }

        if !imported.is_empty() {
            db::write_ip_proxies(&self.data_dir, &proxies)?;
        }
        Ok(ImportIpProxiesResponse {
            imported_count: imported.len(),
            duplicate_count: 0,
            invalid_count: 0,
            proxies: imported,
        })
    }

    pub async fn update_ip_proxy(
        &self,
        id: Uuid,
        request: UpdateIpProxyRequest,
    ) -> AppResult<IpProxy> {
        let _guard = self.lock.lock().await;
        let mut proxies = db::read_ip_proxies(&self.data_dir)?;
        let index = proxies
            .iter()
            .position(|proxy| proxy.id == id)
            .ok_or(AppError::NotFound)?;

        if let Some(name) = request.name {
            let name = validate_non_empty("proxy name", &name)?;
            ensure_unique_name(&proxies, &name, Some(id))?;
            proxies[index].name = name;
        }
        if let Some(raw_url) = request.url {
            let (url, protocol) = normalize_proxy_url(&raw_url)?;
            ensure_unique_url(&proxies, &url, Some(id))?;
            if proxies[index].url != url {
                proxies[index].url = url;
                proxies[index].protocol = protocol;
                proxies[index].status = IpProxyStatus::Unchecked;
                proxies[index].latency_ms = None;
                proxies[index].last_checked_at = None;
                proxies[index].last_error = None;
                proxies[index].ip_address = None;
                proxies[index].country = None;
                proxies[index].country_code = None;
                proxies[index].region = None;
                proxies[index].city = None;
            }
        }
        if let Some(enabled) = request.enabled {
            proxies[index].enabled = enabled;
        }
        proxies[index].updated_at = Utc::now();

        let updated = proxies[index].clone();
        db::write_ip_proxies(&self.data_dir, &proxies)?;
        Ok(updated)
    }

    pub async fn delete_ip_proxy(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.lock.lock().await;
        let mut proxies = db::read_ip_proxies(&self.data_dir)?;
        let original_len = proxies.len();
        proxies.retain(|proxy| proxy.id != id);
        if proxies.len() == original_len {
            return Err(AppError::NotFound);
        }
        db::write_ip_proxies(&self.data_dir, &proxies)
    }

    pub async fn ip_proxies_for_test(&self, ids: &[Uuid]) -> AppResult<Vec<IpProxy>> {
        let proxies = self.list_ip_proxies().await?;
        if ids.is_empty() {
            return Ok(proxies.into_iter().filter(|proxy| proxy.enabled).collect());
        }

        let proxies_by_id = proxies
            .into_iter()
            .map(|proxy| (proxy.id, proxy))
            .collect::<HashMap<_, _>>();
        let mut seen = HashSet::new();
        ids.iter()
            .filter(|id| seen.insert(**id))
            .map(|id| proxies_by_id.get(id).cloned().ok_or(AppError::NotFound))
            .collect()
    }

    pub async fn apply_ip_proxy_test_updates(
        &self,
        updates: &[IpProxyTestUpdate],
    ) -> AppResult<Vec<IpProxy>> {
        let _guard = self.lock.lock().await;
        let mut proxies = db::read_ip_proxies(&self.data_dir)?;
        let updates_by_id = updates
            .iter()
            .map(|update| (update.id, update))
            .collect::<HashMap<_, _>>();
        let mut updated = Vec::with_capacity(updates.len());

        for proxy in &mut proxies {
            let Some(update) = updates_by_id.get(&proxy.id) else {
                continue;
            };
            proxy.status = update.status;
            proxy.latency_ms = update.latency_ms;
            proxy.last_checked_at = Some(update.checked_at);
            proxy.last_error = update.error.clone();
            proxy.ip_address = update.ip_address.clone();
            proxy.country = update.country.clone();
            proxy.country_code = update.country_code.clone();
            proxy.region = update.region.clone();
            proxy.city = update.city.clone();
            proxy.updated_at = Utc::now();
            updated.push(proxy.clone());
        }

        if updated.len() != updates.len() {
            return Err(AppError::NotFound);
        }
        db::write_ip_proxies(&self.data_dir, &proxies)?;
        Ok(updated)
    }
}

fn normalize_proxy_url(value: &str) -> AppResult<(String, IpProxyProtocol)> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::BadRequest(
            "proxy URL cannot be empty".to_string(),
        ));
    }
    let candidate = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{value}")
    };
    let mut parsed = Url::parse(&candidate)
        .map_err(|_| AppError::BadRequest("proxy URL is invalid".to_string()))?;
    if parsed.scheme() == "socks" {
        parsed
            .set_scheme("socks5")
            .map_err(|_| AppError::BadRequest("proxy scheme is invalid".to_string()))?;
    }
    let protocol = match parsed.scheme() {
        "http" => IpProxyProtocol::Http,
        "https" => IpProxyProtocol::Https,
        "socks5" | "socks5h" => IpProxyProtocol::Socks5,
        _ => {
            return Err(AppError::BadRequest(
                "proxy URL must use http, https, socks5, or socks5h".to_string(),
            ));
        }
    };
    if parsed.host_str().is_none() {
        return Err(AppError::BadRequest(
            "proxy URL must include a host".to_string(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() || !matches!(parsed.path(), "" | "/")
    {
        return Err(AppError::BadRequest(
            "proxy URL cannot include a path, query, or fragment".to_string(),
        ));
    }

    let mut normalized = parsed.to_string();
    if normalized.ends_with('/') {
        normalized.pop();
    }
    Ok((normalized, protocol))
}

fn ensure_unique_url(proxies: &[IpProxy], url: &str, current_id: Option<Uuid>) -> AppResult<()> {
    if proxies
        .iter()
        .any(|proxy| proxy.id != current_id.unwrap_or(Uuid::nil()) && proxy.url == url)
    {
        return Err(AppError::Conflict(
            "proxy address is already configured".to_string(),
        ));
    }
    Ok(())
}

fn ensure_unique_name(proxies: &[IpProxy], name: &str, current_id: Option<Uuid>) -> AppResult<()> {
    let normalized = name.trim().to_lowercase();
    if proxies
        .iter()
        .any(|proxy| Some(proxy.id) != current_id && proxy.name.trim().to_lowercase() == normalized)
    {
        return Err(AppError::Conflict(format!(
            "proxy name '{}' is already configured",
            name.trim()
        )));
    }
    Ok(())
}

fn default_proxy_name(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| {
            url.host_str().map(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            })
        })
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "proxy".to_string())
}

fn unique_proxy_name(base_name: String, known_names: &mut HashSet<String>) -> String {
    if known_names.insert(base_name.to_lowercase()) {
        return base_name;
    }

    for index in 1_u64.. {
        let candidate = format!("{base_name}_{index}");
        if known_names.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CreateIpProxyRequest, ImportIpProxiesRequest, ip_proxies::ImportIpProxyEntry,
    };

    #[test]
    fn proxy_urls_are_normalized_and_classified() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7890").unwrap(),
            ("http://127.0.0.1:7890".to_string(), IpProxyProtocol::Http)
        );
        assert_eq!(
            normalize_proxy_url("socks://proxy.test:1080").unwrap(),
            (
                "socks5://proxy.test:1080".to_string(),
                IpProxyProtocol::Socks5
            )
        );
        assert!(normalize_proxy_url("ftp://proxy.test:21").is_err());
        assert!(normalize_proxy_url("http://proxy.test/path").is_err());
    }

    #[test]
    fn generated_names_use_hosts_and_increment_duplicates() {
        let mut known = HashSet::from(["proxy.test".to_string()]);
        assert_eq!(default_proxy_name("http://127.0.0.1:7890"), "127.0.0.1");
        assert_eq!(default_proxy_name("socks5://proxy.test:1080"), "proxy.test");
        assert_eq!(
            unique_proxy_name("proxy.test".to_string(), &mut known),
            "proxy.test_1"
        );
        assert_eq!(
            unique_proxy_name("proxy.test".to_string(), &mut known),
            "proxy.test_2"
        );
    }

    #[tokio::test]
    async fn imports_valid_unique_proxies_and_persists_them() {
        let root = tempfile::tempdir().unwrap();
        let store = JsonStore::new(root.path()).await.unwrap();
        let result = store
            .import_ip_proxies(ImportIpProxiesRequest {
                entries: vec![
                    ImportIpProxyEntry {
                        name: Some("Local".to_string()),
                        url: "127.0.0.1:7890".to_string(),
                    },
                    ImportIpProxyEntry {
                        name: None,
                        url: "http://proxy.test:8000".to_string(),
                    },
                    ImportIpProxyEntry {
                        name: None,
                        url: "socks5://proxy.test:8001".to_string(),
                    },
                ],
                enabled: true,
            })
            .await
            .unwrap();

        assert_eq!(result.imported_count, 3);
        assert_eq!(result.duplicate_count, 0);
        assert_eq!(result.invalid_count, 0);
        let saved = store.list_ip_proxies().await.unwrap();
        assert_eq!(saved.len(), 3);
        assert_eq!(saved[0].name, "Local");
        assert_eq!(saved[0].url, "http://127.0.0.1:7890");
        assert_eq!(saved[1].name, "proxy.test");
        assert_eq!(saved[2].name, "proxy.test_1");

        let duplicate_name = store
            .create_ip_proxy(CreateIpProxyRequest {
                name: "local".to_string(),
                url: "http://other.test:9000".to_string(),
                enabled: true,
            })
            .await;
        assert!(matches!(duplicate_name, Err(AppError::Conflict(_))));

        let conflicting_batch = store
            .import_ip_proxies(ImportIpProxiesRequest {
                entries: vec![ImportIpProxyEntry {
                    name: Some("Local".to_string()),
                    url: "http://another.test:9001".to_string(),
                }],
                enabled: true,
            })
            .await;
        assert!(matches!(conflicting_batch, Err(AppError::Conflict(_))));
        assert_eq!(store.list_ip_proxies().await.unwrap().len(), 3);
    }
}
