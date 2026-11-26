//! Safe archive extraction for runtime artifacts.
//!
//! Supports `.tar.gz` (`flate2`), `.tar.xz` (`liblzma`) and `.zip` (deflate).
//! Extraction is defensive against zip-slip, symlink escapes, hard links,
//! decompression bombs and path confusion, then staged into a temporary sibling
//! directory and atomically renamed into place only after expected binaries exist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::ArchiveFormat;
use crate::{DblError, Result};

mod deb;
mod fsops;
mod path;
mod tarball;
mod zip;

/// Maximum number of entries extracted from a single archive (bomb/zip-bomb cap).
pub const MAX_EXTRACT_ENTRIES: u64 = 200_000;
/// Multiplier for the maximum total extracted bytes relative to the archive's
/// declared compressed size. Generous to accommodate compression ratios while
/// still bounding runaway expansion.
pub const MAX_EXTRACT_BYTES_MULTIPLIER: u64 = 30;
/// Floor for the extracted-bytes cap so small archives are not over-constrained.
pub const MIN_EXTRACT_BYTES_CAP: u64 = 256 * 1024 * 1024; // 256 MiB
/// Ceiling for the extracted-bytes cap.
pub const MAX_EXTRACT_BYTES_CAP: u64 = 8 * 1024 * 1024 * 1024; // 8 GiB
/// Maximum rename retries on Windows when antivirus/indexers transiently lock
/// the freshly extracted directory.
#[cfg(windows)]
pub const WINDOWS_RENAME_RETRIES: u32 = 20;
/// Initial delay between Windows rename retries.
#[cfg(windows)]
pub const WINDOWS_RENAME_INITIAL_BACKOFF: std::time::Duration =
    std::time::Duration::from_millis(100);
/// Maximum delay between Windows rename retries.
#[cfg(windows)]
pub const WINDOWS_RENAME_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_millis(1000);

/// Options controlling extraction and finalisation.
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// Paths relative to the runtime root that must exist after extraction
    /// (e.g. `bin/postgres`, `bin/valkey-server`).
    pub expected_binaries: Vec<String>,
    /// Directory prefix stripped from every archive entry. Must match exactly.
    pub strip_prefix: Option<String>,
    /// Override for the maximum total extracted bytes (tests can lower this).
    pub max_total_bytes: Option<u64>,
    /// Override for the maximum entry count.
    pub max_entries: Option<u64>,
}

impl ExtractOptions {
    fn max_total_bytes_for_size(&self, artifact_size_bytes: u64) -> u64 {
        if let Some(v) = self.max_total_bytes {
            return v;
        }
        let scaled = artifact_size_bytes.saturating_mul(MAX_EXTRACT_BYTES_MULTIPLIER);
        scaled.clamp(MIN_EXTRACT_BYTES_CAP, MAX_EXTRACT_BYTES_CAP)
    }

    fn max_entries(&self) -> u64 {
        self.max_entries.unwrap_or(MAX_EXTRACT_ENTRIES)
    }
}

/// Extracts the verified archive at `archive_path` into `final_dir`.
///
/// The archive is unpacked into a temporary sibling of `final_dir`, validated,
/// then atomically renamed. On any failure the temporary directory is removed
/// and `final_dir` is never created/left behind by this function.
pub async fn extract_runtime(
    archive_path: &Path,
    format: ArchiveFormat,
    artifact_size_bytes: u64,
    final_dir: &Path,
    options: &ExtractOptions,
) -> Result<PathBuf> {
    // Refuse to overwrite an existing final directory; callers decide whether to
    // skip (already installed) or remove first.
    if tokio::fs::try_exists(final_dir).await.unwrap_or(false) {
        return Err(DblError::AlreadyExists {
            kind: "runtime",
            name: final_dir.display().to_string(),
        });
    }

    let parent = final_dir.parent().ok_or_else(|| DblError::InvalidInput {
        field: "path",
        reason: "runtime directory has no parent".to_owned(),
    })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|source| DblError::Io { source })?;

    let staging = staging_dir_path(final_dir);
    // Ensure a clean slate if a previous attempt left debris.
    if tokio::fs::try_exists(&staging).await.unwrap_or(false) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|source| DblError::Io { source })?;

    let result =
        extract_into_staging(archive_path, format, artifact_size_bytes, &staging, options).await;
    let finalised = async {
        result?;
        verify_expected_binaries(&staging, &options.expected_binaries).await?;
        rename_staging_to_final(&staging, final_dir).await?;
        Ok::<_, crate::DblError>(())
    }
    .await;

    match finalised {
        Ok(()) => Ok(final_dir.to_path_buf()),
        Err(e) => {
            if tokio::fs::try_exists(&staging).await.unwrap_or(false) {
                let _ = tokio::fs::remove_dir_all(&staging).await;
            }
            Err(e)
        }
    }
}

fn staging_dir_path(final_dir: &Path) -> PathBuf {
    let name = final_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("runtime");
    let nonce: u64 = rand::random();
    final_dir.with_file_name(format!(".{name}.extracting-{nonce:016x}"))
}

async fn extract_into_staging(
    archive_path: &Path,
    format: ArchiveFormat,
    artifact_size_bytes: u64,
    staging: &Path,
    options: &ExtractOptions,
) -> Result<()> {
    // Extraction uses synchronous std IO / CPU-heavy decompression; run it on a
    // blocking thread to avoid stalling the async runtime.
    let archive_path = archive_path.to_path_buf();
    let staging = staging.to_path_buf();
    let options = options.clone();
    tokio::task::spawn_blocking(move || {
        extract_into_staging_sync(
            &archive_path,
            format,
            artifact_size_bytes,
            &staging,
            &options,
        )
    })
    .await
    .map_err(|e| DblError::Io {
        source: std::io::Error::other(format!("extraction task failed: {e}")),
    })?
}

