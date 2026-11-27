//! HTTPS-only transport with retries, polite User-Agent, streaming SHA-256
//! downloads, size caps, and disk-space guards.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Polite User-Agent with no personal or machine details.
pub const USER_AGENT: &str = "dblocal-catalog-generator";
/// Maximum HTTPS redirect hops.
pub const MAX_REDIRECTS: usize = 5;
/// Maximum retry attempts for transient HTTP errors.
const MAX_ATTEMPTS: usize = 3;
/// Maximum bytes allowed for a single downloaded artifact (256 MiB).
pub const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum cumulative bytes allowed across one generator run (2 GiB).
pub const MAX_TOTAL_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Minimum free disk space required before downloading an artifact (512 MiB).
pub const MIN_FREE_DISK_BYTES: u64 = 512 * 1024 * 1024;

/// Result of downloading and stream-hashing an artifact to a temporary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Abstract upstream transport so all parsing logic can be tested offline.
pub trait ToolchainSource {
    /// Fetches a UTF-8 text document over HTTPS.
    fn get_text(&self, url: &str) -> Result<String, String>;
    /// Queries `Content-Length` via HTTP HEAD over HTTPS.
    fn head_size(&self, url: &str) -> Result<u64, String>;
    /// Streams an artifact to `dest`, computing its SHA-256 and size.
    fn download_to_file(&self, url: &str, dest: &Path) -> Result<DownloadedFile, String>;
    /// Total bytes downloaded through `download_to_file` so far.
    fn total_bytes_downloaded(&self) -> u64;
}

/// Rejects any URL that is not HTTPS.
pub fn require_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("refusing non-HTTPS URL {url:?}"))
    }
}

/// Formats bytes as lowercase hex.
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Checks available disk space on the filesystem containing `dir`.
#[cfg(unix)]
pub fn check_free_disk_bytes(dir: &Path, min_bytes: u64) -> Result<u64, String> {
    let output = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .map_err(|err| format!("failed to run df on {}: {err}", dir.display()))?;
    if !output.status.success() {
        return Err(format!("df exited with {}", output.status));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let data_line = stdout
        .lines()
        .nth(1)
        .ok_or_else(|| "unexpected df output (missing data line)".to_owned())?;
    let avail_kb: u64 = data_line
        .split_whitespace()
        .nth(3)
        .ok_or_else(|| format!("unexpected df line: {data_line}"))?
        .parse()
        .map_err(|err| format!("invalid df available KB: {err}"))?;
    let avail_bytes = avail_kb.saturating_mul(1024);
    if avail_bytes < min_bytes {
        return Err(format!(
            "insufficient free disk space in {}: {} MiB available, need at least {} MiB",
            dir.display(),
            avail_bytes / (1024 * 1024),
            min_bytes / (1024 * 1024)
        ));
    }
    Ok(avail_bytes)
}

#[cfg(not(unix))]
pub fn check_free_disk_bytes(_dir: &Path, min_bytes: u64) -> Result<u64, String> {
    Ok(min_bytes)
}

/// Live HTTPS transport with retries, backoff, size caps, and disk checks.
pub struct LiveHttpSource {
    client: reqwest::blocking::Client,
    total_bytes: AtomicU64,
}

impl LiveHttpSource {
    /// Creates an HTTPS-only blocking HTTP client.
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(180))
            .connect_timeout(Duration::from_secs(20))
            .user_agent(USER_AGENT)
            .https_only(true)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() != "https" {
                    let scheme = attempt.url().scheme().to_owned();
                    return attempt.error(format!("refusing redirect to non-HTTPS {scheme}"));
                }
                if attempt.previous().len() > MAX_REDIRECTS {
                    return attempt.error(format!("exceeded {MAX_REDIRECTS} redirects"));
                }
                attempt.follow()
            }))
            .build()
            .map_err(|err| format!("failed to build HTTP client: {err}"))?;
        Ok(Self {
            client,
            total_bytes: AtomicU64::new(0),
        })
    }

    fn send_with_retry(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::blocking::Response, String> {
        require_https(url)?;
        let mut last_err = String::new();
        for attempt in 1..=MAX_ATTEMPTS {
            let result = self
                .client
                .request(method.clone(), url)
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .send();
            match result {
                Ok(resp) if resp.status().is_success() => return Ok(resp),
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    last_err = format!("{method} {url} returned HTTP {code}");
                    let retryable = code == 429 || code >= 500;
                    if !retryable || attempt == MAX_ATTEMPTS {
                        return Err(last_err);
                    }
                }
                Err(err) => {
                    last_err = format!("{method} {url} failed: {err}");
                    if attempt == MAX_ATTEMPTS {
                        return Err(last_err);
                    }
                }
            }
            let backoff = Duration::from_secs(1u64 << (attempt - 1));
            std::thread::sleep(backoff);
        }
        Err(last_err)
    }
}

impl ToolchainSource for LiveHttpSource {
    fn get_text(&self, url: &str) -> Result<String, String> {
        let resp = self.send_with_retry(reqwest::Method::GET, url)?;
        resp.text()
            .map_err(|err| format!("GET {url} body failed: {err}"))
    }

