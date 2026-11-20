//! SHA-256 verification helpers.
//!
//! Hashing is streaming (chunked) so large archives are never loaded whole into
//! memory. On a resumable download the caller hashes the existing `.part` prefix
//! before requesting more bytes, then continues the same hasher over the newly
//! received chunks.

use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{DblError, Result};

/// Hex-encoded SHA-256 digest (64 lowercase characters).
pub type Sha256Hex = String;

/// Hashes a file at `path` and returns its lowercase hex digest.
pub async fn sha256_file(path: &Path) -> Result<Sha256Hex> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|source| DblError::Io { source })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_SIZE];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|source| DblError::Io { source })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Hashes a byte slice (used by tests to build fixtures and expectations).
#[cfg(test)]
pub fn sha256_bytes(data: &[u8]) -> Sha256Hex {
    let mut hasher = Sha256::new();
    hasher.update(data);
    to_hex(&hasher.finalize())
}

/// A streaming hasher fed chunk by chunk while downloading.
#[derive(Clone)]
pub struct StreamingHasher {
    inner: Sha256,
}

impl StreamingHasher {
    /// Creates a fresh, empty hasher.
    pub fn new() -> Self {
        Self {
            inner: Sha256::new(),
        }
    }

    /// Feeds more bytes into the digest.
    pub fn update(&mut self, chunk: &[u8]) {
        self.inner.update(chunk);
    }

    /// Finalizes and returns the lowercase hex digest without consuming the
    /// ability to keep a copy? (Digest::finalize consumes; we clone before).
    pub fn finalize_hex(&self) -> Sha256Hex {
        to_hex(&self.inner.clone().finalize())
    }
}

impl Default for StreamingHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Hashes the first `len` bytes of `path`, returning the hasher positioned at
/// that offset so the caller can continue feeding newly downloaded chunks.
///
/// `len` must not exceed the file's size; if it does, the whole file is hashed
/// and the mismatch will be caught at the end.
pub async fn hash_prefix(path: &Path, len: u64) -> Result<StreamingHasher> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|source| DblError::Io { source })?;
    let mut hasher = StreamingHasher::new();
    let mut remaining = len;
    let mut buf = vec![0u8; HASH_BUF_SIZE];
    while remaining > 0 {
        let to_read = std::cmp::min(remaining, buf.len() as u64) as usize;
        let n = file
            .read(&mut buf[..to_read])
            .await
            .map_err(|source| DblError::Io { source })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        remaining = remaining.saturating_sub(n as u64);
    }
    Ok(hasher)
}

/// Returns `Ok(())` when `actual_hex` matches `expected_hex`, otherwise deletes
/// `part_path` and returns [`DblError::ChecksumMismatch`] whose display names both
/// digests.
pub async fn verify_or_reject(
    artifact_label: &str,
    expected_hex: &str,
    actual_hex: &str,
    part_path: &Path,
) -> Result<()> {
    if constant_time_eq_hex(expected_hex, actual_hex) {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(part_path).await;
    let mut sidecar = part_path.as_os_str().to_os_string();
    sidecar.push(".etag");
    let _ = tokio::fs::remove_file(std::path::PathBuf::from(sidecar)).await;
    Err(DblError::ChecksumMismatch {
        artifact: format!("{artifact_label} (expected sha256 {expected_hex}, got {actual_hex})"),
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

/// Constant-time-ish comparison for equal-length hex strings (avoids early exit
/// on the first differing nibble; both inputs are digests, not secrets, but this
/// is cheap and tidy).
fn constant_time_eq_hex(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

const HASH_BUF_SIZE: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn hashes_file_and_bytes_consistently() {
        let dir = tempdir().expect("tmp");
        let p = dir.path().join("x.bin");
        tokio::fs::write(&p, b"hello").await.expect("write");
        let a = sha256_file(&p).await.expect("hash");
        let b = sha256_bytes(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[tokio::test]
    async fn prefix_hash_can_continue() {
        let dir = tempdir().expect("tmp");
        let p = dir.path().join("x.bin");
        let data = b"hello world";
        tokio::fs::write(&p, data).await.expect("write");
        let mut h = hash_prefix(&p, 6).await.expect("prefix"); // "hello "
        h.update(&data[6..]);
        assert_eq!(h.finalize_hex(), sha256_bytes(data));
    }
}
