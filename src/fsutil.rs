use std::path::{Path, PathBuf};

use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::error::AppResult;

/// User home directory from USERPROFILE/HOME. Shared by config, workspace, and MCP expansion.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Formats a filesystem path for user-facing output.
///
/// Windows canonicalization may return an extended-length path such as
/// `\\\\?\\C:\\path` or `\\\\?\\UNC\\server\\share`. Keep that prefix on
/// paths used for filesystem operations, but omit it when displaying paths
/// in API responses, logs, and prompts.
pub fn display_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy().into_owned();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return rest.to_string();
        }

        return value;
    }

    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

/// Write `content` to `path` via a synced same-directory temp file and atomic replace.
pub async fn write_atomic(path: &Path, content: impl AsRef<[u8]>) -> AppResult<()> {
    let content = content.as_ref();
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
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
        let mut temp_file = fs::File::create(&temp_path).await?;
        temp_file.write_all(content).await?;
        temp_file.flush().await?;
        temp_file.sync_all().await?;
        drop(temp_file);
        replace_file(&temp_path, path).await
    }
    .await;

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path).await;
    }
    write_result?;
    Ok(())
}

/// Restrict a directory to the current OS user where the platform exposes
/// portable Unix permission bits. Windows user-data directories inherit the
/// profile ACL, so this is intentionally a no-op there.
pub async fn ensure_private_dir(path: &Path) -> AppResult<()> {
    fs::create_dir_all(path).await?;
    set_private_dir(path).await
}

pub async fn set_private_file(_path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

async fn set_private_dir(_path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

#[cfg(not(windows))]
async fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, path).await
}

#[cfg(windows)]
async fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temp_path = temp_path.to_path_buf();
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let source = temp_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// True when `path` is `root` or a descendant of `root`.
/// On Windows the comparison is case-insensitive and slash-normalized.
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = normalize_path_key(path);
        let root = normalize_path_key(root);
        path == root || path.starts_with(&(root.clone() + "\\")) || path.starts_with(&(root + "/"))
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
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\work\project"
        } else {
            "/work/project"
        });
        let child = root.join("src").join("main.rs");
        assert!(path_is_within(&child, &root));
        assert!(path_is_within(&root, &root));
        let outside = PathBuf::from(if cfg!(windows) { r"C:\other" } else { "/other" });
        assert!(!path_is_within(&outside, &root));
    }

    #[cfg(windows)]
    #[test]
    fn display_path_removes_windows_extended_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\work\project")),
            r"C:\work\project"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\project")),
            r"\\server\share\project"
        );
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
