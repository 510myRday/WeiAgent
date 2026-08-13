use crate::error::{AppError, AppResult};

pub const DEFAULT_AVATAR_ICON: &str = "bot";
pub const DEFAULT_AVATAR_COLOR: &str = "#22c55e";

pub fn default_avatar_icon() -> String {
    DEFAULT_AVATAR_ICON.to_string()
}

pub fn default_avatar_color() -> String {
    DEFAULT_AVATAR_COLOR.to_string()
}

pub fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.chars().skip(1).all(|ch| ch.is_ascii_hexdigit())
}

pub fn normalize_avatar_icon(value: Option<&str>) -> AppResult<String> {
    let icon = value.unwrap_or(DEFAULT_AVATAR_ICON).trim();
    if icon.is_empty() {
        return Err(AppError::BadRequest(
            "avatar_icon cannot be empty".to_string(),
        ));
    }
    Ok(icon.to_string())
}

pub fn normalize_avatar_color(value: Option<&str>) -> AppResult<String> {
    let color = value.unwrap_or(DEFAULT_AVATAR_COLOR).trim();
    if !is_hex_color(color) {
        return Err(AppError::BadRequest(
            "avatar_color must be a hex color like #22c55e".to_string(),
        ));
    }
    Ok(color.to_ascii_lowercase())
}
