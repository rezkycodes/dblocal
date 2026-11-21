//! Download update assets with HTTPS-only, host allowlist, size cap, and SHA-256 verification.

use std::path::Path;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT_ENCODING, RANGE};
use tokio::io::AsyncWriteExt;

use super::error::{UpdateError, UpdateResult};
use super::verify::verify_sha256_file;

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

/// Options for update download.
#[derive(Debug, Clone)]
pub struct UpdateDownloadOptions {
    /// Allow only loopback http in tests.
    pub allow_loopback_http: bool,
    /// Max bytes allowed.
    pub max_bytes: u64,
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
    #[cfg(test)]
    pub fn loopback_for_tests() -> Self {
        Self {
            allow_loopback_http: true,
            max_bytes: MAX_UPDATE_BYTES,
        }
    }
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
            reason: "ukuran asset update tidak valid".to_owned(),
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
        reason: format!("URL update tidak valid: {e}"),
    })?;
    validate_url(&parsed, options)?;
    Ok(parsed)
}

/// Validates URL scheme and host allowlist.
pub fn validate_url(url: &reqwest::Url, options: &UpdateDownloadOptions) -> UpdateResult<()> {
    let scheme = url.scheme();
    if scheme == "https" {
        validate_allowed_host(url)?;
        return Ok(());
    }
    if scheme == "http" && options.allow_loopback_http && is_loopback(url) {
        return Ok(());
    }
    Err(UpdateError::Network {
        reason: format!("URL update ditolak: {}", redact_url(url)),
    })
}

fn validate_allowed_host(url: &reqwest::Url) -> UpdateResult<()> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if ALLOWED_HOSTS.iter().any(|h| *h == host) {
        return Ok(());
    }
    Err(UpdateError::Network {
        reason: format!("host update tidak diizinkan: {host}"),
    })
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
    let allow_loopback = options.allow_loopback_http;
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
            let ok = if url.scheme() == "https" {
                let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
                ALLOWED_HOSTS.iter().any(|h| *h == host)
            } else if url.scheme() == "http" && allow_loopback {
                is_loopback(&url)
            } else {
                false
            };
            if ok {
                attempt.follow()
            } else {
                attempt.error(format!("redirect host not allowed: {}", redact_url(&url)))
            }
        }))
        .build()
        .map_err(|e| UpdateError::Network {
            reason: format!("gagal membuat HTTP client: {e}"),
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
                reason: format!("gagal membuat direktori unduhan: {source}"),
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
            reason: format!("status HTTP tidak terduga: {status}"),
        });
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .append(append)
        .open(dest)
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("gagal membuka file unduhan: {source}"),
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
                reason: format!("gagal menulis file unduhan: {source}"),
            })?;
        downloaded = next;
    }
    file.flush()
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("gagal flush file unduhan: {source}"),
        })?;
    file.sync_all()
        .await
        .map_err(|source| UpdateError::ApplyFailed {
            reason: format!("gagal fsync file unduhan: {source}"),
        })?;
    Ok(())
}

#[cfg(test)]
#[path = "download/tests.rs"]
mod tests;
