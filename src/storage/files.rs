use std::path::Path;

use serde::{Serialize, de::DeserializeOwned};
use tokio::fs;

use crate::error::AppResult;

pub(super) async fn ensure_json_array(path: &Path) -> AppResult<()> {
    if fs::metadata(path).await.is_err() {
        fs::write(path, "[]").await?;
    }
    Ok(())
}

pub(super) async fn ensure_text_file(path: &Path, content: String) -> AppResult<()> {
    if fs::metadata(path).await.is_err() {
        fs::write(path, content).await?;
    }
    Ok(())
}

pub(super) async fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<T> {
    let content = fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&content)?)
}

pub(super) async fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> AppResult<()> {
    fs::write(path, serde_json::to_string_pretty(value)?).await?;
    Ok(())
}
