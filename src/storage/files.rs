use std::path::Path;

use serde::{Serialize, de::DeserializeOwned};
use tokio::fs;

use crate::{error::AppResult, fsutil};

pub(super) async fn ensure_json_array(path: &Path) -> AppResult<()> {
    if fs::metadata(path).await.is_err() {
        fsutil::write_atomic(path, b"[]").await?;
    }
    Ok(())
}

pub(super) async fn ensure_text_file(path: &Path, content: String) -> AppResult<()> {
    if fs::metadata(path).await.is_err() {
        fsutil::write_atomic(path, content).await?;
    }
    Ok(())
}

pub(super) async fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<T> {
    let content = fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&content)?)
}

pub(super) async fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> AppResult<()> {
    fsutil::write_atomic(path, serde_json::to_string_pretty(value)?).await?;
    Ok(())
}
