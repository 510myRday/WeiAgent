use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, RwLock},
};
use uuid::Uuid;

use crate::{
    domain::{CodexRuntimeSource, CodexRuntimeStatus},
    error::{AppError, AppResult},
};

pub const FALLBACK_CODEX_VERSION: &str = "0.137.0";
pub const MIN_SUPPORTED_CODEX_VERSION: &str = "0.137.0";
const MAX_RUNTIME_DOWNLOAD_BYTES: u64 = 600 * 1024 * 1024;
const RUNTIME_MANIFEST_FILE: &str = "weiagent-runtime.json";
const LATEST_VERSION_CACHE_FILE: &str = "latest-version.json";
const LATEST_VERSION_CACHE_HOURS: i64 = 24;
const LATEST_VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const APP_SERVER_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct CodexExecutable {
    pub program: PathBuf,
    pub codex_home: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedRuntimeManifest {
    package_version: String,
    integrity: String,
    executable: String,
    installed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatestVersionCache {
    version: String,
    checked_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
struct NpmPackageMetadata {
    version: String,
    dist: NpmPackageDist,
}

#[derive(Debug, Deserialize)]
struct NpmPackageDist {
    tarball: String,
    integrity: String,
    #[serde(default, rename = "unpackedSize")]
    unpacked_size: Option<u64>,
}

#[derive(Debug, Clone)]
struct RuntimeCandidate {
    program: PathBuf,
    source: CodexRuntimeSource,
}

#[derive(Debug)]
struct ProbedRuntime {
    display_version: String,
    version: Version,
}

pub struct CodexRuntimeManager {
    runtime_root: PathBuf,
    codex_home: PathBuf,
    client: reqwest::Client,
    status: RwLock<CodexRuntimeStatus>,
    recommended_version: RwLock<String>,
    install_lock: Mutex<()>,
    latest_check_lock: Mutex<()>,
    installing: AtomicBool,
    allow_system_discovery: bool,
}

impl CodexRuntimeManager {
    pub fn new(data_dir: impl AsRef<Path>) -> AppResult<Self> {
        Self::with_system_discovery(data_dir, true)
    }

    #[cfg(test)]
    pub fn new_available_for_tests(data_dir: impl AsRef<Path>) -> AppResult<Self> {
        let mut manager = Self::with_system_discovery(data_dir, false)?;
        let program = if cfg!(windows) { "codex.exe" } else { "codex" };
        manager.status = RwLock::new(CodexRuntimeStatus {
            supported: true,
            available: true,
            managed_installed: false,
            installing: false,
            target_version: FALLBACK_CODEX_VERSION.to_string(),
            update_available: false,
            platform: platform_package_suffix().map(str::to_string),
            source: Some(CodexRuntimeSource::Environment),
            detected_version: Some("codex-cli test".to_string()),
            executable: Some(program.to_string()),
            error: None,
        });
        Ok(manager)
    }

    fn with_system_discovery(
        data_dir: impl AsRef<Path>,
        allow_system_discovery: bool,
    ) -> AppResult<Self> {
        let runtime_root = data_dir.as_ref().join("runtimes").join("codex");
        let codex_home = data_dir.as_ref().join("runtimes").join("codex-home");
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(15 * 60))
            .user_agent(format!("WeiAgent/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                AppError::Process(format!("failed to build runtime client: {error}"))
            })?;
        let platform = platform_package_suffix().map(str::to_string);
        Ok(Self {
            runtime_root,
            codex_home,
            client,
            status: RwLock::new(CodexRuntimeStatus {
                supported: platform.is_some(),
                available: false,
                managed_installed: false,
                installing: false,
                target_version: FALLBACK_CODEX_VERSION.to_string(),
                update_available: false,
                platform,
                source: None,
                detected_version: None,
                executable: None,
                error: None,
            }),
            recommended_version: RwLock::new(FALLBACK_CODEX_VERSION.to_string()),
            install_lock: Mutex::new(()),
            latest_check_lock: Mutex::new(()),
            installing: AtomicBool::new(false),
            allow_system_discovery,
        })
    }

    pub async fn initialize(self: &Arc<Self>) -> AppResult<CodexRuntimeStatus> {
        fs::create_dir_all(&self.runtime_root).await?;
        fs::create_dir_all(&self.codex_home).await?;
        if let Some(cache) = self.latest_version_cache().await {
            *self.recommended_version.write().await = cache.version;
        }
        let status = self.refresh_local().await?;
        let manager = self.clone();
        tokio::spawn(async move {
            if let Err(error) = manager.update_recommended_version(false).await {
                tracing::warn!(%error, "failed to check for a newer Codex runtime");
            }
            if let Err(error) = manager.refresh_local().await {
                tracing::warn!(%error, "failed to refresh Codex runtime after update check");
            }
        });
        Ok(status)
    }

    pub async fn status(&self) -> CodexRuntimeStatus {
        self.status.read().await.clone()
    }

    pub fn is_available_cached(&self) -> bool {
        self.status
            .try_read()
            .map(|status| status.available)
            .unwrap_or(false)
    }

    pub async fn executable(&self) -> Option<CodexExecutable> {
        let status = self.status.read().await;
        if !status.available {
            return None;
        }
        let executable = status.executable.as_ref()?;
        Some(CodexExecutable {
            program: PathBuf::from(executable),
            codex_home: self.codex_home.clone(),
        })
    }

    pub async fn refresh(&self) -> AppResult<CodexRuntimeStatus> {
        if let Err(error) = self.update_recommended_version(true).await {
            tracing::warn!(%error, "failed to refresh the recommended Codex version");
        }
        self.refresh_local().await
    }

    async fn refresh_local(&self) -> AppResult<CodexRuntimeStatus> {
        let platform = platform_package_suffix().map(str::to_string);
        let candidates = self.candidates().await;
        let managed_installed = candidates
            .iter()
            .any(|candidate| candidate.source == CodexRuntimeSource::Managed);
        let target_version = self.recommended_version.read().await.clone();
        let mut last_error = None;
        let mut selected = None;

        for candidate in candidates {
            let program = preferred_native_program(&candidate.program)
                .unwrap_or_else(|| candidate.program.clone());
            match probe_codex(&program, &self.codex_home).await {
                Ok(probed) => {
                    let is_environment = candidate.source == CodexRuntimeSource::Environment;
                    let should_select =
                        selected
                            .as_ref()
                            .is_none_or(|(_, _, current): &(_, _, ProbedRuntime)| {
                                probed.version > current.version
                            });
                    if is_environment || should_select {
                        selected = Some((candidate, program, probed));
                    }
                    if is_environment {
                        break;
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }

        if let Some((candidate, program, probed)) = selected {
            let update_available = candidate.source != CodexRuntimeSource::Environment
                && Version::parse(&target_version).is_ok_and(|target| probed.version < target);
            let status = CodexRuntimeStatus {
                supported: platform.is_some(),
                available: true,
                managed_installed,
                installing: self.installing.load(Ordering::SeqCst),
                target_version,
                update_available,
                platform,
                source: Some(candidate.source),
                detected_version: Some(probed.display_version),
                executable: Some(program.to_string_lossy().to_string()),
                error: None,
            };
            *self.status.write().await = status.clone();
            return Ok(status);
        }

        let status = CodexRuntimeStatus {
            supported: platform.is_some(),
            available: false,
            managed_installed,
            installing: self.installing.load(Ordering::SeqCst),
            target_version,
            update_available: false,
            platform,
            source: None,
            detected_version: None,
            executable: None,
            error: last_error,
        };
        *self.status.write().await = status.clone();
        Ok(status)
    }

    pub async fn install(self: &Arc<Self>) -> AppResult<CodexRuntimeStatus> {
        let _guard = self.install_lock.try_lock().map_err(|_| {
            AppError::Conflict("Codex runtime installation is already in progress".to_string())
        })?;
        let package_suffix = platform_package_suffix().ok_or_else(|| {
            AppError::BadRequest(
                "managed Codex runtime is not available for this platform".to_string(),
            )
        })?;
        self.installing.store(true, Ordering::SeqCst);
        {
            let mut status = self.status.write().await;
            status.installing = true;
            status.error = None;
        }

        if let Err(error) = self.update_recommended_version(true).await {
            tracing::warn!(%error, "using the cached or fallback Codex version for installation");
        }
        let target_version = self.recommended_version.read().await.clone();
        let result = self.install_inner(package_suffix, &target_version).await;
        self.installing.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => self.refresh_local().await,
            Err(error) => {
                let mut status = self.status.write().await;
                status.installing = false;
                status.error = Some(error.to_string());
                Err(error)
            }
        }
    }

    async fn install_inner(&self, package_suffix: &str, target_version: &str) -> AppResult<()> {
        fs::create_dir_all(&self.runtime_root).await?;
        fs::create_dir_all(&self.codex_home).await?;

        let package_version = format!("{target_version}-{package_suffix}");
        let metadata_url = format!("https://registry.npmjs.org/@openai%2Fcodex/{package_version}");
        let metadata = self
            .client
            .get(metadata_url)
            .send()
            .await
            .map_err(runtime_download_error)?
            .error_for_status()
            .map_err(runtime_download_error)?
            .json::<NpmPackageMetadata>()
            .await
            .map_err(runtime_download_error)?;
        if metadata.version != package_version {
            return Err(AppError::Process(format!(
                "runtime registry returned unexpected version '{}'",
                metadata.version
            )));
        }
        if metadata
            .dist
            .unpacked_size
            .is_some_and(|size| size > MAX_RUNTIME_DOWNLOAD_BYTES)
        {
            return Err(AppError::Process(
                "runtime package exceeds the configured size limit".to_string(),
            ));
        }

        let install_id = Uuid::new_v4().simple().to_string();
        let archive_path = self
            .runtime_root
            .join(format!(".download-{install_id}.tgz"));
        let staging_dir = self.runtime_root.join(format!(".staging-{install_id}"));
        ensure_child_path(&self.runtime_root, &archive_path)?;
        ensure_child_path(&self.runtime_root, &staging_dir)?;
        fs::create_dir_all(&staging_dir).await?;

        let install_result = async {
            self.download_archive(
                &metadata.dist.tarball,
                &metadata.dist.integrity,
                &archive_path,
            )
            .await?;
            let archive_for_extract = archive_path.clone();
            let staging_for_extract = staging_dir.clone();
            tokio::task::spawn_blocking(move || {
                extract_runtime_archive(&archive_for_extract, &staging_for_extract)
            })
            .await
            .map_err(|error| {
                AppError::Process(format!("runtime extraction task failed: {error}"))
            })??;

            let executable = find_runtime_executable(&staging_dir).ok_or_else(|| {
                AppError::Process(
                    "downloaded runtime did not contain the Codex executable".to_string(),
                )
            })?;
            let relative_executable = executable
                .strip_prefix(&staging_dir)
                .map_err(|_| {
                    AppError::Process("runtime executable escaped staging directory".to_string())
                })?
                .to_path_buf();
            let probed = probe_codex(&executable, &self.codex_home)
                .await
                .map_err(AppError::Process)?;
            let expected_version = Version::parse(target_version).map_err(|error| {
                AppError::Process(format!("invalid target Codex version: {error}"))
            })?;
            if probed.version != expected_version {
                return Err(AppError::Process(format!(
                    "downloaded runtime reported version {}, expected {target_version}",
                    probed.version
                )));
            }
            probe_codex_app_server(&executable, &self.codex_home)
                .await
                .map_err(AppError::Process)?;

            let final_dir = self.managed_version_dir(target_version);
            ensure_child_path(&self.runtime_root, &final_dir)?;
            if let Some(parent) = final_dir.parent() {
                fs::create_dir_all(parent).await?;
            }
            if fs::metadata(&final_dir).await.is_ok() {
                fs::remove_dir_all(&final_dir).await?;
            }
            fs::rename(&staging_dir, &final_dir).await?;
            let manifest = ManagedRuntimeManifest {
                package_version,
                integrity: metadata.dist.integrity,
                executable: relative_executable.to_string_lossy().to_string(),
                installed_at: chrono::Utc::now(),
            };
            fs::write(
                final_dir.join(RUNTIME_MANIFEST_FILE),
                serde_json::to_string_pretty(&manifest)?,
            )
            .await?;
            Ok(())
        }
        .await;

        let _ = fs::remove_file(&archive_path).await;
        if fs::metadata(&staging_dir).await.is_ok() {
            let _ = fs::remove_dir_all(&staging_dir).await;
        }
        install_result
    }

    async fn download_archive(
        &self,
        url: &str,
        integrity: &str,
        destination: &Path,
    ) -> AppResult<()> {
        let expected = parse_sha512_integrity(integrity)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(runtime_download_error)?
            .error_for_status()
            .map_err(runtime_download_error)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RUNTIME_DOWNLOAD_BYTES)
        {
            return Err(AppError::Process(
                "runtime download exceeds the configured size limit".to_string(),
            ));
        }

        let mut file = fs::File::create(destination).await?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha512::new();
        let mut received = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(runtime_download_error)?;
            received = received.saturating_add(chunk.len() as u64);
            if received > MAX_RUNTIME_DOWNLOAD_BYTES {
                return Err(AppError::Process(
                    "runtime download exceeds the configured size limit".to_string(),
                ));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        let actual = hasher.finalize();
        if actual.as_slice() != expected.as_slice() {
            return Err(AppError::Process(
                "runtime package integrity verification failed".to_string(),
            ));
        }
        Ok(())
    }

    async fn update_recommended_version(&self, force: bool) -> AppResult<String> {
        let _guard = self.latest_check_lock.lock().await;
        if let Some(cache) = self.latest_version_cache().await
            && (!force || latest_cache_is_fresh(&cache))
        {
            *self.recommended_version.write().await = cache.version.clone();
            return Ok(cache.version);
        }

        let package_suffix = platform_package_suffix().ok_or_else(|| {
            AppError::BadRequest(
                "managed Codex runtime is not available for this platform".to_string(),
            )
        })?;
        let latest = self.fetch_package_metadata("latest").await?;
        let version = parse_release_version(&latest.version)?;
        let minimum = Version::parse(MIN_SUPPORTED_CODEX_VERSION).map_err(|error| {
            AppError::Process(format!("invalid minimum Codex version: {error}"))
        })?;
        if version < minimum {
            return Err(AppError::Process(format!(
                "registry Codex version {version} is older than the minimum supported version {minimum}"
            )));
        }
        let package_version = format!("{version}-{package_suffix}");
        let platform_package = self.fetch_package_metadata(&package_version).await?;
        if platform_package.version != package_version {
            return Err(AppError::Process(format!(
                "runtime registry returned unexpected platform version '{}'",
                platform_package.version
            )));
        }

        let cache = LatestVersionCache {
            version: version.to_string(),
            checked_at: chrono::Utc::now(),
        };
        crate::fsutil::write_atomic(
            &self.runtime_root.join(LATEST_VERSION_CACHE_FILE),
            serde_json::to_string_pretty(&cache)?,
        )
        .await?;
        *self.recommended_version.write().await = cache.version.clone();
        Ok(cache.version)
    }

    async fn fetch_package_metadata(&self, version: &str) -> AppResult<NpmPackageMetadata> {
        let url = format!("https://registry.npmjs.org/@openai%2Fcodex/{version}");
        tokio::time::timeout(LATEST_VERSION_TIMEOUT, async {
            self.client
                .get(url)
                .send()
                .await
                .map_err(runtime_download_error)?
                .error_for_status()
                .map_err(runtime_download_error)?
                .json::<NpmPackageMetadata>()
                .await
                .map_err(runtime_download_error)
        })
        .await
        .map_err(|_| AppError::Process("Codex version lookup timed out".to_string()))?
    }

    async fn latest_version_cache(&self) -> Option<LatestVersionCache> {
        let content = fs::read_to_string(self.runtime_root.join(LATEST_VERSION_CACHE_FILE))
            .await
            .ok()?;
        let cache: LatestVersionCache = serde_json::from_str(&content).ok()?;
        let version = parse_release_version(&cache.version).ok()?;
        let minimum = Version::parse(MIN_SUPPORTED_CODEX_VERSION).ok()?;
        if version < minimum {
            return None;
        }
        Some(cache)
    }

    async fn candidates(&self) -> Vec<RuntimeCandidate> {
        let mut candidates = Vec::new();
        if let Some(path) = std::env::var_os("WEIAGENT_CODEX_BIN") {
            candidates.push(RuntimeCandidate {
                program: PathBuf::from(path),
                source: CodexRuntimeSource::Environment,
            });
        }
        candidates.extend(self.managed_candidates().await);
        if self.allow_system_discovery {
            for program in system_codex_programs() {
                candidates.push(RuntimeCandidate {
                    program: PathBuf::from(program),
                    source: CodexRuntimeSource::System,
                });
            }
        }
        candidates
    }

    async fn managed_candidates(&self) -> Vec<RuntimeCandidate> {
        let Some(package_suffix) = platform_package_suffix() else {
            return Vec::new();
        };
        let Ok(mut entries) = fs::read_dir(&self.runtime_root).await else {
            return Vec::new();
        };
        let mut managed = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let version_text = entry.file_name().to_string_lossy().to_string();
            let Ok(version) = parse_release_version(&version_text) else {
                continue;
            };
            let version_dir = self.managed_version_dir(&version_text);
            let manifest_path = version_dir.join(RUNTIME_MANIFEST_FILE);
            let Ok(content) = fs::read_to_string(manifest_path).await else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<ManagedRuntimeManifest>(&content) else {
                continue;
            };
            if manifest.package_version != format!("{version_text}-{package_suffix}") {
                continue;
            }
            let program = version_dir.join(manifest.executable);
            if ensure_child_path(&version_dir, &program).is_err() {
                continue;
            }
            managed.push((
                version,
                RuntimeCandidate {
                    program,
                    source: CodexRuntimeSource::Managed,
                },
            ));
        }
        managed.sort_by(|left, right| right.0.cmp(&left.0));
        managed
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect()
    }

    fn managed_version_dir(&self, version: &str) -> PathBuf {
        self.runtime_root
            .join(version)
            .join(platform_package_suffix().unwrap_or("unsupported"))
    }
}

fn runtime_download_error(error: reqwest::Error) -> AppError {
    AppError::Process(format!("Codex runtime download failed: {error}"))
}

fn parse_release_version(value: &str) -> AppResult<Version> {
    let version = Version::parse(value)
        .map_err(|error| AppError::Process(format!("invalid Codex version '{value}': {error}")))?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(AppError::Process(format!(
            "Codex version '{value}' is not a stable release"
        )));
    }
    Ok(version)
}

fn parse_codex_version(value: &str) -> Result<Version, String> {
    value
        .split_whitespace()
        .find_map(|part| Version::parse(part.trim_start_matches('v')).ok())
        .ok_or_else(|| format!("Codex returned an unrecognized version '{value}'"))
}

fn latest_cache_is_fresh(cache: &LatestVersionCache) -> bool {
    let age = chrono::Utc::now().signed_duration_since(cache.checked_at);
    age >= chrono::Duration::zero() && age < chrono::Duration::hours(LATEST_VERSION_CACHE_HOURS)
}

async fn probe_codex(program: &Path, codex_home: &Path) -> Result<ProbedRuntime, String> {
    let mut command = Command::new(program);
    command
        .arg("--version")
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| format!("'{} --version' timed out", program.display()))?
        .map_err(|error| format!("failed to start '{}': {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!(
            "'{} --version' failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        return Err(format!(
            "'{} --version' returned no version",
            program.display()
        ));
    }
    let parsed = parse_codex_version(&version)?;
    let minimum = Version::parse(MIN_SUPPORTED_CODEX_VERSION)
        .map_err(|error| format!("invalid minimum Codex version: {error}"))?;
    if parsed < minimum {
        return Err(format!(
            "Codex {} is older than the minimum supported version {MIN_SUPPORTED_CODEX_VERSION}",
            parsed
        ));
    }
    Ok(ProbedRuntime {
        display_version: version,
        version: parsed,
    })
}