fn extract_into_staging_sync(
    archive_path: &Path,
    format: ArchiveFormat,
    artifact_size_bytes: u64,
    staging: &Path,
    options: &ExtractOptions,
) -> Result<()> {
    let caps = ExtractCaps {
        max_total_bytes: options.max_total_bytes_for_size(artifact_size_bytes),
        max_entries: options.max_entries(),
    };
    let strip = options.strip_prefix.as_deref();

    match format {
        ArchiveFormat::TarGz => tarball::extract_tar_gz(archive_path, staging, strip, &caps),
        ArchiveFormat::TarXz => tarball::extract_tar_xz(archive_path, staging, strip, &caps),
        ArchiveFormat::Zip => zip::extract_zip(archive_path, staging, strip, &caps),
        ArchiveFormat::Deb => deb::extract_deb(archive_path, staging, strip, &caps),
    }
}

#[derive(Debug, Clone, Copy)]
struct ExtractCaps {
    max_total_bytes: u64,
    max_entries: u64,
}

#[derive(Debug, Default)]
struct ExtractState {
    total_bytes: u64,
    entries: u64,
    seen_dirs: HashSet<PathBuf>,
    stripped_any: bool,
    /// Canonicalized relative paths of regular files extracted so far (after
    /// strip_prefix). Hard links may only target these.
    regular_files: HashSet<PathBuf>,
}

impl ExtractState {
    fn count_entry(&mut self, caps: &ExtractCaps) -> Result<()> {
        self.entries = self.entries.saturating_add(1);
        if self.entries > caps.max_entries {
            return Err(DblError::UnsafeArchive {
                entry: format!("archive exceeds maximum entry count ({})", caps.max_entries),
            });
        }
        Ok(())
    }

    fn add_bytes(&mut self, n: u64, caps: &ExtractCaps, entry: &str) -> Result<()> {
        self.total_bytes = self.total_bytes.saturating_add(n);
        if self.total_bytes > caps.max_total_bytes {
            return Err(DblError::UnsafeArchive {
                entry: format!(
                    "{entry}: extracted data exceeds maximum size ({} bytes)",
                    caps.max_total_bytes
                ),
            });
        }
        Ok(())
    }
}

async fn verify_expected_binaries(staging: &Path, expected: &[String]) -> Result<()> {
    for rel in expected {
        // Reject an absolute path in any syntax, not just the host's: on
        // Windows `Path::new("/bin/app").is_absolute()` is false (a bare root is
        // drive-relative), so a POSIX-absolute entry would pass and then resolve
        // outside the staging tree. See `path::is_absolute_like`.
        if path::is_absolute_like(rel) {
            return Err(DblError::InvalidInput {
                field: "expected_binaries",
                reason: format!("expected binary path must be relative: {rel}"),
            });
        }
        let rel_path = Path::new(rel);
        let full = staging.join(rel_path);
        let meta = tokio::fs::symlink_metadata(&full).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DblError::InvalidInput {
                    field: "archive",
                    reason: format!("extracted runtime is missing expected binary: {rel}"),
                }
            } else {
                DblError::Io { source: e }
            }
        })?;
        if !meta.file_type().is_file() && !meta.file_type().is_symlink() {
            return Err(DblError::InvalidInput {
                field: "archive",
                reason: format!("expected binary is not a file: {rel}"),
            });
        }
    }
    Ok(())
}

async fn rename_staging_to_final(staging: &Path, final_dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        rename_with_antivirus_retry(staging, final_dir).await
    }
    #[cfg(not(windows))]
    {
        tokio::fs::rename(staging, final_dir)
            .await
            .map_err(|source| DblError::Io { source })
    }
}

#[cfg(windows)]
async fn rename_with_antivirus_retry(staging: &Path, final_dir: &Path) -> Result<()> {
    let mut attempt: u32 = 0;
    let mut delay = WINDOWS_RENAME_INITIAL_BACKOFF;
    loop {
        match tokio::fs::rename(staging, final_dir).await {
            Ok(()) => return Ok(()),
            Err(e) if is_windows_lock_error(&e) && attempt < WINDOWS_RENAME_RETRIES => {
                attempt += 1;
                tracing::warn!(
                    attempt,
                    max = WINDOWS_RENAME_RETRIES,
                    delay_ms = delay.as_millis() as u64,
                    error = %e,
                    "rename locked, retrying (possible antivirus/indexer interference)"
                );
                tokio::time::sleep(delay).await;
                delay = std::cmp::min(delay.saturating_mul(2), WINDOWS_RENAME_MAX_BACKOFF);
            }
            Err(e) if is_windows_lock_error(&e) => {
                return Err(DblError::Io {
                    source: std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to finalize runtime directory after {} attempts: {e}. \
                             This may be caused by antivirus or indexing software locking \
                             newly extracted files; try allowing the DBLocal directory and \
                             installing again.",
                            WINDOWS_RENAME_RETRIES
                        ),
                    ),
                });
            }
            Err(source) => return Err(DblError::Io { source }),
        }
    }
}

#[cfg(windows)]
fn is_windows_lock_error(e: &std::io::Error) -> bool {
    // ERROR_SHARING_VIOLATION (32), ERROR_ACCESS_DENIED (5), ERROR_LOCK_VIOLATION (33)
    matches!(e.raw_os_error(), Some(32) | Some(5) | Some(33))
        || e.kind() == std::io::ErrorKind::PermissionDenied
}
