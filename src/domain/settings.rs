use serde::{Deserialize, Serialize};

pub const DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT: u8 = 90;
pub const MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT: u8 = 1;
pub const MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT: u8 = 99;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_avatar_icon")]
    pub avatar_icon: String,
    #[serde(default = "default_avatar_color")]
    pub avatar_color: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_request_retry_attempts")]
    pub request_retry_attempts: u32,
    /// Global auto-compaction threshold shared by every model and agent.
    #[serde(default = "default_context_compress_threshold_percent")]
    pub context_compress_threshold_percent: u8,
    #[serde(default = "default_llm_log_enabled")]
    pub llm_log_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            log_level: default_log_level(),
            avatar_icon: default_avatar_icon(),
            avatar_color: default_avatar_color(),
            language: default_language(),
            request_timeout_seconds: default_request_timeout_seconds(),
            request_retry_attempts: default_request_retry_attempts(),
            context_compress_threshold_percent: default_context_compress_threshold_percent(),
            llm_log_enabled: default_llm_log_enabled(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    18283
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_avatar_icon() -> String {
    "bot".to_string()
}

fn default_avatar_color() -> String {
    "#22c55e".to_string()
}

fn default_language() -> String {
    "zh".to_string()
}

fn default_request_timeout_seconds() -> u64 {
    60
}

fn default_request_retry_attempts() -> u32 {
    5
}

fn default_context_compress_threshold_percent() -> u8 {
    DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT
}

pub fn normalize_context_compress_threshold(value: u8) -> Result<u8, String> {
    if (MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT..=MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT)
        .contains(&value)
    {
        Ok(value)
    } else {
        Err(format!(
            "context_compress_threshold_percent must be between {MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT} and {MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT}"
        ))
    }
}

pub fn coerce_context_compress_threshold(value: u8) -> u8 {
    value.clamp(
        MIN_CONTEXT_COMPRESS_THRESHOLD_PERCENT,
        MAX_CONTEXT_COMPRESS_THRESHOLD_PERCENT,
    )
}

fn default_llm_log_enabled() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_threshold_defaults_and_rejects_out_of_range() {
        assert_eq!(
            AppSettings::default().context_compress_threshold_percent,
            DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT
        );
        assert!(normalize_context_compress_threshold(1).is_ok());
        assert!(normalize_context_compress_threshold(99).is_ok());
        assert!(normalize_context_compress_threshold(0).is_err());
        assert!(normalize_context_compress_threshold(100).is_err());
        assert_eq!(coerce_context_compress_threshold(0), 1);
        assert_eq!(coerce_context_compress_threshold(150), 99);
    }

    #[test]
    fn legacy_settings_json_defaults_global_threshold() {
        let settings: AppSettings = serde_json::from_value(serde_json::json!({
            "host": "127.0.0.1",
            "port": 18283,
            "log_level": "info",
            "avatar_icon": "bot",
            "avatar_color": "#22c55e",
            "language": "zh",
            "request_timeout_seconds": 60,
            "request_retry_attempts": 5,
            "llm_log_enabled": false
        }))
        .unwrap();
        assert_eq!(
            settings.context_compress_threshold_percent,
            DEFAULT_CONTEXT_COMPRESS_THRESHOLD_PERCENT
        );
    }
}
