use serde::{Deserialize, Serialize};

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
