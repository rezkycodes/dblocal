//! Download update assets with HTTPS-only, host allowlist, size cap, disk-space
//! preflight, progress reporting, cancellation, and SHA-256 verification.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::ACCEPT_ENCODING;
use tokio::io::AsyncWriteExt;

use crate::download::{CancellationToken, FreeSpaceFn, check_disk_space};

use super::error::{UpdateError, UpdateResult};
use super::model::{AvailableUpdate, UpdateAssetKind, UpdateProgress};
#[cfg(test)]
use super::verify::Sha256ReleaseVerifier;
use super::verify::{Checksums, ReleaseVerifier, verify_sha256_file};

/// Hosts allowed for update downloads/redirects.
pub const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "github-releases.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
/// Maximum download size (60 MB + 5 MB slack).
pub const MAX_UPDATE_BYTES: u64 = 65 * 1024 * 1024;
/// Connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Read timeout.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Fixed staged file name; never derived from release JSON.
const STAGED_ASSET_NAME: &str = "dblocal-update.bin";
/// Fixed staged checksums file name.
const STAGED_CHECKSUMS_NAME: &str = "checksums.txt";
/// Fixed staged signature file name.
const STAGED_SIGNATURE_NAME: &str = "checksums.txt.sig";
/// Maximum signature file size accepted (raw 64-byte Ed25519 signature + slack).
const MAX_SIGNATURE_BODY_BYTES: u64 = 256;

/// Callback receiving [`UpdateProgress`] updates during download and verification.
pub type UpdateProgressSink = Arc<dyn Fn(UpdateProgress) + Send + Sync>;

/// Options for update download.
#[derive(Clone)]
pub struct UpdateDownloadOptions {
    /// Allow only loopback http in tests/debug builds.
    allow_loopback_http: bool,
    /// Max bytes allowed.
    max_bytes: u64,
    /// Optional disk-space probe override for tests.
    free_space_probe: Option<FreeSpaceFn>,
}

impl std::fmt::Debug for UpdateDownloadOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateDownloadOptions")
            .field("allow_loopback_http", &self.allow_loopback_http)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl Default for UpdateDownloadOptions {
    fn default() -> Self {
        Self {
            allow_loopback_http: false,
            max_bytes: MAX_UPDATE_BYTES,
            free_space_probe: None,
        }
    }
}

impl UpdateDownloadOptions {
    /// Test/debug-only constructor that allows loopback plain HTTP (unavailable in release builds).
    #[cfg(any(test, debug_assertions))]
    pub fn loopback_for_tests() -> Self {
        Self {
            allow_loopback_http: true,
            max_bytes: MAX_UPDATE_BYTES,
            free_space_probe: None,
        }
    }

    /// Test/debug-only constructor with a custom max-bytes cap (unavailable in release builds).
    #[cfg(any(test, debug_assertions))]
    pub fn loopback_for_tests_with_max(max_bytes: u64) -> Self {
        Self {
            allow_loopback_http: true,
            max_bytes,
            free_space_probe: None,
        }
    }

    /// Injects a disk-space probe (for tests).
    #[cfg(any(test, debug_assertions))]
    pub fn with_free_space_probe(mut self, probe: FreeSpaceFn) -> Self {
        self.free_space_probe = Some(probe);
        self
    }

    /// Returns true when loopback plain HTTP is allowed.
    pub(crate) fn allows_loopback_http(&self) -> bool {
        #[cfg(not(any(test, debug_assertions)))]
        {
            false
        }
        #[cfg(any(test, debug_assertions))]
        {
            self.allow_loopback_http
        }
    }
}

/// A verified update asset staged inside a fresh private directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    version: semver::Version,
    staged_path: PathBuf,
    run_dir: PathBuf,
    asset_name: String,
    asset_kind: UpdateAssetKind,
}

impl StagedUpdate {
    /// Version of the staged update.
    pub fn version(&self) -> &semver::Version {
        &self.version
    }

    /// Path to the verified staged file.
    pub fn staged_path(&self) -> &Path {
        &self.staged_path
    }

    /// Private staging directory created for this attempt.
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// Original asset name from release metadata, for diagnostics only.
    pub fn asset_name(&self) -> &str {
        &self.asset_name
    }

    /// Kind of the staged asset (e.g. `TarGz`, `AppImage`, `Zip`, `Msi`).
    pub fn asset_kind(&self) -> UpdateAssetKind {
        self.asset_kind
    }

