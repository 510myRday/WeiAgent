use std::{
    ffi::OsStr,
    io::{Read, Seek},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncWriteExt, process::Command};
use uuid::Uuid;

use crate::{
    domain::{CodexRuntimeSource, RuntimeComponentKind, RuntimeComponentStatus},
    error::{AppError, AppResult},
};

const MAX_DOWNLOAD_BYTES: u64 = 600 * 1024 * 1024;

#[derive(Debug)]
struct ProbeCandidate {
    program: PathBuf,
    args: &'static [&'static str],
    source: CodexRuntimeSource,
}

#[derive(Debug, Deserialize)]
struct NodeRelease {
    version: String,
    lts: serde_json::Value,
    files: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PythonReleaseCycle {
    latest: String,
}

#[derive(Debug, Deserialize)]
struct GithubTag {
    name: String,
}

pub async fn latest_component_version(
    kind: RuntimeComponentKind,
    client: &reqwest::Client,
) -> AppResult<String> {
    match kind {
        RuntimeComponentKind::Node => {
            let index_file = node_index_file().ok_or_else(unsupported_platform)?;
            let releases = fetch_node_releases(client).await?;
            releases
                .into_iter()
                .find(|release| {
                    !release.lts.is_null()
                        && release.lts != false
                        && (node_uses_unofficial_builds()
                            || release.files.iter().any(|file| file == index_file))
                })
                .map(|release| release.version.trim_start_matches('v').to_string())
                .ok_or_else(|| {
                    AppError::Process("no compatible Node.js LTS release was found".to_string())
                })
        }
        RuntimeComponentKind::Uv => Ok(fetch_uv_release(client)
            .await?
            .tag_name
            .trim_start_matches('v')
            .to_string()),
        RuntimeComponentKind::Python => {
            let releases = client
                .get("https://endoflife.date/api/python.json")
                .send()
                .await
                .map_err(download_error)?
                .error_for_status()
                .map_err(download_error)?
                .json::<Vec<PythonReleaseCycle>>()
                .await
                .map_err(download_error)?;
            releases
                .into_iter()
                .next()
                .map(|release| release.latest)
                .ok_or_else(|| AppError::Process("no stable Python release was found".to_string()))
        }
        RuntimeComponentKind::Git => fetch_git_tags(client)
            .await?
            .into_iter()
            .filter_map(|tag| stable_git_version(&tag.name))
            .max()
            .map(|version| version.to_string())
            .ok_or_else(|| AppError::Process("no stable Git release was found".to_string())),
        RuntimeComponentKind::Codex => Err(AppError::BadRequest(
            "Codex version is managed by the terminal runtime manager".to_string(),
        )),
    }
}

pub async fn detect_components(
    runtimes_root: &Path,
    installing: Option<RuntimeComponentKind>,
) -> Vec<RuntimeComponentStatus> {
    let node = detect_component(
        RuntimeComponentKind::Node,
        node_candidates(runtimes_root).await,
        installing == Some(RuntimeComponentKind::Node),
    )
    .await;
    let python = detect_component(
        RuntimeComponentKind::Python,
        python_candidates(runtimes_root).await,
        installing == Some(RuntimeComponentKind::Python),
    )
    .await;
    let uv = detect_component(
        RuntimeComponentKind::Uv,
        uv_candidates(runtimes_root).await,
        installing == Some(RuntimeComponentKind::Uv),
    )
    .await;
    let git = detect_component(
        RuntimeComponentKind::Git,
        git_candidates(runtimes_root).await,
        installing == Some(RuntimeComponentKind::Git),
    )
    .await;
    vec![node, python, uv, git]
}

pub async fn managed_bin_paths(runtimes_root: &Path) -> Vec<PathBuf> {
    let managed_programs = [
        (runtimes_root.join("node").join("current"), node_program()),
        (runtimes_root.join("python"), python_program()),
        (runtimes_root.join("uv").join("current"), uv_program()),
        (runtimes_root.join("git").join("current"), git_program()),
    ];
    let mut paths = Vec::new();
    for (root, program) in managed_programs {
        for executable in find_named_files(&root, program).await {
            let Some(parent) = executable.parent() else {
                continue;
            };
            let parent = parent.to_path_buf();
            if !paths.iter().any(|current| current == &parent) {
                paths.push(parent);
            }
        }
    }
    paths
}

pub async fn install_component(
    kind: RuntimeComponentKind,
    runtimes_root: &Path,
    client: &reqwest::Client,
) -> AppResult<()> {
    fs::create_dir_all(runtimes_root).await?;
    match kind {
        RuntimeComponentKind::Node => install_node(runtimes_root, client).await,
        RuntimeComponentKind::Uv => install_uv(runtimes_root, client).await,
        RuntimeComponentKind::Git => install_git(runtimes_root, client).await,
        RuntimeComponentKind::Python => {
            if component_missing(
                &detect_components(runtimes_root, Some(RuntimeComponentKind::Python)).await,
                RuntimeComponentKind::Uv,
            ) {
                install_uv(runtimes_root, client).await?;
            }
            install_python(runtimes_root, client).await
        }
        RuntimeComponentKind::Codex => Err(AppError::BadRequest(
            "Codex is managed by the terminal runtime manager".to_string(),
        )),
    }
}

fn component_missing(statuses: &[RuntimeComponentStatus], kind: RuntimeComponentKind) -> bool {
    statuses
        .iter()
        .find(|status| status.kind == kind)
        .is_none_or(|status| !status.available)
}

async fn detect_component(
    kind: RuntimeComponentKind,
    candidates: Vec<ProbeCandidate>,
    installing: bool,
) -> RuntimeComponentStatus {
    let managed_installed = candidates
        .iter()
        .any(|candidate| candidate.source == CodexRuntimeSource::Managed);
    let mut last_error = None;
    for candidate in candidates {
        match probe_version(&candidate.program, candidate.args).await {
            Ok(version) => {
                return RuntimeComponentStatus {
                    kind,
                    available: true,
                    required: false,
                    managed_installed,
                    installing,
                    source: Some(candidate.source),
                    detected_version: Some(version),
                    target_version: None,
                    update_available: false,
                    error: None,
                };
            }
            Err(error) => last_error = Some(error),
        }
    }
    RuntimeComponentStatus {
        kind,
        available: false,
        required: false,
        managed_installed,
        installing,
        source: None,
        detected_version: None,
        target_version: None,
        update_available: false,
        error: last_error,
    }
}

async fn probe_version(program: &Path, args: &[&str]) -> Result<String, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("'{}' timed out", program.display()))?
    .map_err(|error| format!("failed to start '{}': {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!(
            "'{}' failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let version = if stdout.is_empty() { stderr } else { stdout };
    if version.is_empty() {
        return Err(format!("'{}' returned no version", program.display()));
    }
    Ok(version)
}

async fn node_candidates(root: &Path) -> Vec<ProbeCandidate> {
    let mut candidates = managed_programs(&root.join("node").join("current"), node_program()).await;
    candidates.push(system_candidate(node_program(), &["--version"]));
    candidates
}

async fn uv_candidates(root: &Path) -> Vec<ProbeCandidate> {
    let mut candidates = managed_programs(&root.join("uv").join("current"), uv_program()).await;
    candidates.push(system_candidate(uv_program(), &["--version"]));
    candidates
}

async fn git_candidates(root: &Path) -> Vec<ProbeCandidate> {
    let mut candidates = managed_programs(&root.join("git").join("current"), git_program()).await;
    candidates.push(system_candidate(git_program(), &["--version"]));
    candidates
}

async fn python_candidates(root: &Path) -> Vec<ProbeCandidate> {
    let mut candidates = managed_programs(&root.join("python"), python_program()).await;
    if cfg!(windows) {
        candidates.push(system_candidate("python.exe", &["--version"]));
        candidates.push(system_candidate("py.exe", &["-3", "--version"]));
    } else {
        candidates.push(system_candidate("python3", &["--version"]));
        candidates.push(system_candidate("python", &["--version"]));
    }
    candidates
}

fn system_candidate(program: &'static str, args: &'static [&'static str]) -> ProbeCandidate {
    ProbeCandidate {
        program: PathBuf::from(program),
        args,
        source: CodexRuntimeSource::System,
    }
}

async fn managed_programs(root: &Path, expected: &str) -> Vec<ProbeCandidate> {
    find_named_files(root, expected)
        .await
        .into_iter()
        .map(|program| ProbeCandidate {
            program,
            args: &["--version"],
            source: CodexRuntimeSource::Managed,
        })
        .collect()
}

async fn find_named_files(root: &Path, expected: &str) -> Vec<PathBuf> {
    let root = root.to_path_buf();
    let expected = expected.to_string();
    tokio::task::spawn_blocking(move || {
        let mut matches = Vec::new();
        let mut stack = vec![root];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case(OsStr::new(&expected)))
                {
                    matches.push(path);
                }
            }
        }
        matches.sort_by(|left, right| {
            version_hint_from_path(right)
                .cmp(&version_hint_from_path(left))
                .then_with(|| left.cmp(right))
        });
        matches
    })
    .await
    .unwrap_or_default()
}

