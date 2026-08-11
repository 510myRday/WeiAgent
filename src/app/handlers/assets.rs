use std::{io::Cursor, path::PathBuf};

use crate::{
    app::AppState,
    error::{AppError, AppResult},
    fsutil,
};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{StatusCode, header},
    response::Response,
};
use serde::Deserialize;

const MAX_INLINE_ASSET_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 50_000_000;
const MAX_DECODED_IMAGE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct LocalAssetQuery {
    path: String,
}

pub async fn read_local_asset(
    State(state): State<AppState>,
    Query(query): Query<LocalAssetQuery>,
) -> AppResult<Response> {
    let path = resolve_local_asset_path(&state.store.attachments_dir, &query.path).await?;
    let metadata = tokio::fs::metadata(&path).await?;
    if !metadata.is_file() {
        return Err(AppError::NotFound);
    }
    if metadata.len() > MAX_INLINE_ASSET_BYTES {
        return Err(AppError::BadRequest(format!(
            "local image exceeds the {} MB inline display limit",
            MAX_INLINE_ASSET_BYTES / 1024 / 1024
        )));
    }

    let bytes = tokio::fs::read(&path).await?;
    let content_type = validate_image_bytes(&path, &bytes)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CONTENT_DISPOSITION, "inline")
        .header("x-content-type-options", "nosniff")
        .header("referrer-policy", "no-referrer")
        .body(Body::from(bytes))
        .map_err(|error| AppError::Process(error.to_string()))
}

fn validate_image_bytes(path: &std::path::Path, bytes: &[u8]) -> AppResult<&'static str> {
    let expected_format = allowed_image_format(path)?;
    let detected_format = image::guess_format(bytes)
        .map_err(|_| AppError::BadRequest("local asset is not a valid image".to_string()))?;
    if detected_format != expected_format {
        return Err(AppError::BadRequest(
            "local asset content does not match its file extension".to_string(),
        ));
    }
    let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), expected_format)
        .into_dimensions()
        .map_err(|_| {
            AppError::BadRequest("local asset image dimensions are invalid".to_string())
        })?;
    if u64::from(width).saturating_mul(u64::from(height)) > MAX_IMAGE_PIXELS {
        return Err(AppError::BadRequest(
            "local asset dimensions are too large".to_string(),
        ));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), expected_format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| AppError::BadRequest("local asset image decoding failed".to_string()))?;
    Ok(image_content_type(expected_format))
}

async fn resolve_local_asset_path(root: &std::path::Path, raw_path: &str) -> AppResult<PathBuf> {
    let requested = path_from_local_source(raw_path)?;
    let root = tokio::fs::canonicalize(root).await?;
    let is_absolute = requested.is_absolute();
    let requested = if requested.is_absolute() {
        requested
    } else {
        std::env::current_dir()?.join(requested)
    };
    let resolved =
        tokio::fs::canonicalize(&requested)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => AppError::NotFound,
                _ => AppError::Storage(error),
            })?;
    if !is_absolute && !fsutil::path_is_within(&resolved, &root) {
        return Err(AppError::BadRequest(
            "local asset must be inside the WeiAgent attachments directory".to_string(),
        ));
    }
    Ok(resolved)
}

fn allowed_image_format(path: &std::path::Path) -> AppResult<image::ImageFormat> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Ok(image::ImageFormat::Png),
        Some("jpg" | "jpeg") => Ok(image::ImageFormat::Jpeg),
        Some("webp") => Ok(image::ImageFormat::WebP),
        Some("gif") => Ok(image::ImageFormat::Gif),
        _ => Err(AppError::BadRequest(
            "local assets must be PNG, JPEG, WebP, or GIF".to_string(),
        )),
    }
}

fn image_content_type(format: image::ImageFormat) -> &'static str {
    match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Gif => "image/gif",
        _ => "application/octet-stream",
    }
}

fn path_from_local_source(raw_path: &str) -> AppResult<PathBuf> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(
            "local image path cannot be empty".to_string(),
        ));
    }

    let path = if let Some(file_path) = trimmed.strip_prefix("file://") {
        file_uri_path(file_path)
    } else {
        trimmed.to_string()
    };
    let path = PathBuf::from(path);
    Ok(path)
}

fn file_uri_path(value: &str) -> String {
    #[cfg(windows)]
    {
        let normalized = value.replace('/', "\\");
        if normalized.starts_with('\\')
            && normalized
                .as_bytes()
                .get(1)
                .is_some_and(u8::is_ascii_alphabetic)
            && normalized.as_bytes().get(2) == Some(&b':')
        {
            return normalized[1..].to_string();
        }
        normalized
    }
    #[cfg(not(windows))]
    {
        if value.starts_with('/') {
            value.to_string()
        } else {
            format!("/{value}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[tokio::test]
    async fn resolves_an_absolute_local_image_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("attachments");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let image = root.join("chart.png");
        let valid_png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap();
        tokio::fs::write(&image, valid_png).await.unwrap();

        let resolved = resolve_local_asset_path(&root, image.to_string_lossy().as_ref())
            .await
            .unwrap();

        assert_eq!(resolved, tokio::fs::canonicalize(image).await.unwrap());
    }

    #[tokio::test]
    async fn rejects_images_outside_the_attachment_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("attachments");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let outside = directory.path().join("outside.png");
        tokio::fs::write(&outside, valid_png()).await.unwrap();

        assert!(matches!(
            resolve_local_asset_path(&root, outside.to_string_lossy().as_ref()).await,
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_svg_and_mismatched_or_invalid_image_content() {
        assert!(matches!(
            validate_image_bytes(std::path::Path::new("image.svg"), b"<svg></svg>"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            validate_image_bytes(std::path::Path::new("image.jpg"), &valid_png()),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            validate_image_bytes(std::path::Path::new("image.png"), b"\x89PNG"),
            Err(AppError::BadRequest(_))
        ));
    }

    fn valid_png() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap()
    }

    #[tokio::test]
    async fn resolves_a_relative_local_image_path_inside_the_attachment_directory() {
        let current_dir = std::env::current_dir().unwrap();
        let directory = tempfile::tempdir_in(&current_dir).unwrap();
        let root = directory.path().join("attachments");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let image = root.join("chart.png");
        tokio::fs::write(&image, valid_png()).await.unwrap();
        let relative = image.strip_prefix(&current_dir).unwrap();

        let resolved = resolve_local_asset_path(&root, relative.to_string_lossy().as_ref())
            .await
            .unwrap();

        assert_eq!(resolved, tokio::fs::canonicalize(image).await.unwrap());
    }
}
