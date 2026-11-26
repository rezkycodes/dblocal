//! Integrity and Ed25519 signature verification seam for update artifacts.
//!
//! All release verification goes through [`ReleaseVerifier`]. In production,
//! [`Ed25519ReleaseVerifier`] verifies a detached 64-byte Ed25519 signature
//! (`checksums.txt.sig`) over `checksums.txt` against [`TRUSTED_PUBLIC_KEYS`]
//! **before** parsing any SHA-256 digest. It fails closed: missing, malformed,
//! or unverified signatures — or an empty trusted-key list — reject the update
//! before any artifact is extracted or installed.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::OnceLock;

use ring::signature::{ED25519, UnparsedPublicKey};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::error::{UpdateError, UpdateResult};

/// Expected raw byte length of an Ed25519 public key.
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;
/// Expected raw byte length of an Ed25519 signature (`checksums.txt.sig`).
pub const ED25519_SIGNATURE_LEN: usize = 64;

/// Single source of truth for trusted Ed25519 release-signing public keys,
/// embedded directly from `packaging/release-signing/public-keys.txt`.
///
/// Both CI (`.github/workflows/release.yml`) and the compiled Rust binary read
/// this same file so they cannot drift. Multiple keys may be listed to support
/// seamless key rotation. While this file has no keys, release builds fail
/// closed and refuse self-update.
pub const TRUSTED_PUBLIC_KEYS_RAW: &str =
    include_str!("../../../../packaging/release-signing/public-keys.txt");

/// Parses a `public-keys.txt` document into raw 32-byte Ed25519 public keys.
///
/// Format rules (shared with `.github/workflows/release.yml`):
/// - Blank lines and lines whose trimmed form starts with `#` are ignored.
/// - Every other line must be a 64-character lowercase hex string encoding a
///   32-byte Ed25519 public key.
/// - Any malformed line returns an error instead of being silently skipped.
pub fn parse_trusted_public_keys(
    contents: &str,
) -> UpdateResult<Vec<[u8; ED25519_PUBLIC_KEY_LEN]>> {
    let mut keys = Vec::new();
    for (idx, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.len() != ED25519_PUBLIC_KEY_LEN * 2
            || !line.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(UpdateError::VerifyFailed {
                reason: format!(
                    "malformed Ed25519 public key in public-keys.txt at line {}",
                    idx + 1
                ),
            });
        }
        keys.push(decode_hex_32(line)?);
    }
    Ok(keys)
}

fn compiled_trusted_public_keys() -> &'static UpdateResult<Vec<[u8; ED25519_PUBLIC_KEY_LEN]>> {
    static PARSED: OnceLock<UpdateResult<Vec<[u8; ED25519_PUBLIC_KEY_LEN]>>> = OnceLock::new();
    PARSED.get_or_init(|| parse_trusted_public_keys(TRUSTED_PUBLIC_KEYS_RAW))
}

/// Returns true when `packaging/release-signing/public-keys.txt` parses cleanly
/// and contains at least one trusted Ed25519 public key.
pub fn has_trusted_signing_keys() -> bool {
    matches!(compiled_trusted_public_keys(), Ok(keys) if !keys.is_empty())
}

/// Verification seam for release manifests (`checksums.txt` + `checksums.txt.sig`)
/// and downloaded artifacts.
pub trait ReleaseVerifier: Send + Sync {
    /// Verifies and parses `checksums.txt`.
    ///
    /// Production implementations verify `signature_bytes` (Ed25519 over
    /// `checksums_bytes`) before parsing any digest and fail closed on missing
    /// or invalid signatures.
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

/// Production release verifier: requires a valid Ed25519 signature over
/// `checksums.txt` from one of the trusted public keys before parsing digests,
/// then checks the artifact's SHA-256 in constant time.
#[derive(Debug, Clone)]
pub struct Ed25519ReleaseVerifier {
    trusted_keys: Vec<[u8; ED25519_PUBLIC_KEY_LEN]>,
    init_error: Option<String>,
}

impl Ed25519ReleaseVerifier {
    /// Builds a verifier from the compiled-in `packaging/release-signing/public-keys.txt`.
    ///
    /// If the committed key file contains a malformed entry, every verification
    /// call fails closed with that parse error rather than silently skipping it.
    pub fn production() -> Self {
        match compiled_trusted_public_keys() {
            Ok(keys) => Self {
                trusted_keys: keys.clone(),
                init_error: None,
            },
            Err(err) => Self {
                trusted_keys: Vec::new(),
                init_error: Some(err.to_string()),
            },
        }
    }

