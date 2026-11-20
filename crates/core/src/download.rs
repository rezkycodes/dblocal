//! Resumable download, SHA-256 verification, and safe extraction.
//!
//! # Ownership
//! Owned by W1-C. Other modules obtain a verified runtime directory by calling
//! [`install_runtime`] (or the lower-level primitives re-exported here); they
//! never download or extract engine archives themselves.
//!
//! # Contract highlights
//! - Downloads are resumable (`Range` + `If-Range`/ETag) and written to a
//!   `<name>.part` file in the downloads directory. A server that ignores
//!   `Range` triggers a clean restart from byte 0.
//! - HTTPS is mandatory in production, including redirects (max 5, never
//!   https -> http). [`UrlPolicy::AllowLoopbackHttp`] exists for tests only and
//!   is gated so production code cannot select it.
//! - SHA-256 is computed while streaming; on resume the existing prefix is
//!   hashed before the request is issued. A mismatch deletes the `.part`, names
//!   the expected and actual digests, and extracts nothing. There is no skip.
//! - Extraction rejects path traversal, absolute/drive/UNC paths, hard links,
//!   and symlinks that would escape the destination, caps total size and entry
//!   count, preserves the Unix executable bit, strips [`Artifact::strip_prefix`]
//!   exactly, and only finalises after the caller's expected binaries exist.
//!   The runtime is assembled in a temporary sibling directory and atomically
//!   renamed into place; any failure leaves no half-installed runtime.
//!
//! # Cancellation
//! Pass a shared [`CancellationToken`] (`Arc<AtomicBool>`). Setting it stops the
//! transfer promptly and the `.part` file is removed.

mod extract;
mod fetch;
mod space;
mod verify;

pub use extract::{ExtractOptions, extract_runtime};
pub use fetch::{
    CancellationToken, DownloadEvent, DownloadOptions, ProgressSink, UrlPolicy, download_artifact,
};
pub use space::{DISK_SPACE_MULTIPLIER, DiskSpace, check_disk_space};
pub use verify::sha256_file;

use std::path::Path;

use crate::Result;
use crate::model::{Artifact, DownloadProgress};
use crate::paths::DblPaths;

/// Everything needed to install one artifact, independent of [`Artifact`] so the
/// pipeline can be driven from a test server over loopback HTTP.
///
/// The URL policy in `options` is the enforcement point for HTTPS; production
/// callers always pass the default [`DownloadOptions`] whose policy is
/// [`UrlPolicy::HttpsOnly`].
#[derive(Debug, Clone, Copy)]
pub struct SourceSpec<'a> {
    /// Absolute download URL.
    pub url: &'a str,
    /// Diagnostics label (typically the artifact id).
    pub label: &'a str,
    /// Expected lowercase hex SHA-256.
    pub sha256: &'a str,
    /// Expected size in bytes, used for the free-space preflight and progress.
    pub size_bytes: u64,
    /// Archive format to extract.
    pub format: crate::model::ArchiveFormat,
    /// Directory prefix stripped from the archive, matched exactly.
    pub strip_prefix: Option<&'a str>,
}

impl<'a> SourceSpec<'a> {
    /// Builds a spec from a catalog [`Artifact`].
    pub fn from_artifact(artifact: &'a Artifact) -> Self {
        Self {
            url: artifact.url(),
            label: artifact.id(),
            sha256: artifact.sha256(),
            size_bytes: artifact.size_bytes(),
            format: artifact.format(),
            strip_prefix: artifact.strip_prefix(),
        }
    }
}

/// Downloads, verifies and extracts `artifact` into
/// `paths.runtime_dir(artifact.engine(), artifact.version(), artifact.arch())`.
///
/// `expected_binaries` are paths relative to the runtime root that must exist
/// after extraction (for example `bin/postgres` or `bin/valkey-server`).
pub async fn install_runtime(
    artifact: &Artifact,
    paths: &DblPaths,
    expected_binaries: &[&str],
    options: InstallOptions,
) -> Result<std::path::PathBuf> {
    let final_dir = paths.runtime_dir(artifact.engine(), artifact.version(), artifact.arch())?;
    let source = SourceSpec::from_artifact(artifact);
    run_install(
        &source,
        &fetch::downloads_dir(paths),
        &final_dir,
        expected_binaries,
        options,
    )
    .await
}

