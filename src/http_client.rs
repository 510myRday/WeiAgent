use std::time::Duration;

use crate::domain::ip_proxies::proxy_url_with_remote_dns;

#[derive(Default)]
pub struct HttpClientConfig {
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub proxy_url: Option<String>,
    pub user_agent: Option<String>,
    pub follow_redirects: bool,
    pub max_redirects: Option<usize>,
    pub no_system_proxy: bool,
    pub identity: Option<reqwest::Identity>,
}

impl HttpClientConfig {
    pub fn new() -> Self {
        Self {
            follow_redirects: true,
            ..Self::default()
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    pub fn proxy_url(mut self, proxy_url: Option<&str>) -> Self {
        self.proxy_url = proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| proxy_url_with_remote_dns(value).into_owned());
        self
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    pub fn no_redirects(mut self) -> Self {
        self.follow_redirects = false;
        self
    }

    pub fn no_system_proxy(mut self) -> Self {
        self.no_system_proxy = true;
        self
    }

    pub fn max_redirects(mut self, limit: usize) -> Self {
        self.max_redirects = Some(limit);
        self
    }

    pub fn identity(mut self, identity: reqwest::Identity) -> Self {
        self.identity = Some(identity);
        self
    }
}

pub fn build_reqwest_client(config: HttpClientConfig) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder();
    if let Some(timeout) = config.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(timeout) = config.connect_timeout {
        builder = builder.connect_timeout(timeout);
    }
    if let Some(user_agent) = config.user_agent.as_deref() {
        builder = builder.user_agent(user_agent);
    }
    if let Some(limit) = config.max_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::limited(limit));
    } else if !config.follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    if let Some(identity) = config.identity {
        builder = builder.identity(identity);
    }
    if let Some(proxy_url) = config.proxy_url.as_deref() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|error| format!("failed to configure proxy: {error}"))?;
        if config.no_system_proxy {
            builder = builder.no_proxy();
        }
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))
}
