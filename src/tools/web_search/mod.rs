use crate::domain::{
    WebSearchProviderKind, WebSearchToolConfig, ToolResult,
};
use crate::domain::tools::{MAX_WEB_SEARCH_QUERY_CHARS, MAX_WEB_SEARCH_RESULTS};

use self::key_pool::key_attempt_order;
use self::providers::{exa, grok, jina, tavily};
use crate::domain::tools::mask_api_key;

use self::types::{
    ProviderError, WebSearchKeyAttempt, WebSearchOutput, sanitize_error_message,
};

pub mod key_pool;
pub mod providers;
pub mod types;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchArgs {
    pub query: String,
    #[serde(default)]
    pub max_results: Option<u32>,
}

pub async fn execute_web_search(args: WebSearchArgs, config: &WebSearchToolConfig) -> ToolResult {
    let started = std::time::Instant::now();
    let query = args.query.trim();
    if query.is_empty() {
        return ToolResult::Error {
            message: "query must not be empty".to_string(),
        };
    }
    if query.chars().count() > MAX_WEB_SEARCH_QUERY_CHARS {
        return ToolResult::Error {
            message: format!("query must be at most {MAX_WEB_SEARCH_QUERY_CHARS} characters"),
        };
    }

    let max_results = args
        .max_results
        .unwrap_or_else(|| config.max_results())
        .clamp(1, MAX_WEB_SEARCH_RESULTS);

    let sources: Vec<_> = config.usable_sources().cloned().collect();
    if sources.is_empty() {
        return ToolResult::Error {
            message: "web_search has no enabled source with API keys".to_string(),
        };
    }

    let timeout = std::time::Duration::from_millis(config.timeout_ms());
    let control_client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(error) => {
            return ToolResult::Error {
                message: format!("failed to build HTTP client: {error}"),
            };
        }
    };

    let mut last_error: Option<String> = None;
    let mut key_attempts: Vec<WebSearchKeyAttempt> = Vec::new();

    for source in sources {
        let keys: Vec<String> = source
            .api_keys
            .iter()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        if keys.is_empty() {
            continue;
        }

        let order = key_attempt_order(&source.id, keys.len(), &config.key_selection);
        let base_url = source
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| source.provider.default_base_url())
            .to_string();
        let flaresolverr_url = source
            .flaresolverr_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let flaresolverr_proxy_url = source
            .flaresolverr_proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let proxy_url = source
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        // Non-Grok providers keep reqwest; Grok API uses wreq Chrome TLS emulation.
        // Grok+local proxy needs a higher floor than the global 15s web_search timeout.
        let grok_client = if source.provider == WebSearchProviderKind::Grok {
            let grok_timeout = std::cmp::max(timeout, std::time::Duration::from_secs(90));
            match providers::grok_support::build_emulated_client(grok_timeout, proxy_url) {
                Ok(client) => Some(client),
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            }
        } else {
            None
        };

        for index in order {
            let Some(api_key) = keys.get(index) else {
                continue;
            };
            let result = match source.provider {
                WebSearchProviderKind::Tavily => {
                    tavily::search(&control_client, &base_url, api_key, query, max_results).await
                }
                WebSearchProviderKind::Exa => {
                    exa::search(&control_client, &base_url, api_key, query, max_results).await
                }
                WebSearchProviderKind::Jina => {
                    jina::search(&control_client, &base_url, api_key, query, max_results).await
                }
                WebSearchProviderKind::Grok => {
                    let Some(grok_client) = grok_client.as_ref() else {
                        last_error = Some("Grok emulated client missing".to_string());
                        continue;
                    };
                    grok::search(
                        grok_client,
                        &control_client,
                        &base_url,
                        api_key,
                        query,
                        max_results,
                        flaresolverr_url,
                        flaresolverr_proxy_url,
                        proxy_url,
                    )
                    .await
                }
            };

            match result {
                Ok(hits) => {
                    key_attempts.push(WebSearchKeyAttempt {
                        provider: source.provider.as_str().to_string(),
                        source_id: source.id.clone(),
                        key_index: index,
                        key_hint: mask_api_key(api_key),
                        status: "normal".to_string(),
                        message: "request succeeded".to_string(),
                    });
                    let output = WebSearchOutput {
                        provider: source.provider.as_str().to_string(),
                        source_id: source.id.clone(),
                        query: query.to_string(),
                        result_count: hits.len(),
                        results: hits,
                        key_attempts,
                    };
                    let stdout = match serde_json::to_string_pretty(&output) {
                        Ok(value) => value,
                        Err(error) => {
                            return ToolResult::Error {
                                message: format!("failed to serialize search results: {error}"),
                            };
                        }
                    };
                    return ToolResult::Completed {
                        stdout,
                        stderr: String::new(),
                        exit_code: 0,
                        duration_ms: started.elapsed().as_millis() as u64,
                        runtime: Some("builtin_web_search".to_string()),
                        output_cursor: 0,
                    };
                }
                Err(ProviderError::Fatal(message)) => {
                    let message = sanitize_error_message(&message, api_key);
                    key_attempts.push(WebSearchKeyAttempt {
                        provider: source.provider.as_str().to_string(),
                        source_id: source.id.clone(),
                        key_index: index,
                        key_hint: mask_api_key(api_key),
                        status: key_status(source.provider.clone(), &message),
                        message: message.clone(),
                    });
                    return ToolResult::Error {
                        message: format_search_failure(&message, &key_attempts),
                    };
                }
                Err(ProviderError::Retryable(message)) => {
                    let message = sanitize_error_message(&message, api_key);
                    key_attempts.push(WebSearchKeyAttempt {
                        provider: source.provider.as_str().to_string(),
                        source_id: source.id.clone(),
                        key_index: index,
                        key_hint: mask_api_key(api_key),
                        status: key_status(source.provider.clone(), &message),
                        message: message.clone(),
                    });
                    last_error = Some(message);
                }
            }
        }
    }

    ToolResult::Error {
        message: format_search_failure(
            &last_error.unwrap_or_else(|| {
                "web_search failed: all configured sources and API keys were exhausted".to_string()
            }),
            &key_attempts,
        ),
    }
}

