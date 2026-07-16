use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use tokio::fs;

use crate::{
    domain::AppSettings,
    error::{AppError, AppResult},
    fsutil,
};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub settings: AppSettings,
    pub data_dir: PathBuf,
}

impl AppConfig {
    pub async fn load(is_dev: bool) -> AppResult<Self> {
        let data_dir = resolve_data_dir(is_dev)?;
        fs::create_dir_all(&data_dir).await?;
        let settings = load_or_init_settings(&data_dir.join("settings.json")).await?;
        Ok(Self { settings, data_dir })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        self.settings
            .host
            .trim()
            .parse::<IpAddr>()
            .map(|host| SocketAddr::new(host, self.settings.port))
    }
}

/// Resolve the persistent data directory.
///
/// Priority:
/// 1. `WEIAGENT_DATA_DIR` absolute/relative override
/// 2. dev mode -> `./data`
/// 3. `WEIAGENT_PORTABLE=1` -> `<exe_dir>/data`
/// 4. user home folder (easy to find):
///    - Windows: `%USERPROFILE%\WeiAgent` (e.g. `C:\Users\Administrator\WeiAgent`)
///    - macOS/Linux: `$HOME/WeiAgent` (e.g. `/Users/name/WeiAgent`, `/home/name/WeiAgent`)
pub fn resolve_data_dir(is_dev: bool) -> AppResult<PathBuf> {
    if let Ok(override_dir) = std::env::var("WEIAGENT_DATA_DIR") {
        let path = PathBuf::from(override_dir.trim());
        if path.as_os_str().is_empty() {
            return Err(AppError::BadRequest(
                "WEIAGENT_DATA_DIR cannot be empty".to_string(),
            ));
        }
        return Ok(path);
    }

    if is_dev {
        return Ok(PathBuf::from("data"));
    }

    if env_flag_enabled("WEIAGENT_PORTABLE") {
        return portable_data_dir();
    }

    platform_user_data_dir()
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn portable_data_dir() -> AppResult<PathBuf> {
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| AppError::BadRequest("failed to locate executable directory".to_string()))?;
    Ok(exe_dir.join("data"))
}

fn platform_user_data_dir() -> AppResult<PathBuf> {
    Ok(user_home_dir()?.join("WeiAgent"))
}

fn user_home_dir() -> AppResult<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            AppError::BadRequest(
                "user home directory is unset; set WEIAGENT_DATA_DIR or WEIAGENT_PORTABLE=1"
                    .to_string(),
            )
        })?;
    let home = PathBuf::from(home);
    if home.as_os_str().is_empty() {
        return Err(AppError::BadRequest(
            "user home directory is empty; set WEIAGENT_DATA_DIR or WEIAGENT_PORTABLE=1"
                .to_string(),
        ));
    }
    Ok(home)
}

pub async fn load_or_init_settings(path: &std::path::Path) -> AppResult<AppSettings> {
    if fs::metadata(path).await.is_err() {
        write_settings(path, &AppSettings::default()).await?;
    }
    let settings = read_settings(path).await?;
    write_settings(path, &settings).await?;
    Ok(settings)
}

pub async fn read_settings(path: &std::path::Path) -> AppResult<AppSettings> {
    let content = fs::read_to_string(path).await?;
    let mut settings: AppSettings = serde_json::from_str(&content)?;
    validate_settings(&settings)?;
    settings.log_level = settings.log_level.trim().to_ascii_lowercase();
    settings.language = settings.language.trim().to_string();
    Ok(settings)
}

pub async fn write_settings(path: &std::path::Path, settings: &AppSettings) -> AppResult<()> {
    validate_settings(settings)?;
    let content = serde_json::to_string_pretty(settings)?;
    fsutil::write_atomic(path, content).await?;
    Ok(())
}

