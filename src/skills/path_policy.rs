use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub const MAX_SKILL_FILE_BYTES: u64 = 5 * 1024 * 1024;

pub fn relative_file(root: &Path, raw: &str) -> AppResult<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return relative_file(root, "SKILL.md");
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() || raw.contains('\0') {
        return Err(AppError::BadRequest(
            "skill file path must be relative".to_string(),
        ));
    }
    let joined = root.join(candidate);
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical = std::fs::canonicalize(&joined).map_err(|_| AppError::NotFound)?;
    if !crate::fsutil::path_is_within(&canonical, &canonical_root) {
        return Err(AppError::BadRequest(
            "skill file path escapes the skill directory".to_string(),
        ));
    }
    if !canonical.is_file() {
        return Err(AppError::NotFound);
    }
    let metadata = std::fs::metadata(&canonical)?;
    if metadata.len() > MAX_SKILL_FILE_BYTES {
        return Err(AppError::BadRequest(format!(
            "skill file exceeds the {MAX_SKILL_FILE_BYTES} byte limit"
        )));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside_skill_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "secret").unwrap();

        let error = relative_file(
            root.path(),
            &format!(
                "../{}/secret.md",
                outside.path().file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap_err();
        assert!(error.to_string().contains("escapes") || error.to_string().contains("not found"));
    }

    #[test]
    fn empty_path_resolves_skill_markdown() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("SKILL.md"), "content").unwrap();
        assert_eq!(
            relative_file(root.path(), "").unwrap(),
            std::fs::canonicalize(root.path().join("SKILL.md")).unwrap()
        );
    }
}
