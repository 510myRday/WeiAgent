use std::{net::SocketAddr, path::PathBuf};

use tokio::fs;

use crate::{
    error::{AppError, AppResult},
    models::AppSettings,
};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub settings: AppSettings,
    pub data_dir: PathBuf,
}

impl AppConfig {
    pub async fn load(is_dev: bool) -> AppResult<Self> {
        let data_dir = data_dir(is_dev)?;
        fs::create_dir_all(&data_dir).await?;
        let settings = load_or_init_settings(&data_dir.join("settings.json")).await?;
        Ok(Self { settings, data_dir })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        format!("{}:{}", self.settings.host, self.settings.port).parse()
    }
}

fn data_dir(is_dev: bool) -> AppResult<PathBuf> {
    if is_dev {
        return Ok(PathBuf::from("data"));
    }

    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| AppError::BadRequest("failed to locate executable directory".to_string()))?;
    Ok(exe_dir.join("data"))
}

pub async fn load_or_init_settings(path: &std::path::Path) -> AppResult<AppSettings> {
    if fs::metadata(path).await.is_err() {
        write_settings(path, &AppSettings::default()).await?;
    }
    let content = fs::read_to_string(path).await?;
    let mut settings: AppSettings = serde_json::from_str(&content)?;
    validate_settings(&settings)?;
    settings.log_level = settings.log_level.trim().to_ascii_lowercase();
    write_settings(path, &settings).await?;
    Ok(settings)
}

pub async fn write_settings(path: &std::path::Path, settings: &AppSettings) -> AppResult<()> {
    validate_settings(settings)?;
    let content = serde_json::to_string_pretty(settings)?;
    fs::write(path, content).await?;
    Ok(())
}

pub fn validate_settings(settings: &AppSettings) -> AppResult<()> {
    if settings.host.trim().is_empty() {
        return Err(AppError::BadRequest("host cannot be empty".to_string()));
    }
    if settings.port == 0 {
        return Err(AppError::BadRequest("port cannot be 0".to_string()));
    }
    let level = settings.log_level.trim().to_ascii_lowercase();
    match level.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => Ok(()),
        _ => Err(AppError::BadRequest(
            "log_level must be trace, debug, info, warn, or error".to_string(),
        )),
    }
}

pub fn log_level_filter(value: &str) -> tracing_subscriber::filter::LevelFilter {
    match value.trim().to_ascii_lowercase().as_str() {
        "trace" => tracing_subscriber::filter::LevelFilter::TRACE,
        "debug" => tracing_subscriber::filter::LevelFilter::DEBUG,
        "warn" => tracing_subscriber::filter::LevelFilter::WARN,
        "error" => tracing_subscriber::filter::LevelFilter::ERROR,
        _ => tracing_subscriber::filter::LevelFilter::INFO,
    }
}
