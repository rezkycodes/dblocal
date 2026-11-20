//! Resumable HTTPS download with progress, retries and cancellation.

mod guard;
mod session;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::model::Artifact;
use crate::model::DownloadProgress;
use crate::paths::DblPaths;
use crate::{DblError, Result};

/// Directory name under the DBLocal root that holds `.part` downloads.
///
/// `DblPaths` does not currently expose a downloads directory; this module owns
/// the layout choice and keeps it inside the DBLocal root so it shares the same
/// filesystem as runtimes (required for predictable disk accounting).
pub const DOWNLOADS_DIR_NAME: &str = "downloads";

/// Maximum number of HTTP redirects followed for a download.
pub const MAX_REDIRECTS: usize = 5;
/// Maximum retry attempts for transient network failures (in addition to the
/// initial attempt). Total attempts = 1 + MAX_RETRIES.
pub const MAX_RETRIES: u32 = 5;
/// Initial backoff delay between retries.
pub const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
/// Maximum backoff delay between retries.
pub const MAX_BACKOFF: Duration = Duration::from_secs(8);
/// TCP connect timeout for the download client.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-response idle/read timeout; generous because large archives stream for a
/// long time but any single stalled read should not hang forever.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Cooperative cancellation flag shared with the caller.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Creates an unset token.
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Returns `true` once cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// URL policy enforced for artifact downloads.
///
/// Production code must use [`UrlPolicy::HttpsOnly`]. Loopback plain HTTP is
/// available only to in-crate tests so they can run a local server on port 0
/// without TLS infrastructure.
#[derive(Clone, Debug)]
pub enum UrlPolicy {
    /// Require `https://` for the initial URL and for every redirect target.
    /// This is the only policy constructible outside `#[cfg(test)]`.
    HttpsOnly,
    /// Allow plain `http://` only when the host is a loopback address.
    /// Constructible only from test builds of this crate.
    #[cfg(test)]
    AllowLoopbackHttp(TestOnlyLoopback),
}

/// Proof token that restricts [`UrlPolicy::AllowLoopbackHttp`] to tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct TestOnlyLoopback(());

#[cfg(test)]
impl TestOnlyLoopback {
    /// Creates the test-only loopback permission.
    pub fn new() -> Self {
        Self(())
    }
}

impl UrlPolicy {
    /// The production policy.
    pub fn https_only() -> Self {
        Self::HttpsOnly
    }

    #[cfg(test)]
    /// The test-only policy permitting `http://127.0.0.1`, `http://[::1]` and
    /// `http://localhost`.
    pub fn allow_loopback_http_for_tests() -> Self {
        Self::AllowLoopbackHttp(TestOnlyLoopback::new())
    }

    fn validate_url(&self, url: &reqwest::Url) -> Result<()> {
        match self {
            Self::HttpsOnly => {
                if url.scheme() != "https" {
                    return Err(DblError::Network {
                        reason: format!("refusing non-HTTPS download URL: {}", redact_url(url)),
                    });
                }
                Ok(())
            }
            #[cfg(test)]
            Self::AllowLoopbackHttp(_) => {
                let ok = match url.scheme() {
                    "https" => true,
                    "http" => is_loopback_host(url),
                    _ => false,
                };
                if ok {
                    Ok(())
                } else {
                    Err(DblError::Network {
                        reason: format!(
                            "refusing non-loopback HTTP download URL in test mode: {}",
                            redact_url(url)
                        ),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
fn is_loopback_host(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn redact_url(url: &reqwest::Url) -> String {
    // Strip query/fragment/userinfo so diagnostics never leak tokens.
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(url.path());
    out
}

/// Events emitted during a download.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// Periodic progress sample (bytes, total, speed, ETA).
    Progress(DownloadProgress),
    /// The server ignored `Range` or the stored validator no longer matched, so
    /// the transfer restarted from byte 0.
    Restarted {
        /// Human-readable reason (for logs/UI).
        reason: String,
    },
}

/// Sink receiving [`DownloadEvent`]s. Must be cheap and non-blocking.
pub type ProgressSink = Arc<dyn Fn(DownloadEvent) + Send + Sync>;

/// Options controlling the HTTP download phase.
#[derive(Clone, Debug)]
pub struct DownloadOptions {
    /// URL policy (HTTPS-only in production).
    pub policy: UrlPolicy,
    /// Cooperative cancellation flag.
    pub cancel: CancellationToken,
    /// Maximum retry attempts for transient failures.
    pub max_retries: u32,
    /// Initial retry backoff.
    pub initial_backoff: Duration,
    /// Maximum retry backoff.
    pub max_backoff: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            policy: UrlPolicy::HttpsOnly,
            cancel: CancellationToken::new(),
            max_retries: MAX_RETRIES,
            initial_backoff: INITIAL_BACKOFF,
            max_backoff: MAX_BACKOFF,
        }
    }
}

impl DownloadOptions {
    /// Returns a copy using `policy`.
    pub fn with_policy(mut self, policy: UrlPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Returns a copy using `token` for cancellation.
    pub fn with_cancel(mut self, token: CancellationToken) -> Self {
        self.cancel = token;
        self
    }
}

/// Computes the downloads directory path under the DBLocal root.
pub fn downloads_dir(paths: &DblPaths) -> PathBuf {
    paths.root().join(DOWNLOADS_DIR_NAME)
}

/// Computes the `.part` file path for an arbitrary label.
pub fn part_path_for_label(downloads_dir: &Path, label: &str) -> PathBuf {
    downloads_dir.join(format!("{}.part", sanitize_file_name(label)))
}

/// Sidecar file storing the ETag/validator for resumable downloads.
pub fn etag_sidecar_path(part_path: &Path) -> PathBuf {
    let mut s = part_path.as_os_str().to_os_string();
    s.push(".etag");
    PathBuf::from(s)
}

fn sanitize_file_name(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "artifact".to_owned()
    } else {
        out
    }
}

// --- transfer session --------------------------------------------------------

/// Downloads `artifact` to `part_path`, resuming when possible, verifying the
/// SHA-256 against `artifact.sha256()` before returning.
///
/// On checksum mismatch the `.part` is deleted and [`DblError::ChecksumMismatch`]
/// is returned. On cancellation the `.part` is deleted and
/// [`DblError::Cancelled`] is returned.
pub async fn download_artifact(
    artifact: &Artifact,
    part_path: &Path,
    options: &DownloadOptions,
    progress: Option<ProgressSink>,
) -> Result<()> {
    session::download_artifact_impl(artifact, part_path, options, progress).await
}

/// Core download routine, parameterised by everything it needs rather than by
/// an [`Artifact`] so it can be exercised over loopback HTTP in tests while
/// [`download_artifact`] remains the HTTPS-only production entry point.
pub async fn download_from_url(
    url: &reqwest::Url,
    label: &str,
    expected_sha256: &str,
    expected_size: u64,
    part_path: &Path,
    options: &DownloadOptions,
    progress: Option<ProgressSink>,
) -> Result<()> {
    session::download_from_url_impl(
        url,
        label,
        expected_sha256,
        expected_size,
        part_path,
        options,
        progress,
    )
    .await
}

#[cfg(test)]
pub(crate) fn test_validate(policy: &UrlPolicy, url: &reqwest::Url) -> Result<()> {
    policy.validate_url(url)
}