fn version_hint_from_path(path: &Path) -> Option<Version> {
    for component in path.components().rev() {
        let text = component.as_os_str().to_string_lossy();
        for token in text.split(|character: char| !character.is_ascii_digit() && character != '.') {
            let parts = token
                .trim_matches('.')
                .split('.')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            if parts.len() < 2 || parts.iter().any(|part| part.parse::<u64>().is_err()) {
                continue;
            }
            let normalized = format!(
                "{}.{}.{}",
                parts[0],
                parts[1],
                parts.get(2).copied().unwrap_or("0")
            );
            if let Ok(version) = Version::parse(&normalized) {
                return Some(version);
            }
        }
    }
    None
}

async fn install_node(root: &Path, client: &reqwest::Client) -> AppResult<()> {
    let asset_suffix = node_asset_suffix().ok_or_else(unsupported_platform)?;
    let index_file = node_index_file().ok_or_else(unsupported_platform)?;
    let releases = fetch_node_releases(client).await?;
    let release = releases
        .into_iter()
        .find(|release| {
            !release.lts.is_null()
                && release.lts != false
                && (node_uses_unofficial_builds()
                    || release.files.iter().any(|file| file == index_file))
        })
        .ok_or_else(|| {
            AppError::Process("no compatible Node.js LTS release was found".to_string())
        })?;
    let version = release.version.trim_start_matches('v');
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset_name = format!("node-v{version}-{asset_suffix}.{extension}");
    let base_url = if node_uses_unofficial_builds() {
        format!("https://unofficial-builds.nodejs.org/download/release/v{version}")
    } else {
        format!("https://nodejs.org/dist/v{version}")
    };
    let checksums = client
        .get(format!("{base_url}/SHASUMS256.txt"))
        .send()
        .await
        .map_err(download_error)?
        .error_for_status()
        .map_err(download_error)?
        .text()
        .await
        .map_err(download_error)?;
    let checksum = checksum_for(&checksums, &asset_name)?;
    install_archive(
        root,
        "node",
        client,
        &format!("{base_url}/{asset_name}"),
        &asset_name,
        &checksum,
        node_program(),
    )
    .await
}

