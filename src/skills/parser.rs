use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::{
    domain::{SkillFile, SkillManifest},
    error::{AppError, AppResult},
    fsutil,
    skills::path_policy::MAX_SKILL_FILE_BYTES,
};

pub const COLLAPSED_FILE_KIND: &str = "collapsed";
pub const COLLAPSED_FILE_NAME: &str = "...";

const COLLAPSED_DIRECTORY_NAMES: &[&str] = &[
    ".conda",
    ".eggs",
    ".git",
    ".gradle",
    ".mypy_cache",
    ".nox",
    ".pytest_cache",
    ".ruff_cache",
    ".svn",
    ".tox",
    ".venv",
    "__pycache__",
    "bower_components",
    "conda-meta",
    "dist-packages",
    "env",
    "node_modules",
    "site-packages",
    "target",
    "venv",
];

pub fn is_collapsed_directory(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    COLLAPSED_DIRECTORY_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub async fn parse_skill(root: PathBuf) -> AppResult<SkillManifest> {
    let skill_md = root.join("SKILL.md");
    let metadata = fs::symlink_metadata(&skill_md).await?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::BadRequest(
            "SKILL.md is not a regular file".to_string(),
        ));
    }
    if metadata.len() > MAX_SKILL_FILE_BYTES {
        return Err(AppError::BadRequest(format!(
            "SKILL.md exceeds the {MAX_SKILL_FILE_BYTES} byte limit"
        )));
    }
    let content = fs::read_to_string(&skill_md).await?;
    let (frontmatter, _) = split_frontmatter(&content);
    let fields = parse_frontmatter(frontmatter);
    let name = fields
        .get("name")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let description = fields
        .get("description")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() || description.is_empty() {
        return Err(AppError::BadRequest(
            "SKILL.md frontmatter requires name and description".to_string(),
        ));
    }
    if !valid_name(&name) {
        return Err(AppError::BadRequest(
            "skill name must contain lowercase letters, numbers, '_' or '-'".to_string(),
        ));
    }
    let mut hash = Sha256::new();
    hash.update(content.as_bytes());
    let files = discover_all_files(&root).await?;
    let modified_at = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let updated_at = files
        .iter()
        .filter_map(|(_, item)| item.modified().ok())
        .max()
        .unwrap_or(modified_at);
    let created_at = fs::metadata(&root)
        .await
        .ok()
        .and_then(|item| item.created().ok())
        .or_else(|| metadata.created().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let modified_at: DateTime<Utc> = modified_at.into();
    let created_at: DateTime<Utc> = created_at.into();
    let updated_at: DateTime<Utc> = updated_at.into();
    Ok(SkillManifest {
        name,
        description,
        version: fields
            .get("version")
            .cloned()
            .filter(|v| !v.trim().is_empty()),
        group: fields
            .get("group")
            .cloned()
            .filter(|v| !v.trim().is_empty()),
        group_id: None,
        is_builtin: false,
        tags: fields
            .get("tags")
            .map(|v| parse_list(v))
            .unwrap_or_default(),
        icon_url: None,
        source_url: None,
        icon_proxy_url: None,
        update_proxy_url: None,
        source_skill: None,
        root_dir: fsutil::display_path(&root),
        skill_md: fsutil::display_path(&skill_md),
        scripts: discover_files(&root, "scripts").await?,
        references: discover_files(&root, "references").await?,
        examples: discover_files(&root, "examples").await?,
        content_hash: format!("{:x}", hash.finalize()),
        modified_at: modified_at.to_rfc3339(),
        created_at: created_at.to_rfc3339(),
        updated_at: updated_at.to_rfc3339(),
        diagnostics: Vec::new(),
        enabled: true,
        global_override: None,
        group_override: None,
        agent_override: None,
        agent_group_override: None,
    })
}

fn split_frontmatter(content: &str) -> (&str, &str) {
    let Some(rest) = content.strip_prefix("---") else {
        return ("", content);
    };
    let Some(end) = rest.find("\n---") else {
        return ("", content);
    };
    let after_delimiter = &rest[end + 4..];
    let body = after_delimiter
        .strip_prefix("\r\n")
        .or_else(|| after_delimiter.strip_prefix('\n'))
        .unwrap_or(after_delimiter);
    (&rest[..end], body)
}