    /// Removes the private staging directory for this attempt.
    pub fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.run_dir);
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_any(
        version: semver::Version,
        staged_path: PathBuf,
        run_dir: PathBuf,
        asset_name: String,
        asset_kind: UpdateAssetKind,
    ) -> Self {
        Self {
            version,
            staged_path,
            run_dir,
            asset_name,
            asset_kind,
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn new_for_test(
        version: semver::Version,
        staged_path: PathBuf,
        run_dir: PathBuf,
        asset_name: String,
        asset_kind: UpdateAssetKind,
    ) -> Self {
        Self {
            version,
            staged_path,
            run_dir,
            asset_name,
            asset_kind,
        }
    }
}

/// Test-only helper: downloads checksums and asset for `update` using the
/// unsigned SHA-256-only verifier. Production callers must use
/// [`download_update_with_progress`] with [`super::Ed25519ReleaseVerifier`].
#[cfg(test)]
pub async fn download_update(
    update: &AvailableUpdate,
    staging_dir: &Path,
    options: &UpdateDownloadOptions,
) -> UpdateResult<StagedUpdate> {
    let verifier = Sha256ReleaseVerifier;
    let cancel = CancellationToken::new();
    download_update_with_progress(update, staging_dir, options, &verifier, &cancel, None).await
}

/// Full download-and-verify pipeline with progress reporting, cancellation,
/// disk-space preflight, and pluggable [`ReleaseVerifier`].
///
/// Any error or cancellation deletes the fresh private staging directory so no
/// partial downloads are left behind.
pub async fn download_update_with_progress(
    update: &AvailableUpdate,
    staging_dir: &Path,
    options: &UpdateDownloadOptions,
    verifier: &dyn ReleaseVerifier,
    cancel: &CancellationToken,
    on_progress: Option<&UpdateProgressSink>,
) -> UpdateResult<StagedUpdate> {
    if cancel.is_cancelled() {
        return Err(UpdateError::Cancelled);
    }

    let declared_size = update.asset().size_bytes();
    let max_bytes = effective_max(declared_size, options.max_bytes)?;

    tokio::fs::create_dir_all(staging_dir)
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not create the update staging directory: {source}"),
        })?;

    check_disk_space(
        staging_dir,
        declared_size,
        options.free_space_probe.as_ref(),
    )
    .map_err(|err| UpdateError::DiskSpace {
        reason: err.to_string(),
    })?;

    let run_dir = fresh_private_run_dir(staging_dir)?;
    let outcome = download_into_run_dir(
        update,
        &run_dir,
        max_bytes,
        options,
        verifier,
        cancel,
        on_progress,
    )
    .await;

    match outcome {
        Ok(staged) => Ok(staged),
        Err(err) => {
            let _ = tokio::fs::remove_dir_all(&run_dir).await;
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_into_run_dir(
    update: &AvailableUpdate,
    run_dir: &Path,
    max_bytes: u64,
    options: &UpdateDownloadOptions,
    verifier: &dyn ReleaseVerifier,
    cancel: &CancellationToken,
    on_progress: Option<&UpdateProgressSink>,
) -> UpdateResult<StagedUpdate> {
    let checksums = fetch_checksums(update, run_dir, options, verifier, cancel).await?;
    let expected = checksums.digest_for(update.asset().name()).ok_or_else(|| {
        UpdateError::ChecksumMetadata {
            reason: format!(
                "checksums.txt has no entry for asset {}",
                update.asset().name()
            ),
        }
    })?;

    let staged_path = run_dir.join(STAGED_ASSET_NAME);
    let parsed_url = parse_and_validate_url(update.asset().url(), options)?;
    download_to_file_with_progress(
        &parsed_url,
        &staged_path,
        max_bytes,
        Some(update.asset().size_bytes()),
        options,
        cancel,
        on_progress,
    )
    .await?;

    if cancel.is_cancelled() {
        return Err(UpdateError::Cancelled);
    }

    if let Some(sink) = on_progress {
        sink(UpdateProgress::Verifying);
    }
    verifier
        .verify_artifact(&staged_path, expected, update.asset().name())
        .await?;

    Ok(StagedUpdate {
        version: update.version().clone(),
        staged_path,
        run_dir: run_dir.to_path_buf(),
        asset_name: update.asset().name().to_owned(),
        asset_kind: update.asset().kind(),
    })
}

fn fresh_private_run_dir(staging_dir: &Path) -> UpdateResult<PathBuf> {
    let nonce: u64 = rand::random();
    let dir = staging_dir.join(format!("run-{}-{nonce:016x}", std::process::id()));
    super::extract::ensure_private_dir(&dir)?;
    Ok(dir)
}

async fn fetch_checksums(
    update: &AvailableUpdate,
    run_dir: &Path,
    options: &UpdateDownloadOptions,
    verifier: &dyn ReleaseVerifier,
    cancel: &CancellationToken,
) -> UpdateResult<Checksums> {
    let checksums_path = run_dir.join(STAGED_CHECKSUMS_NAME);
    let url = parse_and_validate_url(update.checksums_asset().url(), options)?;
    let declared = update.checksums_asset().size_bytes();
    let hard_cap = super::check::MAX_CHECKSUMS_BODY_BYTES as u64;
    if declared == 0 || declared > hard_cap {
        return Err(UpdateError::ChecksumMetadata {
            reason: format!("invalid checksums.txt size: {declared}"),
        });
    }
    let max_bytes = declared.saturating_add(1024).min(hard_cap);
    download_to_file_with_progress(
        &url,
        &checksums_path,
        max_bytes,
        None,
        options,
        cancel,
        None,
    )
    .await?;
    let bytes = read_bounded_bytes(&checksums_path, hard_cap).await?;
    let _ = tokio::fs::remove_file(&checksums_path).await;

    let sig_bytes = if let Some(sig_asset) = update.signature_asset() {
        let sig_path = run_dir.join(STAGED_SIGNATURE_NAME);
        let sig_url = parse_and_validate_url(sig_asset.url(), options)?;
        let sig_declared = sig_asset.size_bytes();
        if sig_declared == 0 || sig_declared > MAX_SIGNATURE_BODY_BYTES {
            return Err(UpdateError::VerifyFailed {
                reason: format!("invalid checksums.txt.sig size: {sig_declared}"),
            });
        }
        download_to_file_with_progress(
            &sig_url,
            &sig_path,
            MAX_SIGNATURE_BODY_BYTES,
            Some(sig_declared),
            options,
            cancel,
            None,
        )
        .await?;
        let data = read_bounded_bytes(&sig_path, MAX_SIGNATURE_BODY_BYTES).await?;
        let _ = tokio::fs::remove_file(&sig_path).await;
        Some(data)
    } else {
        None
    };

    verifier.verify_manifest(&bytes, sig_bytes.as_deref())
}

async fn read_bounded_bytes(path: &Path, max_bytes: u64) -> UpdateResult<Vec<u8>> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|source| UpdateError::ChecksumMetadata {
            reason: format!("could not read checksums.txt: {source}"),
        })?;
    if meta.len() > max_bytes {
        let _ = tokio::fs::remove_file(path).await;
        return Err(UpdateError::ChecksumMetadata {
            reason: format!("checksums.txt exceeds the limit of {} bytes", max_bytes),
        });
    }
    tokio::fs::read(path)
        .await
        .map_err(|source| UpdateError::ChecksumMetadata {
            reason: format!("could not read checksums.txt: {source}"),
        })
}