fn key_status(provider: WebSearchProviderKind, message: &str) -> String {
    if provider == WebSearchProviderKind::Grok {
        return grok::classify_key_status(message).to_string();
    }
    "error".to_string()
}

fn format_search_failure(message: &str, attempts: &[WebSearchKeyAttempt]) -> String {
    if attempts.is_empty() {
        return message.to_string();
    }
    let diagnostics = attempts
        .iter()
        .map(|attempt| {
            format!(
                "{}#{} {} {}: {}",
                attempt.provider,
                attempt.key_index + 1,
                attempt.key_hint,
                attempt.status,
                attempt.message
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!("{message}; key diagnostics: {diagnostics}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{WebSearchKeySelection, WebSearchSourceConfig, WebSearchToolConfig};

    fn futures_executor_block<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn empty_query_is_rejected() {
        let config = WebSearchToolConfig::default();
        let result = futures_executor_block(execute_web_search(
            WebSearchArgs {
                query: "   ".to_string(),
                max_results: None,
            },
            &config,
        ));
        assert!(matches!(result, ToolResult::Error { .. }));
    }

    #[test]
    fn no_keys_returns_error() {
        let config = WebSearchToolConfig {
            enabled: true,
            ..WebSearchToolConfig::default()
        };
        let result = futures_executor_block(execute_web_search(
            WebSearchArgs {
                query: "rust async".to_string(),
                max_results: Some(3),
            },
            &config,
        ));
        match result {
            ToolResult::Error { message } => {
                assert!(message.contains("no enabled source"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn usable_source_detection() {
        let mut config = WebSearchToolConfig::default();
        assert!(!config.has_usable_source());
        config.sources[0].api_keys = vec!["sk-test".to_string()];
        assert!(config.has_usable_source());
        config.sources[0].enabled = false;
        assert!(!config.has_usable_source());
    }

    #[test]
    fn key_selection_field_is_honored_by_order_helper() {
        let order = key_pool::key_attempt_order("x", 2, &WebSearchKeySelection::Sequential);
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn source_config_clone_keeps_provider() {
        let source = WebSearchSourceConfig {
            id: "tavily".to_string(),
            provider: WebSearchProviderKind::Tavily,
            enabled: true,
            base_url: None,
            api_keys: vec!["a".to_string()],
            key_statuses: Vec::new(),
            flaresolverr_url: None,
            flaresolverr_proxy_url: None,
            proxy_url: None,
        };
        assert_eq!(source.provider.as_str(), "tavily");
    }
}
