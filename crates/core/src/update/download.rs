//! Download update assets with HTTPS-only, host allowlist, size cap, and SHA-256 verification.

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT_ENCODING, RANGE};
use tokio::io::AsyncWriteExt;

use super::error::{UpdateError, UpdateResult};
use super::model::AvailableUpdate;
use super::verify::{Checksums, verify_sha256_file};

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

/// Options for update download.
#[derive(Debug, Clone)]
pub struct UpdateDownloadOptions {
    /// Allow only loopback http in tests.
    allow_loopback_http: bool,
    /// Max bytes allowed.
    max_bytes: u64,
}

impl Default for UpdateDownloadOptions {
    fn default() -> Self {
        Self {
            allow_loopback_http: false,
            max_bytes: MAX_UPDATE_BYTES,
        }
    }
}

impl UpdateDownloadOptions {
    /// Test-only constructor that allows loopback plain HTTP.
    #[cfg(test)]
    pub fn loopback_for_tests() -> Self {
        Self {
            allow_loopback_http: true,
            max_bytes: MAX_UPDATE_BYTES,
        }
    }

    /// Test-only constructor with a custom max-bytes cap.
    #[cfg(test)]
    pub fn loopback_for_tests_with_max(max_bytes: u64) -> Self {
        Self {
            allow_loopback_http: true,
            max_bytes,
        }
    }

    /// Returns true when loopback plain HTTP is allowed.
    pub(crate) fn allows_loopback_http(&self) -> bool {
        self.allow_loopback_http
    }
}

/// A verified update asset staged inside a caller-provided directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    version: semver::Version,
    staged_path: PathBuf,
    asset_name: String,
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

    /// Original asset name from release metadata, for diagnostics only.
    pub fn asset_name(&self) -> &str {
        &self.asset_name
    }
}

/// Downloads checksums and asset for `update`, verifies SHA-256, and stages the
/// asset under a fixed file name inside `staging_dir`.
pub async fn download_update(
    update: &AvailableUpdate,
    staging_dir: &Path,
    options: &UpdateDownloadOptions,
) -> UpdateResult<StagedUpdate> {
    tokio::fs::create_dir_all(staging_dir)
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not create the update staging directory: {source}"),
        })?;

    let checksums = fetch_checksums(update, staging_dir, options).await?;
    let expected = checksums.digest_for(update.asset().name()).ok_or_else(|| {
        UpdateError::ChecksumMetadata {
            reason: format!(
                "checksums.txt has no entry for asset {}",
                update.asset().name()
            ),
        }
    })?;

    let staged_path = staging_dir.join(STAGED_ASSET_NAME);
    download_and_verify(
        update.asset().url(),
        &staged_path,
        expected,
        update.asset().size_bytes(),
        options,
    )
    .await?;

    Ok(StagedUpdate {
        version: update.version().clone(),
        staged_path,
        asset_name: update.asset().name().to_owned(),
    })
}

async fn fetch_checksums(
    update: &AvailableUpdate,
    staging_dir: &Path,
    options: &UpdateDownloadOptions,
) -> UpdateResult<Checksums> {
    let checksums_path = staging_dir.join(STAGED_CHECKSUMS_NAME);
    let url = parse_and_validate_url(update.checksums_asset().url(), options)?;
    let declared = update.checksums_asset().size_bytes();
    let hard_cap = super::check::MAX_CHECKSUMS_BODY_BYTES as u64;
    if declared == 0 || declared > hard_cap {
        return Err(UpdateError::ChecksumMetadata {
            reason: format!("invalid checksums.txt size: {declared}"),
        });
    }
    let max_bytes = declared.saturating_add(1024).min(hard_cap);
    download_to_file(&url, &checksums_path, max_bytes, options).await?;
    let body = read_bounded_text(&checksums_path, hard_cap).await?;
    let _ = tokio::fs::remove_file(&checksums_path).await;
    Checksums::parse(&body)
}