/// Downloads `url` to `dest`, verifies sha256 from `expected_hex`, deletes on mismatch.
pub async fn download_and_verify(
    url: &str,
    dest: &Path,
    expected_hex: &str,
    declared_size: u64,
    options: &UpdateDownloadOptions,
) -> UpdateResult<()> {
    let parsed = parse_and_validate_url(url, options)?;
    let max_bytes = effective_max(declared_size, options.max_bytes)?;
    let cancel = CancellationToken::new();
    download_to_file_with_progress(&parsed, dest, max_bytes, None, options, &cancel, None).await?;
    verify_sha256_file(dest, expected_hex, url_label(url)).await
}

fn url_label(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

fn effective_max(declared: u64, hard_cap: u64) -> UpdateResult<u64> {
    if declared == 0 {
        return Err(UpdateError::Malformed {
            reason: "invalid update asset size".to_owned(),
        });
    }
    if declared > hard_cap {
        return Err(UpdateError::Malformed {
            reason: format!("the update asset size exceeds the limit: {declared}"),
        });
    }
    let max = declared.saturating_add(1024 * 1024).min(hard_cap);
    Ok(max)
}

fn parse_and_validate_url(
    url: &str,
    options: &UpdateDownloadOptions,
) -> UpdateResult<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).map_err(|e| UpdateError::Malformed {
        reason: format!("invalid update URL: {e}"),
    })?;
    validate_url(&parsed, options)?;
    Ok(parsed)
}

/// Validates URL scheme and host allowlist.
pub fn validate_url(url: &reqwest::Url, options: &UpdateDownloadOptions) -> UpdateResult<()> {
    if redirect_allowed(url, options.allows_loopback_http()) {
        return Ok(());
    }
    Err(UpdateError::Network {
        reason: format!("update URL rejected: {}", redact_url(url)),
    })
}