async fn install_uv(root: &Path, client: &reqwest::Client) -> AppResult<()> {
    let target = uv_target().ok_or_else(unsupported_platform)?;
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset_name = format!("uv-{target}.{extension}");
    let release = fetch_uv_release(client).await?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            AppError::Process(format!(
                "uv {} did not provide {asset_name}",
                release.tag_name
            ))
        })?;
    let checksum = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| {
            AppError::Process("uv release did not provide a SHA-256 digest".to_string())
        })?;
    install_archive(
        root,
        "uv",
        client,
        &asset.browser_download_url,
        &asset_name,
        checksum,
        uv_program(),
    )
    .await
}

async fn install_git(root: &Path, client: &reqwest::Client) -> AppResult<()> {
    if cfg!(target_os = "linux") {
        return install_git_with_linux_package_manager().await;
    }
    if cfg!(target_os = "macos") {
        let brew = macos_brew_program();
        return install_git_with_package_manager(&brew, &["install", "git"], false).await;
    }

    install_git_for_windows(root, client).await
}

async fn install_git_for_windows(root: &Path, client: &reqwest::Client) -> AppResult<()> {
    let asset_suffix = git_asset_suffix().ok_or_else(unsupported_platform)?;
    let release = fetch_git_for_windows_release(client).await?;
    let package_version = git_for_windows_package_version(&release.tag_name).ok_or_else(|| {
        AppError::Process(format!(
            "Git for Windows returned an unrecognized release '{}'",
            release.tag_name
        ))
    })?;
    let asset_name = format!("MinGit-{package_version}-{asset_suffix}.zip");
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            AppError::Process(format!(
                "Git for Windows {} did not provide {asset_name}",
                release.tag_name
            ))
        })?;
    let checksum = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| {
            AppError::Process(
                "Git for Windows release did not provide a SHA-256 digest".to_string(),
            )
        })?;
    install_archive(
        root,
        "git",
        client,
        &asset.browser_download_url,
        &asset_name,
        checksum,
        git_program(),
    )
    .await
}