async fn read_bounded_text(path: &Path, max_bytes: u64) -> UpdateResult<String> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|source| UpdateError::ChecksumMetadata {
            reason: format!("could not read checksums.txt: {source}"),
        })?;
    if meta.len() > max_bytes {
        let _ = tokio::fs::remove_file(path).await;
        return Err(UpdateError::ChecksumMetadata {
            reason: format!("checksums.txt melebihi batas {} byte", max_bytes),
        });
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|source| UpdateError::ChecksumMetadata {
            reason: format!("could not read checksums.txt: {source}"),
        })?;
    String::from_utf8(bytes).map_err(|_| UpdateError::ChecksumMetadata {
        reason: "checksums.txt bukan UTF-8 yang valid".to_owned(),
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
    download_to_file(&parsed, dest, max_bytes, options).await?;
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
    let max = declared.saturating_add(1024 * 1024); // 1MB slack over metadata
    let max = max.min(hard_cap);
    if declared > hard_cap {
        return Err(UpdateError::Malformed {
            reason: format!("ukuran asset update melebihi batas: {declared}"),
        });
    }
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
        reason: format!("URL update ditolak: {}", redact_url(url)),
    })
}

/// Pure policy check for download URLs and redirect targets.
///
/// HTTPS is allowed only for hosts in [`ALLOWED_HOSTS`]. Plain HTTP is allowed
/// only for loopback hosts when `allow_loopback_http` is true (tests).
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

async fn download_to_file(
    url: &reqwest::Url,
    dest: &Path,
    max_bytes: u64,
    options: &UpdateDownloadOptions,
) -> UpdateResult<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not create the download directory: {source}"),
            })?;
    }
    let client = build_client(options)?;
    let existing = tokio::fs::metadata(dest)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut offset = existing.min(max_bytes);
    if existing > max_bytes {
        let _ = tokio::fs::remove_file(dest).await;
        offset = 0;
    }

    let mut request = client.get(url.clone()).header(ACCEPT_ENCODING, "identity");
    let appending = offset > 0;
    if appending {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let response = request.send().await.map_err(|e| UpdateError::Network {
        reason: e.to_string(),
    })?;
    let status = response.status();
    let mut append = appending;
    let mut resp = response;
    if appending && status == StatusCode::OK {
        let _ = tokio::fs::remove_file(dest).await;
        offset = 0;
        append = false;
    } else if appending && status == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = tokio::fs::remove_file(dest).await;
        offset = 0;
        append = false;
        resp = client
            .get(url.clone())
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|e| UpdateError::Network {
                reason: e.to_string(),
            })?;
    }

    let status = resp.status();
    if !status.is_success() {
        return Err(UpdateError::Network {
            reason: format!("unexpected HTTP status: {status}"),
        });
    }
    enforce_declared_size(&resp, offset, max_bytes, dest).await?;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .append(append)
        .open(dest)
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not open the download file: {source}"),
        })?;
    let mut downloaded = offset;
    let mut stream = resp;
    while let Some(chunk) = stream.chunk().await.map_err(|e| UpdateError::Network {
        reason: e.to_string(),
    })? {
        let next = downloaded.saturating_add(chunk.len() as u64);
        if next > max_bytes {
            drop(file);
            let _ = tokio::fs::remove_file(dest).await;
            return Err(UpdateError::Network {
                reason: format!("respons update melebihi batas {max_bytes} byte"),
            });
        }
        file.write_all(&chunk)
            .await
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not write the download file: {source}"),
            })?;
        downloaded = next;
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

async fn enforce_declared_size(
    resp: &reqwest::Response,
    offset: u64,
    max_bytes: u64,
    dest: &Path,
) -> UpdateResult<()> {
    let declared_total = declared_total_bytes(resp, offset);
    if let Some(total) = declared_total
        && total > max_bytes
    {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(UpdateError::Network {
            reason: format!("respons update melebihi batas {max_bytes} byte"),
        });
    }
    Ok(())
}

fn declared_total_bytes(resp: &reqwest::Response, offset: u64) -> Option<u64> {
    if let Some(total) = content_range_total(resp.headers().get(reqwest::header::CONTENT_RANGE)) {
        return Some(total);
    }
    resp.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|len| offset.saturating_add(len))
}

fn content_range_total(value: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    let text = value?.to_str().ok()?;
    let (_, total) = text.rsplit_once('/')?;
    if total == "*" {
        return None;
    }
    total.parse::<u64>().ok()
}

#[cfg(test)]
#[path = "download/tests.rs"]
mod tests;
