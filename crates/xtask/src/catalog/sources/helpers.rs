//! Shared parsing, hashing and assembly helpers.

use super::{
    CATALOG_SCHEMA_VERSION, CATALOG_SOURCE_URL, CatalogArtifact, CatalogDocument, CatalogVersion,
    SourceError,
};
use dblocal_core::model::ArchiveFormat;

/// Parses a `"<hex>  <filename>"` sidecar and checks the filename matches.
pub fn parse_sha256_sidecar(text: &str, expected_name: &str) -> Result<String, SourceError> {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| SourceError::new(format!("empty checksum sidecar for {expected_name}")))?;
    let mut fields = line.split_whitespace();
    let hash = fields
        .next()
        .ok_or_else(|| SourceError::new("checksum sidecar has no hash"))?;
    if !is_sha256_hex(hash) {
        return Err(SourceError::new(format!(
            "checksum sidecar for {expected_name} is not a SHA-256: {hash:?}"
        )));
    }
    if let Some(name) = fields.next() {
        let name = name.trim_start_matches('*');
        if name != expected_name {
            return Err(SourceError::new(format!(
                "checksum sidecar names {name:?}, expected {expected_name:?}"
            )));
        }
    }
    Ok(hash.to_ascii_lowercase())
}

/// Parses a `"<md5>  <filename>"` sidecar.
pub fn parse_md5_sidecar(text: &str, expected_name: &str) -> Result<String, SourceError> {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| SourceError::new(format!("empty md5 sidecar for {expected_name}")))?;
    let mut fields = line.split_whitespace();
    let hash = fields
        .next()
        .ok_or_else(|| SourceError::new("md5 sidecar has no hash"))?;
    let valid = hash.len() == 32 && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !valid {
        return Err(SourceError::new(format!(
            "md5 sidecar for {expected_name} is not an MD5: {hash:?}"
        )));
    }
    if let Some(name) = fields.next() {
        let name = name.trim_start_matches('*');
        if name != expected_name {
            return Err(SourceError::new(format!(
                "md5 sidecar names {name:?}, expected {expected_name:?}"
            )));
        }
    }
    Ok(hash.to_ascii_lowercase())
}

/// Renders bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Whether `value` is exactly 64 lowercase hex characters.
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Rejects anything but an absolute HTTPS URL.
pub fn require_https(url: &str) -> Result<(), SourceError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(SourceError::new(format!(
            "refusing non-HTTPS URL {url:?} (AGENTS.md invariant 6)"
        )))
    }
}

/// Parses an archive format from a file name.
pub fn archive_format(file_name: &str) -> Option<ArchiveFormat> {
    if file_name.ends_with(".tar.gz") {
        Some(ArchiveFormat::TarGz)
    } else if file_name.ends_with(".tar.xz") {
        Some(ArchiveFormat::TarXz)
    } else if file_name.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else {
        None
    }
}

/// The single top-level directory an engine archive unpacks into.
///
/// Every PostgreSQL, MySQL and MariaDB build names its one root after the
/// archive stem (`mysql-8.4.11-linux-glibc2.28-x86_64-minimal/`,
/// `mariadb-11.8.10-winx64/`, ...). Using anything else as `strip_prefix`
/// leaves `bin/` one level too deep and the extraction's expected-binary check
/// fails after a real download, so the prefix is always derived from the name.
pub fn archive_root(file_name: &str) -> Option<String> {
    for suffix in [".tar.xz", ".tar.gz", ".zip"] {
        if let Some(stem) = file_name.strip_suffix(suffix) {
            return Some(stem.to_owned());
        }
    }
    None
}

/// Assembles and sorts the final document deterministically.
pub fn assemble(
    mut versions: Vec<CatalogVersion>,
    mut artifacts: Vec<CatalogArtifact>,
    generated_at: Option<String>,
) -> CatalogDocument {
    versions.sort_by(|left, right| {
        left.engine
            .cmp(&right.engine)
            .then_with(|| version_rank(&right.version).cmp(&version_rank(&left.version)))
    });
    artifacts.sort_by(|left, right| {
        left.engine
            .cmp(&right.engine)
            .then_with(|| version_rank(&right.version).cmp(&version_rank(&left.version)))
            .then_with(|| left.os.cmp(&right.os))
            .then_with(|| left.arch.cmp(&right.arch))
            .then_with(|| left.id.cmp(&right.id))
    });
    CatalogDocument {
        schema_version: CATALOG_SCHEMA_VERSION,
        generated_at,
        source_url: CATALOG_SOURCE_URL.to_owned(),
        versions,
        artifacts,
    }
}

/// Sort key for newest-first semver ordering.
pub fn version_rank(tag: &str) -> (u64, u64, u64, String) {
    let mut parts = tag.split('.');
    let major = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    (major, minor, patch, tag.to_owned())
}