async fn install_git_with_linux_package_manager() -> AppResult<()> {
    let Some((program, args)) = linux_git_package_manager(|program| {
        std::process::Command::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }) else {
        return Err(AppError::Process(
            "no supported Linux package manager was found (apt, dnf, yum, zypper, pacman, or apk)"
                .to_string(),
        ));
    };
    install_git_with_package_manager(program, args, true).await
}

fn macos_brew_program() -> String {
    let candidates = [
        std::env::var_os("HOMEBREW_PREFIX")
            .map(PathBuf::from)
            .map(|prefix| prefix.join("bin").join("brew")),
        Some(PathBuf::from("/opt/homebrew/bin/brew")),
        Some(PathBuf::from("/usr/local/bin/brew")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|program| program.is_file())
        .map(|program| program.to_string_lossy().to_string())
        .unwrap_or_else(|| "brew".to_string())
}

async fn install_git_with_package_manager(
    program: &str,
    args: &[&str],
    elevate: bool,
) -> AppResult<()> {
    let is_root = if elevate {
        current_user_is_root().await
    } else {
        true
    };
    let mut command = if elevate && !is_root {
        let use_pkexec = executable_is_available("pkexec").await;
        let mut command = Command::new(if use_pkexec { "pkexec" } else { "sudo" });
        if use_pkexec {
            command.arg(program);
        } else {
            command.args(["-n", program]);
        }
        command
    } else {
        Command::new(program)
    };
    let output = tokio::time::timeout(
        Duration::from_secs(15 * 60),
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| AppError::Process(format!("{program} Git installation timed out")))?
    .map_err(|error| {
        AppError::Process(format!(
            "failed to start {program} for Git installation: {error}"
        ))
    })?;
    if !output.status.success() {
        return Err(AppError::Process(format!(
            "{program} Git installation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    probe_version(Path::new(git_program()), &["--version"])
        .await
        .map(|_| ())
        .map_err(|error| {
            AppError::Process(format!(
                "{program} completed but Git was not available: {error}"
            ))
        })
}

async fn current_user_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .stdin(Stdio::null())
        .output()
        .await
        .is_ok_and(|output| output.status.success() && output.stdout == b"0\n")
}

async fn executable_is_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

fn linux_git_package_manager(
    mut available: impl FnMut(&str) -> bool,
) -> Option<(&'static str, &'static [&'static str])> {
    const CANDIDATES: [(&str, &[&str]); 6] = [
        ("apt-get", &["install", "-y", "git"]),
        ("dnf", &["install", "-y", "git"]),
        ("yum", &["install", "-y", "git"]),
        ("zypper", &["--non-interactive", "install", "git"]),
        ("pacman", &["--noconfirm", "-S", "git"]),
        ("apk", &["add", "git"]),
    ];
    CANDIDATES
        .into_iter()
        .find(|(program, _)| available(program))
}

async fn install_python(root: &Path, client: &reqwest::Client) -> AppResult<()> {
    let uv = uv_candidates(root)
        .await
        .into_iter()
        .next()
        .map(|candidate| candidate.program)
        .or_else(|| Some(PathBuf::from(uv_program())))
        .ok_or_else(|| AppError::Process("uv is required to install Python".to_string()))?;
    let install_dir = root.join("python");
    fs::create_dir_all(&install_dir).await?;
    let target_version = latest_component_version(RuntimeComponentKind::Python, client).await?;
    let output = Command::new(&uv)
        .args(["python", "install", &target_version])
        .env("UV_PYTHON_INSTALL_DIR", &install_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| AppError::Process(format!("failed to start uv: {error}")))?;
    if !output.status.success() {
        return Err(AppError::Process(format!(
            "Python installation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if find_named_files(&install_dir, python_program())
        .await
        .is_empty()
    {
        return Err(AppError::Process(
            "Python installation completed without a usable executable".to_string(),
        ));
    }
    Ok(())
}

async fn fetch_node_releases(client: &reqwest::Client) -> AppResult<Vec<NodeRelease>> {
    client
        .get("https://nodejs.org/dist/index.json")
        .send()
        .await
        .map_err(download_error)?
        .error_for_status()
        .map_err(download_error)?
        .json::<Vec<NodeRelease>>()
        .await
        .map_err(download_error)
}

async fn fetch_uv_release(client: &reqwest::Client) -> AppResult<GithubRelease> {
    client
        .get("https://api.github.com/repos/astral-sh/uv/releases/latest")
        .send()
        .await
        .map_err(download_error)?
        .error_for_status()
        .map_err(download_error)?
        .json::<GithubRelease>()
        .await
        .map_err(download_error)
}

async fn fetch_git_tags(client: &reqwest::Client) -> AppResult<Vec<GithubTag>> {
    client
        .get("https://api.github.com/repos/git/git/tags?per_page=20")
        .send()
        .await
        .map_err(download_error)?
        .error_for_status()
        .map_err(download_error)?
        .json::<Vec<GithubTag>>()
        .await
        .map_err(download_error)
}

async fn fetch_git_for_windows_release(client: &reqwest::Client) -> AppResult<GithubRelease> {
    client
        .get("https://api.github.com/repos/git-for-windows/git/releases/latest")
        .send()
        .await
        .map_err(download_error)?
        .error_for_status()
        .map_err(download_error)?
        .json::<GithubRelease>()
        .await
        .map_err(download_error)
}

async fn install_archive(
    root: &Path,
    component: &str,
    client: &reqwest::Client,
    url: &str,
    asset_name: &str,
    checksum: &str,
    expected_program: &str,
) -> AppResult<()> {
    let component_root = root.join(component);
    let install_id = Uuid::new_v4().simple().to_string();
    let archive_path = component_root.join(format!(".download-{install_id}-{asset_name}"));
    let staging_dir = component_root.join(format!(".staging-{install_id}"));
    fs::create_dir_all(&staging_dir).await?;
    let result = async {
        download_sha256(client, url, checksum, &archive_path).await?;
        let archive = archive_path.clone();
        let staging = staging_dir.clone();
        tokio::task::spawn_blocking(move || extract_archive(&archive, &staging))
            .await
            .map_err(|error| {
                AppError::Process(format!("archive extraction task failed: {error}"))
            })??;
        if find_named_files(&staging_dir, expected_program)
            .await
            .is_empty()
        {
            return Err(AppError::Process(format!(
                "downloaded {component} archive did not contain {expected_program}"
            )));
        }
        let final_dir = component_root.join("current");
        if fs::metadata(&final_dir).await.is_ok() {
            fs::remove_dir_all(&final_dir).await?;
        }
        fs::rename(&staging_dir, &final_dir).await?;
        Ok(())
    }
    .await;
    let _ = fs::remove_file(&archive_path).await;
    if fs::metadata(&staging_dir).await.is_ok() {
        let _ = fs::remove_dir_all(&staging_dir).await;
    }
    result
}

async fn download_sha256(
    client: &reqwest::Client,
    url: &str,
    expected: &str,
    destination: &Path,
) -> AppResult<()> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(download_error)?
        .error_for_status()
        .map_err(download_error)?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES)
    {
        return Err(AppError::Process(
            "runtime download is too large".to_string(),
        ));
    }
    let mut file = fs::File::create(destination).await?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(download_error)?;
        received = received.saturating_add(chunk.len() as u64);
        if received > MAX_DOWNLOAD_BYTES {
            return Err(AppError::Process(
                "runtime download is too large".to_string(),
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(AppError::Process(
            "runtime package integrity verification failed".to_string(),
        ));
    }
    Ok(())
}

fn checksum_for(checksums: &str, asset_name: &str) -> AppResult<String> {
    checksums
        .lines()
        .find_map(|line| {
            let (checksum, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start_matches('*').trim() == asset_name).then(|| checksum.to_string())
        })
        .ok_or_else(|| AppError::Process(format!("missing checksum for {asset_name}")))
}

fn extract_archive(archive_path: &Path, destination: &Path) -> AppResult<()> {
    if archive_path.extension() == Some(OsStr::new("zip")) {
        let file = std::fs::File::open(archive_path)?;
        extract_zip(file, destination)
    } else {
        let file = std::fs::File::open(archive_path)?;
        extract_tar_gz(file, destination)
    }
}

fn extract_zip<R: Read + Seek>(reader: R, destination: &Path) -> AppResult<()> {
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| AppError::Process(format!("invalid zip archive: {error}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AppError::Process(format!("invalid zip entry: {error}")))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| AppError::Process("runtime zip contained an unsafe path".to_string()))?;
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(output)?;
            std::io::copy(&mut entry, &mut file)?;
        }
    }
    Ok(())
}

fn extract_tar_gz<R: Read>(reader: R, destination: &Path) -> AppResult<()> {
    let decoder = GzDecoder::new(reader);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(AppError::Process(
                "runtime archive contained an unsafe path".to_string(),
            ));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            continue;
        }
        let output = destination.join(path);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&output)?;
        } else if entry_type.is_file() {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(output)?;
        }
    }
    Ok(())
}

fn node_program() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
}

fn uv_program() -> &'static str {
    if cfg!(windows) { "uv.exe" } else { "uv" }
}

fn git_program() -> &'static str {
    if cfg!(windows) { "git.exe" } else { "git" }
}

fn python_program() -> &'static str {
    if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    }
}

