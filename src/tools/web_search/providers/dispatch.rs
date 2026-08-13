use std::time::Duration;

use crate::domain::WebSearchProviderKind;
use crate::tools::web_search::types::{
    ProviderError, ProviderExtractOutput, ProviderExtractRequest, ProviderSearchOutput,
    ProviderSearchRequest,
};

use super::{exa, firecrawl, grok, grok_support, jina, tavily};

pub struct GrokDispatch<'a> {
    pub source_id: &'a str,
    pub proxy_url: Option<&'a str>,
    pub timeout: Duration,
}

impl<'a> GrokDispatch<'a> {
    fn options(&self) -> grok::GrokSearchOptions<'a> {
        grok::GrokSearchOptions {
            source_id: self.source_id,
            proxy_url: self.proxy_url,
        }
    }
}

pub async fn dispatch_search(
    provider: WebSearchProviderKind,
    control_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
    grok: GrokDispatch<'_>,
) -> Result<ProviderSearchOutput, ProviderError> {
    match provider {
        WebSearchProviderKind::Tavily => {
            tavily::search(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Exa => {
            exa::search(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Jina => {
            jina::search(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Firecrawl => {
            firecrawl::search(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Grok => {
            let client = grok_support::build_emulated_client(grok.timeout, grok.proxy_url)
                .map_err(ProviderError::Retryable)?;
            grok::search(
                &client,
                control_client,
                base_url,
                api_key,
                request,
                grok.options(),
            )
            .await
        }
    }
}

pub async fn dispatch_extract(
    provider: WebSearchProviderKind,
    control_client: &reqwest::Client,
    fallback_client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
    grok: GrokDispatch<'_>,
) -> Result<ProviderExtractOutput, ProviderError> {
    match provider {
        WebSearchProviderKind::Tavily => {
            tavily::extract(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Exa => {
            exa::extract(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Jina => {
            jina::extract(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Firecrawl => {
            firecrawl::extract(control_client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Grok => {
            let client = grok_support::build_emulated_client(grok.timeout, grok.proxy_url)
                .map_err(ProviderError::Retryable)?;
            grok::extract(
                &client,
                fallback_client,
                base_url,
                api_key,
                request,
                grok.options(),
            )
            .await
        }
    }
}

pub async fn dispatch_probe(
    provider: WebSearchProviderKind,
    control_client: &reqwest::Client,
    grok_client: Option<&Result<wreq::Client, String>>,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
    grok: GrokDispatch<'_>,
) -> Result<(), String> {
    match provider {
        WebSearchProviderKind::Grok => match grok_client {
            Some(Ok(client)) => grok::probe(client, control_client, base_url, api_key, grok.options())
                .await
                .map_err(|error| error.message().to_string()),
            Some(Err(error)) => Err(error.clone()),
            None => Err("Grok emulated client missing".to_string()),
        },
        _ => dispatch_search(provider, control_client, base_url, api_key, request, grok)
            .await
            .map(|_| ())
            .map_err(|error| error.message().to_string()),
    }
}

pub fn classify_key_status(provider: WebSearchProviderKind, message: &str) -> &'static str {
    match provider {
        WebSearchProviderKind::Tavily => tavily::classify_key_status(message),
        WebSearchProviderKind::Exa => exa::classify_key_status(message),
        WebSearchProviderKind::Jina => jina::classify_key_status(message),
        WebSearchProviderKind::Grok => grok::classify_key_status(message),
        WebSearchProviderKind::Firecrawl => firecrawl::classify_key_status(message),
    }
}