/// Pure policy check for download URLs and redirect targets.
///
/// HTTPS is allowed only for hosts in [`ALLOWED_HOSTS`]. Plain HTTP is allowed
/// only for loopback hosts when `allow_loopback_http` is true (tests/debug).
pub fn redirect_allowed(url: &reqwest::Url, allow_loopback_http: bool) -> bool {
    let scheme = url.scheme();
    if scheme == "https" {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        return ALLOWED_HOSTS.iter().any(|h| *h == host);
    }
    if scheme == "http" && allow_loopback_http {
        return is_loopback(url);
    }
    false
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn redact_url(url: &reqwest::Url) -> String {
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(url.path());
    out
}

fn build_client(options: &UpdateDownloadOptions) -> UpdateResult<reqwest::Client> {
    let allow_loopback = options.allows_loopback_http();
    reqwest::Client::builder()
        .https_only(!allow_loopback)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent(format!("dblocal/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many redirects");
            }
            let url = attempt.url().clone();
            if redirect_allowed(&url, allow_loopback) {
                attempt.follow()
            } else {
                attempt.error(format!("redirect host not allowed: {}", redact_url(&url)))
            }
        }))
        .build()
        .map_err(|e| UpdateError::Network {
            reason: format!("could not build the HTTP client: {e}"),
        })
}

async fn download_to_file_with_progress(
    url: &reqwest::Url,
    dest: &Path,
    max_bytes: u64,
    declared_hint: Option<u64>,
    options: &UpdateDownloadOptions,
    cancel: &CancellationToken,
    on_progress: Option<&UpdateProgressSink>,
) -> UpdateResult<()> {
    if cancel.is_cancelled() {
        return Err(UpdateError::Cancelled);
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not create the download directory: {source}"),
            })?;
    }
    let client = build_client(options)?;
    let resp = client
        .get(url.clone())
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| UpdateError::Network {
            reason: e.to_string(),
        })?;

    let status = resp.status();
    if !status.is_success() {
        return Err(UpdateError::Network {
            reason: format!("unexpected HTTP status: {status}"),
        });
    }
    let content_length = resp.content_length();
    if let Some(total) = content_length
        && total > max_bytes
    {
        return Err(UpdateError::Network {
            reason: format!("the update response exceeds the limit of {max_bytes} bytes"),
        });
    }
    let total_for_progress = content_length.or(declared_hint);
    if let Some(sink) = on_progress {
        sink(UpdateProgress::Downloading {
            received_bytes: 0,
            total_bytes: total_for_progress,
        });
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not open the download file: {source}"),
        })?;
    let mut downloaded: u64 = 0;
    let mut stream = resp;
    loop {
        let Some(chunk) = next_chunk_or_cancel(&mut stream, cancel).await? else {
            break;
        };
        let next = downloaded.saturating_add(chunk.len() as u64);
        if next > max_bytes {
            drop(file);
            let _ = tokio::fs::remove_file(dest).await;
            return Err(UpdateError::Network {
                reason: format!("the update response exceeds the limit of {max_bytes} bytes"),
            });
        }
        file.write_all(&chunk)
            .await
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not write the download file: {source}"),
            })?;
        downloaded = next;
        if let Some(sink) = on_progress {
            sink(UpdateProgress::Downloading {
                received_bytes: downloaded,
                total_bytes: total_for_progress,
            });
        }
    }

    if let Some(expected_len) = content_length
        && downloaded < expected_len
    {
        drop(file);
        let _ = tokio::fs::remove_file(dest).await;
        return Err(UpdateError::Network {
            reason: format!(
                "truncated update download: received {downloaded} of {expected_len} bytes"
            ),
        });
    }

    file.flush()
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not flush the download file: {source}"),
        })?;
    file.sync_all()
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not fsync the download file: {source}"),
        })?;
    Ok(())
}

async fn next_chunk_or_cancel(
    stream: &mut reqwest::Response,
    cancel: &CancellationToken,
) -> UpdateResult<Option<Vec<u8>>> {
    if cancel.is_cancelled() {
        return Err(UpdateError::Cancelled);
    }
    let cancel_wait = async {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::select! {
        biased;
        () = cancel_wait => Err(UpdateError::Cancelled),
        chunk = stream.chunk() => chunk
            .map(|opt| opt.map(|b| b.to_vec()))
            .map_err(|e| UpdateError::Network {
                reason: e.to_string(),
            }),
    }
}

#[cfg(test)]
#[path = "download/tests.rs"]
mod tests;