fn node_asset_suffix() -> Option<&'static str> {
    node_asset_suffix_for(
        std::env::consts::OS,
        std::env::consts::ARCH,
        target_environment(),
    )
}

fn node_asset_suffix_for(os: &str, arch: &str, environment: &str) -> Option<&'static str> {
    match (os, arch, environment) {
        ("linux", "x86_64", "musl") => Some("linux-x64-musl"),
        ("linux", "aarch64", "musl") => Some("linux-arm64-musl"),
        (os, arch, _) => node_glibc_asset_suffix_for(os, arch),
    }
}

fn node_glibc_asset_suffix_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("win-x64"),
        ("windows", "aarch64") => Some("win-arm64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

fn node_index_file() -> Option<&'static str> {
    node_index_file_for(
        std::env::consts::OS,
        std::env::consts::ARCH,
        target_environment(),
    )
}

fn node_index_file_for(os: &str, arch: &str, environment: &str) -> Option<&'static str> {
    match (os, arch, environment) {
        ("linux", "x86_64", "musl") => Some("linux-x64-musl"),
        ("linux", "aarch64", "musl") => Some("linux-arm64-musl"),
        ("windows", "x86_64", _) => Some("win-x64-zip"),
        ("windows", "aarch64", _) => Some("win-arm64-zip"),
        ("linux", "x86_64", _) => Some("linux-x64"),
        ("linux", "aarch64", _) => Some("linux-arm64"),
        ("macos", "x86_64", _) => Some("osx-x64-tar"),
        ("macos", "aarch64", _) => Some("osx-arm64-tar"),
        _ => None,
    }
}

