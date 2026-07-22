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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_attempts: Vec<WebSearchKeyAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchKeyAttempt {
    pub provider: String,
    pub source_id: String,
    pub key_index: usize,
    pub key_hint: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum ProviderError {
    /// Retry with next key / source.
    Retryable(String),
    /// Fail the whole tool call without rotating further (e.g. invalid query).
    Fatal(String),
}

impl ProviderError {
    pub fn message(&self) -> &str {
        match self {
            Self::Retryable(message) | Self::Fatal(message) => message,
        }
    }
}

pub fn sanitize_error_message(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return message.to_string();
    }
    message.replace(api_key, "[redacted]")
}

