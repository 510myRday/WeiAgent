use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub openai_api_key: Option<String>,
    pub openai_base_url: String,
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            openai_api_key: env::var("OPENAI_API_KEY")
                .ok()
                .filter(|value| !value.is_empty()),
            openai_base_url: env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            host: env::var("WEIAGENT_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: env::var("WEIAGENT_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8080),
            data_dir: PathBuf::from("data"),
        }
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        format!("{}:{}", self.host, self.port).parse()
    }
}