fn node_uses_unofficial_builds() -> bool {
    cfg!(all(target_os = "linux", target_env = "musl"))
}

fn target_environment() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl"
    } else {
        "gnu"
    }
}

fn uv_target() -> Option<&'static str> {
    uv_target_for(
        std::env::consts::OS,
        std::env::consts::ARCH,
        target_environment(),
    )
}

fn uv_target_for(os: &str, arch: &str, environment: &str) -> Option<&'static str> {
    match (os, arch, environment) {
        ("linux", "x86_64", "musl") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64", "musl") => Some("aarch64-unknown-linux-musl"),
        ("windows", "x86_64", _) => Some("x86_64-pc-windows-msvc"),
        ("windows", "aarch64", _) => Some("aarch64-pc-windows-msvc"),
        ("linux", "x86_64", _) => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64", _) => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64", _) => Some("x86_64-apple-darwin"),
        ("macos", "aarch64", _) => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

fn git_asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("64-bit"),
        ("windows", "aarch64") => Some("arm64"),
        _ => None,
    }
}

fn stable_git_version(tag: &str) -> Option<Version> {
    let version = tag.trim_start_matches('v');
    Version::parse(version)
        .ok()
        .filter(|version| version.pre.is_empty() && version.build.is_empty())
}

fn git_for_windows_package_version(tag: &str) -> Option<String> {
    let (version, build) = tag.trim_start_matches('v').split_once(".windows.")?;
    Version::parse(version).ok()?;
    build
        .chars()
        .all(|character| character.is_ascii_digit())
        .then(|| format!("{version}.{build}"))
}

