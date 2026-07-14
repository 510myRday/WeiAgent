use std::path::{Path, PathBuf};

use tokio::fs;
use uuid::Uuid;

use crate::error::AppResult;

/// Write `content` to `path` via a same-directory temp file + rename.
/// On Windows, replace is implemented as remove-then-rename because renames
/// cannot overwrite an existing file.
pub async fn write_atomic(path: &Path, content: impl AsRef<[u8]>) -> AppResult<()> {
    let content = content.as_ref();
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).await?;
    }

    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path '{}' is missing a file name", path.display()),
        )
    })?;
    let temp_name = format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        Uuid::new_v4().simple()
    );
    let temp_path = match parent {
        Some(parent) => parent.join(&temp_name),
        None => PathBuf::from(&temp_name),
    };

    let write_result = async {
        fs::write(&temp_path, content).await?;
        match fs::rename(&temp_path, path).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // Windows cannot rename over an existing destination.
                if fs::metadata(path).await.is_ok() {
                    fs::remove_file(path).await?;
                    fs::rename(&temp_path, path).await?;
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
    .await;

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path).await;
    }
    write_result?;
    Ok(())
}

/// True when `path` is `root` or a descendant of `root`.
/// On Windows the comparison is case-insensitive and slash-normalized.
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = normalize_path_key(path);
        let root = normalize_path_key(root);
        path == root
            || path.starts_with(&(root.clone() + "\\"))
            || path.starts_with(&(root + "/"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

/// Equality for filesystem identity checks (case-insensitive on Windows).
pub fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        normalize_path_key(left) == normalize_path_key(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Stable key for deduplicating configured roots.
pub fn path_identity_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        normalize_path_key(path)
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(windows)]
fn normalize_path_key(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('/', "\\");
    while value.ends_with('\\') && value.len() > 3 {
        value.pop();
    }
    value.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_atomic_creates_and_replaces_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_atomic(&path, b"{\"a\":1}").await.unwrap();
        assert_eq!(fs::read_to_string(&path).await.unwrap(), "{\"a\":1}");
        write_atomic(&path, b"{\"a\":2}").await.unwrap();
        assert_eq!(fs::read_to_string(&path).await.unwrap(), "{\"a\":2}");
    }

    #[test]
    fn path_within_accepts_descendants() {
        let root = PathBuf::from(if cfg!(windows) { r"C:\work\project" } else { "/work/project" });
        let child = root.join("src").join("main.rs");
        assert!(path_is_within(&child, &root));
        assert!(path_is_within(&root, &root));
        let outside = PathBuf::from(if cfg!(windows) { r"C:\other" } else { "/other" });
        assert!(!path_is_within(&outside, &root));
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_compare_is_case_insensitive() {
        let root = Path::new(r"C:\Work\Project");
        let child = Path::new(r"c:\work\project\src");
        assert!(path_is_within(child, root));
        assert!(paths_equal(Path::new(r"C:\Work"), Path::new(r"c:\work")));
    }
}