    /// Builds a verifier from explicit hex-encoded 32-byte Ed25519 public keys.
    pub fn from_hex_keys(keys: &[&str]) -> UpdateResult<Self> {
        let mut decoded = Vec::with_capacity(keys.len());
        for hex in keys {
            decoded.push(decode_hex_32(hex)?);
        }
        Ok(Self {
            trusted_keys: decoded,
            init_error: None,
        })
    }

    /// Builds a verifier from raw 32-byte Ed25519 public keys.
    pub fn from_raw_keys(keys: Vec<[u8; ED25519_PUBLIC_KEY_LEN]>) -> Self {
        Self {
            trusted_keys: keys,
            init_error: None,
        }
    }

    /// Number of trusted public keys loaded.
    pub fn key_count(&self) -> usize {
        self.trusted_keys.len()
    }
}

impl ReleaseVerifier for Ed25519ReleaseVerifier {
    fn verify_manifest(
        &self,
        checksums_bytes: &[u8],
        signature_bytes: Option<&[u8]>,
    ) -> UpdateResult<Checksums> {
        if let Some(reason) = &self.init_error {
            return Err(UpdateError::VerifyFailed {
                reason: reason.clone(),
            });
        }
        if self.trusted_keys.is_empty() {
            return Err(UpdateError::VerifyFailed {
                reason: "no trusted release signing public key is configured in this build"
                    .to_owned(),
            });
        }
        let sig = signature_bytes.ok_or_else(|| UpdateError::VerifyFailed {
            reason: "release is missing checksums.txt.sig signature".to_owned(),
        })?;
        if sig.len() != ED25519_SIGNATURE_LEN {
            return Err(UpdateError::VerifyFailed {
                reason: format!(
                    "malformed release signature: expected {} bytes, got {}",
                    ED25519_SIGNATURE_LEN,
                    sig.len()
                ),
            });
        }
        let verified = self.trusted_keys.iter().any(|key| {
            UnparsedPublicKey::new(&ED25519, key)
                .verify(checksums_bytes, sig)
                .is_ok()
        });
        if !verified {
            return Err(UpdateError::VerifyFailed {
                reason: "release signature verification failed against trusted signing keys"
                    .to_owned(),
            });
        }

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

/// Test-only SHA-256-only verifier (used by unit tests that exercise
/// download/checksum logic without generating a signing keypair).
/// Production always uses [`Ed25519ReleaseVerifier`].
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Sha256ReleaseVerifier;

#[cfg(test)]
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

/// Decodes a 64-character hex string into a 32-byte Ed25519 public key.
pub fn decode_hex_32(hex: &str) -> UpdateResult<[u8; ED25519_PUBLIC_KEY_LEN]> {
    let trimmed = hex.trim();
    if trimmed.len() != ED25519_PUBLIC_KEY_LEN * 2 {
        return Err(UpdateError::VerifyFailed {
            reason: format!(
                "invalid Ed25519 public key hex length: expected {}, got {}",
                ED25519_PUBLIC_KEY_LEN * 2,
                trimmed.len()
            ),
        });
    }
    let mut out = [0u8; ED25519_PUBLIC_KEY_LEN];
    let bytes = trimmed.as_bytes();
    for i in 0..ED25519_PUBLIC_KEY_LEN {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> UpdateResult<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(UpdateError::VerifyFailed {
            reason: "invalid hex character in Ed25519 public key".to_owned(),
        }),
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
