use std::path::Path;

use serde::de::DeserializeOwned;
use tokio::fs;

use crate::{error::AppResult, fsutil};

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