    fn head_size(&self, url: &str) -> Result<u64, String> {
        let resp = self.send_with_retry(reqwest::Method::HEAD, url)?;
        resp.headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&len| len > 0)
            .ok_or_else(|| format!("HEAD {url} returned no valid Content-Length"))
    }

    fn download_to_file(&self, url: &str, dest: &Path) -> Result<DownloadedFile, String> {
        require_https(url)?;
        let parent = dest
            .parent()
            .ok_or_else(|| format!("invalid download path {}", dest.display()))?;
        check_free_disk_bytes(parent, MIN_FREE_DISK_BYTES)?;

        let mut resp = self.send_with_retry(reqwest::Method::GET, url)?;
        if let Some(cl) = resp.content_length()
            && cl > MAX_ARTIFACT_BYTES
        {
            return Err(format!(
                "artifact {url} size {cl} exceeds limit {MAX_ARTIFACT_BYTES}"
            ));
        }

        let mut file =
            File::create(dest).map_err(|err| format!("cannot create {}: {err}", dest.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;

        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|err| format!("read error downloading {url}: {err}"))?;
            if n == 0 {
                break;
            }
            written = written.saturating_add(n as u64);
            if written > MAX_ARTIFACT_BYTES {
                let _ = std::fs::remove_file(dest);
                return Err(format!(
                    "artifact {url} exceeded per-artifact cap of {MAX_ARTIFACT_BYTES} bytes"
                ));
            }
            let total = self.total_bytes.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
            if total > MAX_TOTAL_DOWNLOAD_BYTES {
                let _ = std::fs::remove_file(dest);
                return Err(format!(
                    "cumulative download size exceeded cap of {MAX_TOTAL_DOWNLOAD_BYTES} bytes"
                ));
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .map_err(|err| format!("write error on {}: {err}", dest.display()))?;
        }
        file.flush()
            .map_err(|err| format!("flush error on {}: {err}", dest.display()))?;
        if written == 0 {
            let _ = std::fs::remove_file(dest);
            return Err(format!("artifact {url} was empty"));
        }

        Ok(DownloadedFile {
            path: dest.to_path_buf(),
            sha256: hex_lower(&hasher.finalize()),
            size_bytes: written,
        })
    }

    fn total_bytes_downloaded(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }
}

/// Offline fixture source backed by in-memory maps (used by tests and
/// `--offline-fixtures`).
#[derive(Default)]
pub struct MemorySource {
    pub texts: BTreeMap<String, String>,
    pub head_sizes: BTreeMap<String, u64>,
    pub files: BTreeMap<String, Vec<u8>>,
    total_bytes: AtomicU64,
}

impl MemorySource {
    /// Loads a standard offline fixture directory (matching `tests/fixtures/toolchains`).
    pub fn from_fixture_dir(dir: &Path) -> Result<Self, String> {
        let mut src = Self::default();
        let load = |name: &str| -> Result<String, String> {
            std::fs::read_to_string(dir.join(name))
                .map_err(|err| format!("cannot read fixture {name}: {err}"))
        };
        src.texts.insert(
            super::php_linux::COMMON_INDEX_URL.to_owned(),
            load("static-php-common.json")?,
        );
        src.texts.insert(
            super::php_linux::BULK_INDEX_URL.to_owned(),
            load("static-php-bulk.json")?,
        );
        src.texts.insert(
            super::php_windows::RELEASES_JSON_URL.to_owned(),
            load("windows-php-releases.json")?,
        );
        src.texts.insert(
            super::node::INDEX_JSON_URL.to_owned(),
            load("node-index.json")?,
        );
        src.texts.insert(
            super::composer::VERSIONS_JSON_URL.to_owned(),
            load("composer-versions.json")?,
        );

        for (ver, file) in [
            ("26.13.1", "node-shasums-26.13.1.txt"),
            ("24.22.3", "node-shasums-24.22.3.txt"),
            ("22.23.5", "node-shasums-22.23.5.txt"),
        ] {
            if dir.join(file).exists() {
                src.texts.insert(
                    format!("https://nodejs.org/dist/v{ver}/SHASUMS256.txt"),
                    load(file)?,
                );
            }
        }
        if dir.join("composer-2.11.3.sha256sum").exists() {
            src.texts.insert(
                "https://getcomposer.org/download/2.11.3/composer.phar.sha256sum".to_owned(),
                load("composer-2.11.3.sha256sum")?,
            );
        }
        Ok(src)
    }
}

impl ToolchainSource for MemorySource {
    fn get_text(&self, url: &str) -> Result<String, String> {
        require_https(url)?;
        self.texts
            .get(url)
            .cloned()
            .ok_or_else(|| format!("offline fixture has no text for {url}"))
    }

    fn head_size(&self, url: &str) -> Result<u64, String> {
        require_https(url)?;
        self.head_sizes
            .get(url)
            .copied()
            .ok_or_else(|| format!("offline fixture has no HEAD size for {url}"))
    }

    fn download_to_file(&self, url: &str, dest: &Path) -> Result<DownloadedFile, String> {
        require_https(url)?;
        let bytes = self
            .files
            .get(url)
            .ok_or_else(|| format!("offline fixture has no bytes for {url}"))?;
        std::fs::write(dest, bytes)
            .map_err(|err| format!("cannot write {}: {err}", dest.display()))?;
        let hash = hex_lower(&Sha256::digest(bytes));
        let size = bytes.len() as u64;
        self.total_bytes.fetch_add(size, Ordering::Relaxed);
        Ok(DownloadedFile {
            path: dest.to_path_buf(),
            sha256: hash,
            size_bytes: size,
        })
    }

    fn total_bytes_downloaded(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }
}