async fn probe_codex_app_server(program: &Path, codex_home: &Path) -> Result<(), String> {
    let probe_home = codex_home.join("install-probe");
    fs::create_dir_all(&probe_home)
        .await
        .map_err(|error| format!("failed to prepare Codex app-server probe: {error}"))?;
    let mut command = Command::new(program);
    if cfg!(windows) {
        command.args(["-c", "windows.sandbox=\"unelevated\""]);
    }
    command
        .args(["-c", "features.remote_plugin=false"])
        .args(["-c", "features.plugins=false"])
        .args(["-c", "features.apps=false"])
        .args(["-c", "features.hooks=false"])
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    sanitize_runtime_environment(&mut command, "codex_install_probe");
    command.env("CODEX_HOME", probe_home);

    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to start downloaded Codex app-server '{}': {error}",
            program.display()
        )
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server probe stdin is unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server probe stdout is unavailable".to_string())?;
    let request = serde_json::json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "weiagent-install-probe",
                "title": "WeiAgent",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "experimentalApi": true,
                "optOutNotificationMethods": ["remoteControl/status/changed"]
            }
        }
    });
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|error| format!("failed to initialize downloaded Codex app-server: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("failed to flush Codex app-server probe: {error}"))?;

    let result = tokio::time::timeout(APP_SERVER_PROBE_TIMEOUT, async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|error| format!("failed reading Codex app-server probe: {error}"))?
        {
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(serde_json::Value::as_u64) != Some(1) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!(
                    "downloaded Codex app-server rejected initialization: {error}"
                ));
            }
            if message.get("result").is_some() {
                return Ok(());
            }
        }
        Err("downloaded Codex app-server closed during initialization".to_string())
    })
    .await
    .map_err(|_| "downloaded Codex app-server initialization timed out".to_string())?;
    let _ = child.kill().await;
    result
}

