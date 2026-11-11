use crate::domain::WebSearchProviderKind;
use crate::tools::web_search::types::{
    ProviderError, ProviderExtractOutput, ProviderExtractRequest, ProviderSearchOutput,
    ProviderSearchRequest,
};

use super::{exa, firecrawl, jina, keenable, tavily};

pub async fn dispatch_search(
    provider: WebSearchProviderKind,
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
) -> Result<ProviderSearchOutput, ProviderError> {
    match provider {
        WebSearchProviderKind::Tavily => tavily::search(client, base_url, api_key, request).await,
        WebSearchProviderKind::Exa => exa::search(client, base_url, api_key, request).await,
        WebSearchProviderKind::Jina => jina::search(client, base_url, api_key, request).await,
        WebSearchProviderKind::Firecrawl => {
            firecrawl::search(client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Keenable => {
            keenable::search(client, base_url, api_key, request).await
        }
    }
}

pub async fn dispatch_extract(
    provider: WebSearchProviderKind,
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderExtractRequest,
) -> Result<ProviderExtractOutput, ProviderError> {
    match provider {
        WebSearchProviderKind::Tavily => tavily::extract(client, base_url, api_key, request).await,
        WebSearchProviderKind::Exa => exa::extract(client, base_url, api_key, request).await,
        WebSearchProviderKind::Jina => jina::extract(client, base_url, api_key, request).await,
        WebSearchProviderKind::Firecrawl => {
            firecrawl::extract(client, base_url, api_key, request).await
        }
        WebSearchProviderKind::Keenable => {
            keenable::extract(client, base_url, api_key, request).await
        }
    }
}

pub async fn dispatch_probe(
    provider: WebSearchProviderKind,
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: &ProviderSearchRequest,
) -> Result<(), String> {
    dispatch_search(provider, client, base_url, api_key, request)
        .await
        .map(|_| ())
        .map_err(|error| error.message().to_string())
}

pub fn classify_key_status(provider: WebSearchProviderKind, message: &str) -> &'static str {
    match provider {
        WebSearchProviderKind::Tavily => tavily::classify_key_status(message),
        WebSearchProviderKind::Exa => exa::classify_key_status(message),
        WebSearchProviderKind::Jina => jina::classify_key_status(message),
        WebSearchProviderKind::Firecrawl => firecrawl::classify_key_status(message),
        WebSearchProviderKind::Keenable => keenable::classify_key_status(message),
    }
}
