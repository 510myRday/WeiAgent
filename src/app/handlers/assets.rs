use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use axum::{
    body::Body,
    extract::Query,
    http::{StatusCode, header},
    response::Response,
};
use serde::Deserialize;

const MAX_INLINE_ASSET_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct LocalAssetQuery {
    path: String,
}

pub async fn read_local_asset(Query(query): Query<LocalAssetQuery>) -> AppResult<Response> {
    let path = resolve_local_asset_path(&query.path).await?;
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

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    if mime.type_().as_str() != "image" {
        return Err(AppError::BadRequest(
            "the requested local asset is not an image".to_string(),
        ));
    }

    let bytes = tokio::fs::read(&path).await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CONTENT_DISPOSITION, "inline")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .map_err(|error| AppError::Process(error.to_string()))
}

async fn resolve_local_asset_path(raw_path: &str) -> AppResult<PathBuf> {
    let requested = path_from_local_source(raw_path)?;
    tokio::fs::canonicalize(&requested)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound,
            _ => AppError::Storage(error),
        })
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
    if !path.is_absolute() {
        return Err(AppError::BadRequest(
            "local image path must be absolute".to_string(),
        ));
    }
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

    #[tokio::test]
    async fn resolves_an_absolute_local_image_path() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("chart.png");
        tokio::fs::write(&image, b"png").await.unwrap();

        let resolved = resolve_local_asset_path(image.to_string_lossy().as_ref())
            .await
            .unwrap();

        assert_eq!(resolved, tokio::fs::canonicalize(image).await.unwrap());
    }

    #[test]
    fn rejects_relative_paths() {
        assert!(matches!(
            path_from_local_source("images/chart.png"),
            Err(AppError::BadRequest(_))
        ));
    }
}