fn parse_sha512_integrity(value: &str) -> AppResult<Vec<u8>> {
    let encoded = value.strip_prefix("sha512-").ok_or_else(|| {
        AppError::Process("runtime registry did not provide SHA-512 integrity".to_string())
    })?;
    BASE64_STANDARD
        .decode(encoded)
        .map_err(|error| AppError::Process(format!("invalid runtime integrity value: {error}")))
}

fn extract_runtime_archive(archive_path: &Path, destination: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let relative = path.strip_prefix("package").map_err(|_| {
            AppError::Process(format!(
                "runtime archive contained an unexpected path '{}'",
                path.display()
            ))
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if relative.components().any(|component| {
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
            return Err(AppError::Process(
                "runtime archive contained an unsupported link".to_string(),
            ));
        }
        let output = destination.join(relative);
        ensure_child_path(destination, &output)?;
        if entry_type.is_dir() {
            std::fs::create_dir_all(&output)?;
        } else if entry_type.is_file() {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output_file = std::fs::File::create(&output)?;
            std::io::copy(&mut entry, &mut output_file)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mode) = entry.header().mode() {
                    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
    }
    Ok(())
}

fn find_runtime_executable(root: &Path) -> Option<PathBuf> {
    let expected = if cfg!(windows) { "codex.exe" } else { "codex" };
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(directory).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name() == Some(OsStr::new(expected))
                && path.parent()?.file_name() == Some(OsStr::new("bin"))
            {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(windows)]
fn preferred_native_program(program: &Path) -> Option<PathBuf> {
    if program
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("exe"))
    {
        return Some(program.to_path_buf());
    }
    let resolved = resolve_windows_program(program)?;
    let package_root = resolved
        .parent()?
        .join("node_modules")
        .join("@openai")
        .join("codex");
    find_runtime_executable(&package_root)
}

#[cfg(not(windows))]
fn preferred_native_program(program: &Path) -> Option<PathBuf> {
    Some(program.to_path_buf())
}

#[cfg(windows)]
fn resolve_windows_program(program: &Path) -> Option<PathBuf> {
    if program.is_absolute() {
        return program.is_file().then(|| program.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn ensure_child_path(root: &Path, path: &Path) -> AppResult<()> {
    let root = absolute_lexical(root)?;
    let path = absolute_lexical(path)?;
    if !path.starts_with(&root) || path == root {
        return Err(AppError::Process(format!(
            "runtime path '{}' escaped '{}'",
            path.display(),
            root.display()
        )));
    }
    Ok(())
}

fn absolute_lexical(path: &Path) -> AppResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    Ok(output)
}

fn platform_package_suffix() -> Option<&'static str> {
    platform_package_suffix_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn platform_package_suffix_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("win32-x64"),
        ("windows", "aarch64") => Some("win32-arm64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

fn system_codex_programs() -> &'static [&'static str] {
    if cfg!(windows) {
        &["codex.cmd", "codex.exe"]
    } else {
        &["codex"]
    }
}

const ALLOWED_RUNTIME_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "USERNAME",
    "LOGNAME",
    "USERPROFILE",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "SystemDrive",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "GOPATH",
    "GOROOT",
    "JAVA_HOME",
    "NODE_PATH",
    "NVM_DIR",
    "PYENV_ROOT",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "HOMEBREW_PREFIX",
    "HOMEBREW_CELLAR",
    "HOMEBREW_REPOSITORY",
];

pub(crate) fn sanitize_runtime_environment(command: &mut Command, runtime: &str) {
    let preserved = ALLOWED_RUNTIME_ENV
        .iter()
        .filter_map(|key| std::env::var_os(key).map(|value| ((*key).to_string(), value)))
        .collect::<Vec<_>>();
    command.env_clear();
    for (key, value) in preserved {
        command.env(key, value);
    }
    if std::env::var_os("TMPDIR").is_none() {
        let fallback = std::env::temp_dir();
        if !fallback.as_os_str().is_empty() {
            command.env("TMPDIR", fallback);
        }
    }
    command.env("WEIAGENT_RUNTIME", runtime);
}

pub(crate) fn sanitized_environment_overrides(
    runtime: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut overrides = serde_json::Map::new();
    for (key, _) in std::env::vars() {
        if !ALLOWED_RUNTIME_ENV
            .iter()
            .any(|allowed| key.eq_ignore_ascii_case(allowed))
        {
            overrides.insert(key, serde_json::Value::Null);
        }
    }
    overrides.insert(
        "WEIAGENT_RUNTIME".to_string(),
        serde_json::Value::String(runtime.to_string()),
    );
    overrides
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrity_requires_sha512() {
        assert!(parse_sha512_integrity("sha256-abc").is_err());
        assert_eq!(
            parse_sha512_integrity("sha512-YWJj").unwrap(),
            b"abc".to_vec()
        );
    }

    #[test]
    fn runtime_paths_must_stay_under_the_runtime_root() {
        let root = PathBuf::from("runtime");
        assert!(ensure_child_path(&root, &root.join("version")).is_ok());
        assert!(ensure_child_path(&root, &root.join("..").join("outside")).is_err());
    }

    #[test]
    fn codex_version_output_is_parsed_as_semver() {
        assert_eq!(
            parse_codex_version("codex-cli 0.144.5").unwrap(),
            Version::new(0, 144, 5)
        );
        assert_eq!(
            parse_codex_version("codex v1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
        assert!(parse_codex_version("codex development").is_err());
    }

    #[test]
    fn recommended_versions_must_be_stable_releases() {
        assert_eq!(
            parse_release_version("0.144.5").unwrap(),
            Version::new(0, 144, 5)
        );
        assert!(parse_release_version("0.144.5-beta.1").is_err());
        assert!(parse_release_version("0.144.5+build").is_err());
        assert!(parse_release_version("latest").is_err());
    }

    #[test]
    fn latest_version_cache_expires_after_one_day() {
        let fresh = LatestVersionCache {
            version: "0.144.5".to_string(),
            checked_at: chrono::Utc::now() - chrono::Duration::hours(23),
        };
        let stale = LatestVersionCache {
            version: "0.144.5".to_string(),
            checked_at: chrono::Utc::now() - chrono::Duration::hours(25),
        };
        assert!(latest_cache_is_fresh(&fresh));
        assert!(!latest_cache_is_fresh(&stale));
    }

    #[test]
    fn managed_runtime_supports_mainstream_windows_linux_and_macos_targets() {
        assert_eq!(
            platform_package_suffix_for("windows", "x86_64"),
            Some("win32-x64")
        );
        assert_eq!(
            platform_package_suffix_for("windows", "aarch64"),
            Some("win32-arm64")
        );
        assert_eq!(
            platform_package_suffix_for("linux", "x86_64"),
            Some("linux-x64")
        );
        assert_eq!(
            platform_package_suffix_for("linux", "aarch64"),
            Some("linux-arm64")
        );
        assert_eq!(
            platform_package_suffix_for("macos", "x86_64"),
            Some("darwin-x64")
        );
        assert_eq!(
            platform_package_suffix_for("macos", "aarch64"),
            Some("darwin-arm64")
        );
        assert_eq!(platform_package_suffix_for("freebsd", "x86_64"), None);
    }
}
