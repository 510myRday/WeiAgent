use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearchOutput {
    pub provider: String,
    pub source_id: String,
    pub query: String,
    pub results: Vec<WebSearchHit>,
    pub result_count: usize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ProviderError {
    /// Retry with next key / source.
    Retryable(String),
    /// Fail the whole tool call without rotating further (e.g. invalid query).
    Fatal(String),
}

#[allow(dead_code)]
impl ProviderError {
    pub fn message(&self) -> &str {
        match self {
            Self::Retryable(message) | Self::Fatal(message) => message,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

pub fn sanitize_error_message(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return message.to_string();
    }
    message.replace(api_key, "[redacted]")
}