/// Options controlling a full runtime install.
#[derive(Clone, Default)]
pub struct InstallOptions {
    /// Download behaviour (URL policy, retries, cancellation, client tuning).
    pub download: DownloadOptions,
    /// Progress sink; receives samples from the first downloaded byte.
    pub progress: Option<ProgressSink>,
    /// Injectable free-space probe, used by tests to simulate a full disk.
    pub free_space: Option<space::FreeSpaceFn>,
}

impl std::fmt::Debug for InstallOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallOptions")
            .field("download", &self.download)
            .field("progress", &self.progress.as_ref().map(|_| "<sink>"))
            .field("free_space", &self.free_space.as_ref().map(|_| "<probe>"))
            .finish()
    }
}

impl InstallOptions {
    /// Returns a copy that reports progress through `sink`.
    pub fn with_progress(mut self, sink: ProgressSink) -> Self {
        self.progress = Some(sink);
        self
    }

    /// Returns a copy using a custom URL policy (tests only for loopback HTTP).
    pub fn with_policy(mut self, policy: UrlPolicy) -> Self {
        self.download = self.download.with_policy(policy);
        self
    }

    /// Returns a copy that can be cancelled through `token`.
    pub fn with_cancel(mut self, token: CancellationToken) -> Self {
        self.download = self.download.with_cancel(token);
        self
    }

    /// Returns a copy using an injected free-space probe.
    pub fn with_free_space(mut self, probe: space::FreeSpaceFn) -> Self {
        self.free_space = Some(probe);
        self
    }
}

/// Convenience: install into a caller-chosen final directory instead of deriving
/// it from [`DblPaths`]. Used by tests and by callers that already resolved the
/// runtime path.
pub async fn install_runtime_to(
    artifact: &Artifact,
    downloads_dir: &Path,
    final_dir: &Path,
    expected_binaries: &[&str],
    options: InstallOptions,
) -> Result<std::path::PathBuf> {
    let source = SourceSpec::from_artifact(artifact);
    install_source_to(
        &source,
        downloads_dir,
        final_dir,
        expected_binaries,
        options,
    )
    .await
}

/// Same as [`install_runtime_to`] but driven by a [`SourceSpec`].
pub async fn install_source_to(
    source: &SourceSpec<'_>,
    downloads_dir: &Path,
    final_dir: &Path,
    expected_binaries: &[&str],
    options: InstallOptions,
) -> Result<std::path::PathBuf> {
    run_install(source, downloads_dir, final_dir, expected_binaries, options).await
}

/// The single install pipeline: preflight, download+verify, extract, finalise.
///
/// Every public entry point funnels through here so the ordering guarantees
/// (space before bytes, hash before extraction, staging before rename) live in
/// exactly one place.
async fn run_install(
    source: &SourceSpec<'_>,
    downloads_dir: &Path,
    final_dir: &Path,
    expected_binaries: &[&str],
    options: InstallOptions,
) -> Result<std::path::PathBuf> {
    tokio::fs::create_dir_all(downloads_dir)
        .await
        .map_err(|source| crate::DblError::Io { source })?;

    check_disk_space(
        downloads_dir,
        source.size_bytes,
        options.free_space.as_ref(),
    )?;

    let url = reqwest::Url::parse(source.url).map_err(|e| crate::DblError::InvalidInput {
        field: "url",
        reason: format!("invalid URL: {e}"),
    })?;
    let part_path = fetch::part_path_for_label(downloads_dir, source.label);
    fetch::download_from_url(
        &url,
        source.label,
        source.sha256,
        source.size_bytes,
        &part_path,
        &options.download,
        options.progress.clone(),
    )
    .await?;

    let extract_opts = ExtractOptions {
        expected_binaries: expected_binaries.iter().map(|s| (*s).to_owned()).collect(),
        strip_prefix: source.strip_prefix.map(str::to_owned),
        max_total_bytes: None,
        max_entries: None,
    };
    let result = extract_runtime(
        &part_path,
        source.format,
        source.size_bytes,
        final_dir,
        &extract_opts,
    )
    .await;

    // The `.part` is only consumed on a successful extract+rename. On download or
    // extract failure it is kept so the next attempt can resume (checksum
    // mismatch and cancellation already deleted it themselves).
    if result.is_ok() {
        let _ = tokio::fs::remove_file(&part_path).await;
        let _ = tokio::fs::remove_file(fetch::etag_sidecar_path(&part_path)).await;
    }
    result
}

/// Re-export for callers that want to emit the standard progress sample shape.
pub type Progress = DownloadProgress;

#[cfg(test)]
mod tests;