fn unsupported_platform() -> AppError {
    AppError::BadRequest("managed runtime is not available for this platform".to_string())
}

fn download_error(error: reqwest::Error) -> AppError {
    AppError::Process(format!("runtime download failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_runtime_targets_cover_supported_platforms() {
        assert!(node_asset_suffix().is_some());
        assert!(node_index_file().is_some());
        assert!(uv_target().is_some());
        if cfg!(windows) {
            assert!(git_asset_suffix().is_some());
        }
    }

    #[test]
    fn linux_runtime_targets_distinguish_glibc_and_musl() {
        assert_eq!(
            node_asset_suffix_for("linux", "x86_64", "gnu"),
            Some("linux-x64")
        );
        assert_eq!(
            node_asset_suffix_for("linux", "aarch64", "musl"),
            Some("linux-arm64-musl")
        );
        assert_eq!(
            node_index_file_for("windows", "x86_64", "gnu"),
            Some("win-x64-zip")
        );
        assert_eq!(
            node_index_file_for("macos", "aarch64", "gnu"),
            Some("osx-arm64-tar")
        );
        assert_eq!(
            uv_target_for("linux", "x86_64", "musl"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            uv_target_for("linux", "aarch64", "gnu"),
            Some("aarch64-unknown-linux-gnu")
        );
    }

    #[test]
    fn git_release_versions_are_normalized_for_comparison_and_assets() {
        assert_eq!(stable_git_version("v2.55.0"), Some(Version::new(2, 55, 0)));
        assert_eq!(stable_git_version("v2.55.0-rc1"), None);
        assert_eq!(
            git_for_windows_package_version("v2.55.0.windows.3"),
            Some("2.55.0.3".to_string())
        );
    }

    #[test]
    fn linux_git_install_uses_the_available_distribution_package_manager() {
        let selected = linux_git_package_manager(|program| program == "apk").unwrap();
        assert_eq!(selected, ("apk", &["add", "git"][..]));

        let selected = linux_git_package_manager(|program| program == "pacman").unwrap();
        assert_eq!(selected, ("pacman", &["--noconfirm", "-S", "git"][..]));
    }

    #[test]
    fn node_checksum_is_selected_by_exact_filename() {
        let checksums = "abc  node-v1-win-x64.zip\ndef  node-v1-win-arm64.zip\n";
        assert_eq!(
            checksum_for(checksums, "node-v1-win-x64.zip").unwrap(),
            "abc"
        );
    }

    #[tokio::test]
    async fn installing_state_only_marks_the_active_component() {
        let temp = tempfile::tempdir().unwrap();
        let statuses = detect_components(temp.path(), Some(RuntimeComponentKind::Python)).await;

        assert!(
            statuses
                .iter()
                .find(|status| status.kind == RuntimeComponentKind::Python)
                .unwrap()
                .installing
        );
        assert!(
            statuses
                .iter()
                .filter(|status| status.kind != RuntimeComponentKind::Python)
                .all(|status| !status.installing)
        );
        assert_eq!(
            statuses.last().map(|status| status.kind),
            Some(RuntimeComponentKind::Git)
        );
    }

    #[tokio::test]
    async fn managed_python_paths_prefer_the_newest_version() {
        let temp = tempfile::tempdir().unwrap();
        let older = temp
            .path()
            .join("python")
            .join("cpython-3.13.7-windows-x86_64-none");
        let newer = temp
            .path()
            .join("python")
            .join("cpython-3.14.0-windows-x86_64-none");
        fs::create_dir_all(&older).await.unwrap();
        fs::create_dir_all(&newer).await.unwrap();
        fs::write(older.join(python_program()), []).await.unwrap();
        fs::write(newer.join(python_program()), []).await.unwrap();

        let paths = managed_bin_paths(temp.path()).await;

        assert_eq!(paths.first(), Some(&newer));
        assert_eq!(paths.get(1), Some(&older));
    }
}