fn parse_frontmatter(content: &str) -> std::collections::BTreeMap<String, String> {
    let mut result: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut list_key: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("-") {
            if let Some(key) = &list_key {
                let entry = result.entry(key.clone()).or_default();
                if !entry.is_empty() {
                    entry.push(',');
                }
                entry.push_str(item.trim().trim_matches(['\"', '\'']));
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches(['\"', '\'']).to_string();
        list_key = if value.is_empty() {
            Some(key.clone())
        } else {
            None
        };
        result.insert(key, value);
    }
    result
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .trim_matches(['[', ']'])
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn valid_name(name: &str) -> bool {
    name.len() <= 96
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

async fn discover_files(root: &Path, directory: &str) -> AppResult<Vec<SkillFile>> {
    let base = root.join(directory);
    let Ok(base_metadata) = fs::symlink_metadata(&base).await else {
        return Ok(Vec::new());
    };
    if base_metadata.file_type().is_symlink() || !base_metadata.is_dir() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(current) = stack.pop() {
        let mut entries = fs::read_dir(&current).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if is_collapsed_directory(&entry.file_name()) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            result.push(SkillFile {
                path: relative,
                kind: directory.to_string(),
                size_bytes: metadata.len(),
            });
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

pub async fn discover_all_files(root: &Path) -> AppResult<Vec<(PathBuf, std::fs::Metadata)>> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut entries = fs::read_dir(&current).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if is_collapsed_directory(&entry.file_name()) {
                    continue;
                }
                stack.push(path);
            } else if metadata.is_file() {
                result.push((path, metadata));
            }
        }
    }
    result.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(result)
}

pub async fn discover_display_files(root: &Path) -> AppResult<Vec<SkillFile>> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut entries = fs::read_dir(&current).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if metadata.is_dir() {
                if is_collapsed_directory(&entry.file_name()) {
                    result.push(SkillFile {
                        path: format!("{relative}/{COLLAPSED_FILE_NAME}"),
                        kind: COLLAPSED_FILE_KIND.to_string(),
                        size_bytes: 0,
                    });
                } else {
                    stack.push(path);
                }
                continue;
            }
            if metadata.is_file() {
                result.push(SkillFile {
                    path: relative,
                    kind: "other".to_string(),
                    size_bytes: metadata.len(),
                });
            }
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_required_and_optional_frontmatter() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("SKILL.md"),
            "---\nname: rust-review\ndescription: Review Rust code.\nversion: 1.0.0\ngroup: development\ntags:\n  - rust\n  - review\nunknown: retained-for-compatibility\n---\n# Review\n",
        )
        .await
        .unwrap();

        let manifest = parse_skill(directory.path().to_path_buf()).await.unwrap();
        assert_eq!(manifest.name, "rust-review");
        assert_eq!(manifest.description, "Review Rust code.");
        assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.group.as_deref(), Some("development"));
        assert_eq!(manifest.tags, ["rust", "review"]);
    }

    #[tokio::test]
    async fn rejects_missing_required_frontmatter() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("SKILL.md"),
            "---\nname: incomplete\n---\n",
        )
        .await
        .unwrap();

        let error = parse_skill(directory.path().to_path_buf())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires name and description"));
    }

    #[tokio::test]
    async fn parses_unicode_skill_with_frontmatter_delimiter_at_eof() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("SKILL.md"),
            "---\nname: unicode-skill\ndescription: 处理中文内容\n---",
        )
        .await
        .unwrap();

        let manifest = parse_skill(directory.path().to_path_buf()).await.unwrap();
        assert_eq!(manifest.name, "unicode-skill");
        assert_eq!(manifest.description, "处理中文内容");
    }

    #[tokio::test]
    async fn display_discovery_collapses_generated_directories() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "content")
            .await
            .unwrap();
        fs::create_dir_all(directory.path().join("node_modules").join("package"))
            .await
            .unwrap();
        fs::write(
            directory
                .path()
                .join("node_modules")
                .join("package")
                .join("index.js"),
            "module",
        )
        .await
        .unwrap();
        fs::create_dir_all(directory.path().join("docs"))
            .await
            .unwrap();
        fs::write(directory.path().join("docs").join("guide.md"), "guide")
            .await
            .unwrap();

        let files = discover_display_files(directory.path()).await.unwrap();
        assert!(
            files.iter().any(|file| {
                file.path == "node_modules/..." && file.kind == COLLAPSED_FILE_KIND
            })
        );
        assert!(
            !files
                .iter()
                .any(|file| file.path.starts_with("node_modules/package/"))
        );
        assert!(files.iter().any(|file| file.path == "docs/guide.md"));
    }
}
