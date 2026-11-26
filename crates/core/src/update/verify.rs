//! Integrity verification seam for update artifacts.
//!
//! All release verification goes through [`ReleaseVerifier`]. Today the
//! production verifier ([`Sha256ReleaseVerifier`]) validates `checksums.txt`
//! syntax and verifies the downloaded artifact's SHA-256 in constant time,
//! deleting the file on mismatch.
//!
//! Keeping manifest verification behind [`ReleaseVerifier::verify_manifest`]
//! allows a future `Ed25519ReleaseVerifier` to require and check a detached
//! `checksums.txt.sig` signature over `checksums.txt` without changing any
//! caller above this module.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::error::{UpdateError, UpdateResult};

/// Verification seam for release manifests (`checksums.txt`) and downloaded
/// artifacts.
pub trait ReleaseVerifier: Send + Sync {
    /// Verifies and parses `checksums.txt`.
    ///
    /// `signature_bytes` is reserved for future detached signature verification
    /// (e.g. Ed25519 over `checksums.txt`).
    fn verify_manifest(
        &self,
        checksums_bytes: &[u8],
        signature_bytes: Option<&[u8]>,
    ) -> UpdateResult<Checksums>;

    /// Verifies that the downloaded file at `path` matches `expected_hex`.
    /// Implementations must delete `path` on mismatch.
    fn verify_artifact<'a>(
        &'a self,
        path: &'a Path,
        expected_hex: &'a str,
        label: &'a str,
    ) -> Pin<Box<dyn Future<Output = UpdateResult<()>> + Send + 'a>>;
}

/// Default release verifier: parses `checksums.txt` and checks SHA-256 digests.
#[derive(Debug, Default, Clone, Copy)]
pub struct Sha256ReleaseVerifier;

impl ReleaseVerifier for Sha256ReleaseVerifier {
    fn verify_manifest(
        &self,
        checksums_bytes: &[u8],
        _signature_bytes: Option<&[u8]>,
    ) -> UpdateResult<Checksums> {
        let text =
            std::str::from_utf8(checksums_bytes).map_err(|_| UpdateError::ChecksumMetadata {
                reason: "checksums.txt is not valid UTF-8".to_owned(),
            })?;
        Checksums::parse(text)
    }

    fn verify_artifact<'a>(
        &'a self,
        path: &'a Path,
        expected_hex: &'a str,
        label: &'a str,
    ) -> Pin<Box<dyn Future<Output = UpdateResult<()>> + Send + 'a>> {
        Box::pin(verify_sha256_file(path, expected_hex, label))
    }
}

/// Parsed `checksums.txt` entries: file name -> lowercase hex sha256.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checksums {
    entries: Vec<(String, String)>,
}

impl Checksums {
    /// Parses a checksums.txt body.
    ///
    /// Accepted formats per line: `<sha256>  <filename>` or `<sha256> <filename>`.
    /// Empty lines and lines starting with `#` are ignored.
    pub fn parse(body: &str) -> UpdateResult<Self> {
        let mut entries = Vec::new();
        for (idx, raw) in body.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (hash, name) =
                parse_checksum_line(line).ok_or_else(|| UpdateError::ChecksumMetadata {
                    reason: format!("invalid line in checksums.txt at line {}", idx + 1),
                })?;
            if entries.iter().any(|(existing, _)| existing == name) {
                return Err(UpdateError::ChecksumMetadata {
                    reason: format!("duplicate checksums.txt entry for {name}"),
                });
            }
            entries.push((name.to_owned(), hash));
        }
        if entries.is_empty() {
            return Err(UpdateError::ChecksumMetadata {
                reason: "checksums.txt is empty or has no valid entries".to_owned(),
            });
        }
        Ok(Self { entries })
    }

    /// Returns the sha256 for `asset_name`, if present.
    pub fn digest_for(&self, asset_name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(name, _)| name == asset_name)
            .map(|(_, hash)| hash.as_str())
    }

    /// Number of parsed entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entries were parsed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn parse_checksum_line(line: &str) -> Option<(String, &str)> {
    let mut parts = line.split_whitespace();
    let hash = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let name = name.strip_prefix('*').unwrap_or(name);
    if !is_sha256_hex(hash) {
        return None;
    }
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    Some((hash.to_ascii_lowercase(), name))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Computes SHA-256 of a file as lowercase hex.
pub async fn sha256_file_hex(path: &Path) -> UpdateResult<String> {
    let mut file =
        tokio::fs::File::open(path)
            .await
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not read the file for verification: {source}"),
            })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not read the file for verification: {source}"),
            })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Verifies `path` has the expected sha256. On mismatch, deletes `path`.
pub async fn verify_sha256_file(path: &Path, expected_hex: &str, label: &str) -> UpdateResult<()> {
    let actual = sha256_file_hex(path).await?;
    if constant_time_eq_hex(expected_hex, &actual) {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(path).await;
    Err(UpdateError::VerifyFailed {
        reason: format!("checksum mismatch for {label}: expected {expected_hex}, got {actual}"),
    })
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Constant-time comparison for equal-length hex digests.
pub fn constant_time_eq_hex(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
#[path = "verify/tests.rs"]
mod tests;