pub fn validate_settings(settings: &AppSettings) -> AppResult<()> {
    if settings.host.trim().is_empty() {
        return Err(AppError::BadRequest("host cannot be empty".to_string()));
    }
    if settings.port == 0 {
        return Err(AppError::BadRequest("port cannot be 0".to_string()));
    }
    let host = settings
        .host
        .trim()
        .parse::<IpAddr>()
        .map_err(|_| AppError::BadRequest("host must be a loopback IP address".to_string()))?;
    if !host.is_loopback() {
        return Err(AppError::BadRequest(
            "WeiAgent may only listen on a loopback address until authentication is configured"
                .to_string(),
        ));
    }
    if settings.avatar_icon.trim().is_empty() {
        return Err(AppError::BadRequest(
            "avatar_icon cannot be empty".to_string(),
        ));
    }
    let color = settings.avatar_color.trim();
    if color.len() != 7
        || !color.starts_with('#')
        || !color.chars().skip(1).all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(AppError::BadRequest(
            "avatar_color must be a hex color like #22c55e".to_string(),
        ));
    }
    let level = settings.log_level.trim().to_ascii_lowercase();
    match level.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {}
        _ => Err(AppError::BadRequest(
            "log_level must be trace, debug, info, warn, or error".to_string(),
        ))?,
    }
    match settings.language.trim() {
        "zh" | "en" => {}
        _ => Err(AppError::BadRequest(
            "language must be zh or en".to_string(),
        ))?,
    }
    if !(1..=600).contains(&settings.request_timeout_seconds) {
        return Err(AppError::BadRequest(
            "request_timeout_seconds must be between 1 and 600".to_string(),
        ));
    }
    if !(1..=10).contains(&settings.request_retry_attempts) {
        return Err(AppError::BadRequest(
            "request_retry_attempts must be between 1 and 10".to_string(),
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn settings_reject_non_loopback_hosts() {
        let settings = AppSettings {
            host: "0.0.0.0".to_string(),
            ..AppSettings::default()
        };
        assert!(validate_settings(&settings).is_err());
    }

    #[test]
    fn settings_allow_ipv4_and_ipv6_loopback_hosts() {
        for host in ["127.0.0.1", "::1"] {
            let settings = AppSettings {
                host: host.to_string(),
                ..AppSettings::default()
            };
            assert!(validate_settings(&settings).is_ok());
        }
    }

    #[test]
    fn settings_reject_unbounded_request_policy() {
        let mut settings = AppSettings {
            request_timeout_seconds: 601,
            ..AppSettings::default()
        };
        assert!(validate_settings(&settings).is_err());
        settings.request_timeout_seconds = 60;
        settings.request_retry_attempts = 11;
        assert!(validate_settings(&settings).is_err());
    }

    #[tokio::test]
    async fn reading_settings_does_not_rewrite_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let original = format!(
            "{}\n\n",
            serde_json::to_string_pretty(&AppSettings::default()).unwrap()
        );
        fs::write(&path, &original).await.unwrap();
        let settings = read_settings(&path).await.unwrap();
        assert_eq!(settings, AppSettings::default());
        assert_eq!(fs::read_to_string(path).await.unwrap(), original);
    }

    #[test]
    fn data_dir_override_wins() {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os("WEIAGENT_DATA_DIR");
        // SAFETY: test-only process-global env mutation.
        unsafe {
            std::env::set_var("WEIAGENT_DATA_DIR", r"D:\custom\weiagent-data");
        }
        let resolved = resolve_data_dir(false).unwrap();
        match previous {
            Some(value) => unsafe { std::env::set_var("WEIAGENT_DATA_DIR", value) },
            None => unsafe { std::env::remove_var("WEIAGENT_DATA_DIR") },
        }
        assert_eq!(resolved, PathBuf::from(r"D:\custom\weiagent-data"));
    }

    #[test]
    fn dev_data_dir_is_local_data_folder() {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os("WEIAGENT_DATA_DIR");
        unsafe {
            std::env::remove_var("WEIAGENT_DATA_DIR");
        }
        let resolved = resolve_data_dir(true).unwrap();
        match previous {
            Some(value) => unsafe { std::env::set_var("WEIAGENT_DATA_DIR", value) },
            None => unsafe { std::env::remove_var("WEIAGENT_DATA_DIR") },
        }
        assert_eq!(resolved, PathBuf::from("data"));
    }

    #[test]
    fn production_data_dir_lives_under_user_home() {
        let _guard = env_lock().lock().unwrap();
        let previous_data = std::env::var_os("WEIAGENT_DATA_DIR");
        let previous_portable = std::env::var_os("WEIAGENT_PORTABLE");
        unsafe {
            std::env::remove_var("WEIAGENT_DATA_DIR");
            std::env::remove_var("WEIAGENT_PORTABLE");
        }
        let resolved = resolve_data_dir(false).unwrap();
        match previous_data {
            Some(value) => unsafe { std::env::set_var("WEIAGENT_DATA_DIR", value) },
            None => unsafe { std::env::remove_var("WEIAGENT_DATA_DIR") },
        }
        match previous_portable {
            Some(value) => unsafe { std::env::set_var("WEIAGENT_PORTABLE", value) },
            None => unsafe { std::env::remove_var("WEIAGENT_PORTABLE") },
        }
        let home = user_home_dir().unwrap();
        assert_eq!(resolved, home.join("WeiAgent"));
        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("WeiAgent")
        );
    }
}
