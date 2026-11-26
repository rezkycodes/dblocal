//! Official Redis APT package resolver (`https://packages.redis.io/deb`).
//!
//! Resolves `redis-tools` `7.4.12-1rl1~jammy1` for `amd64` (`x86_64`) and
//! `arm64` (`aarch64`) from the Ubuntu `jammy` APT index (glibc >= 2.34):
//!
//! 1. Fetches `dists/jammy/InRelease` and parses its `SHA256:` section.
//! 2. Fetches `dists/jammy/main/binary-{amd64,arm64}/Packages.gz` and verifies
//!    that its SHA-256 and byte length match `InRelease`.
//! 3. Decompresses `Packages.gz` and locates the `Package: redis-tools` stanza
//!    matching `7.4.12-1rl1~jammy1` (stripping any Debian epoch prefix like `6:`).
//! 4. Pins the `pool/` HTTPS URL, takes `SHA256` and `Size` from the index, and
//!    cross-checks by downloading and hashing the `.deb` itself — unless an
//!    existing catalog entry already matches `(engine, version, url)` with the
//!    same `sha256` and `size_bytes`.
//!
//! Residual trust assumption: `InRelease` is fetched over HTTPS (`rustls`) and
//! `Packages.gz` + `.deb` are SHA-256 cross-checked against it, without offline
//! OpenPGP signature verification (to avoid adding a heavy OpenPGP dependency).

use std::collections::BTreeMap;
use std::io::Read;

use sha2::{Digest, Sha256};

use super::helpers::{hex, is_sha256_hex, require_https};
use super::tracked::TrackedVersion;
use super::{Arch, CatalogArtifact, CatalogDocument, CatalogVersion, Os, Source, SourceError};

/// Base URL of the official Redis APT repository.
pub const REDIS_APT_BASE: &str = "https://packages.redis.io/deb";
/// Ubuntu distribution codename pinned for glibc >= 2.34 compatibility.
pub const REDIS_APT_DIST: &str = "jammy";
/// Exact Debian revision suffix pinned for Redis 7.4.12 on `jammy`.
pub const REDIS_DEB_REVISION_SUFFIX: &str = "-1rl1~jammy1";
/// License recorded for Redis 7.4.x (dual-licensed; DBLocal only links users to
/// Redis's official package and never redistributes it).
pub const REDIS_74_LICENSE: &str = "RSALv2 / SSPLv1";

struct RedisAptTarget {
    arch: Arch,
    deb_arch: &'static str,
}

const REDIS_TARGETS: [RedisAptTarget; 2] = [
    RedisAptTarget {
        arch: Arch::X86_64,
        deb_arch: "amd64",
    },
    RedisAptTarget {
        arch: Arch::Aarch64,
        deb_arch: "arm64",
    },
];

/// Parsed `InRelease` checksum entry `(sha256, size_bytes)` keyed by path.
pub fn parse_inrelease_sha256(text: &str) -> Result<BTreeMap<String, (String, u64)>, SourceError> {
    let mut in_sha256 = false;
    let mut map = BTreeMap::new();

    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        if trimmed == "SHA256:" {
            in_sha256 = true;
            continue;
        }
        if !in_sha256 {
            continue;
        }
        // A new top-level header or PGP signature block ends the SHA256 section.
        if !raw_line.starts_with(' ') && !raw_line.starts_with('\t') {
            break;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(size_str) = parts.next() else {
            continue;
        };
        let Some(rel_path) = parts.next() else {
            continue;
        };
        if !is_sha256_hex(&hash.to_ascii_lowercase()) {
            return Err(SourceError::new(format!(
                "InRelease has malformed SHA256 {hash:?} for {rel_path}"
            )));
        }
        let size = size_str.parse::<u64>().map_err(|_| {
            SourceError::new(format!(
                "InRelease has malformed size {size_str:?} for {rel_path}"
            ))
        })?;
        map.insert(rel_path.to_owned(), (hash.to_ascii_lowercase(), size));
    }

    if map.is_empty() {
        return Err(SourceError::new(
            "InRelease contains no SHA256 entries".to_owned(),
        ));
    }
    Ok(map)
}

/// One resolved `redis-tools` stanza from `Packages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebPackageRecord {
    /// Relative path inside the APT pool (e.g. `pool/jammy/r/re/redis-tools_...deb`).
    pub filename: String,
    /// Package size in bytes.
    pub size_bytes: u64,
    /// Lowercase 64-hex SHA-256.
    pub sha256: String,
}

/// Parses an uncompressed APT `Packages` index and finds `redis-tools` matching
/// `expected_version` (ignoring any Debian epoch like `6:`).
pub fn find_redis_tools_stanza(
    packages_text: &str,
    expected_version: &str,
) -> Result<DebPackageRecord, SourceError> {
    let mut pkg: Option<String> = None;
    let mut ver: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut size: Option<u64> = None;
    let mut sha256: Option<String> = None;

    let flush = |pkg: &mut Option<String>,
                 ver: &mut Option<String>,
                 filename: &mut Option<String>,
                 size: &mut Option<u64>,
                 sha256: &mut Option<String>|
     -> Result<Option<DebPackageRecord>, SourceError> {
        let matched = pkg.as_deref() == Some("redis-tools")
            && ver
                .as_deref()
                .map(strip_deb_epoch)
                .is_some_and(|v| v == expected_version);
        let result = if matched {
            let f = filename
                .take()
                .ok_or_else(|| SourceError::new("redis-tools stanza missing Filename"))?;
            let s = size
                .take()
                .ok_or_else(|| SourceError::new("redis-tools stanza missing Size"))?;
            let h = sha256
                .take()
                .ok_or_else(|| SourceError::new("redis-tools stanza missing SHA256"))?;
            if f.contains("..") || f.starts_with('/') {
                return Err(SourceError::new(format!(
                    "redis-tools stanza has unsafe Filename {f:?}"
                )));
            }
            if s == 0 {
                return Err(SourceError::new("redis-tools stanza has zero Size"));
            }
            if !is_sha256_hex(&h) {
                return Err(SourceError::new(format!(
                    "redis-tools stanza has invalid SHA256 {h:?}"
                )));
            }
            Some(DebPackageRecord {
                filename: f,
                size_bytes: s,
                sha256: h,
            })
        } else {
            None
        };
        *pkg = None;
        *ver = None;
        *filename = None;
        *size = None;
        *sha256 = None;
        Ok(result)
    };

    for line in packages_text.lines() {
        if line.trim().is_empty() {
            if let Some(found) = flush(&mut pkg, &mut ver, &mut filename, &mut size, &mut sha256)? {
                return Ok(found);
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Package" => pkg = Some(value.to_owned()),
            "Version" => ver = Some(value.to_owned()),
            "Filename" => filename = Some(value.to_owned()),
            "Size" => size = value.parse::<u64>().ok(),
            "SHA256" => sha256 = Some(value.to_ascii_lowercase()),
            _ => {}
        }
    }
    if let Some(found) = flush(&mut pkg, &mut ver, &mut filename, &mut size, &mut sha256)? {
        return Ok(found);
    }

    Err(SourceError::new(format!(
        "Packages index has no redis-tools stanza matching version {expected_version}"
    )))
}

fn strip_deb_epoch(version: &str) -> &str {
    match version.split_once(':') {
        Some((epoch, rest)) if !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit()) => {
            rest
        }
        _ => version,
    }
}

/// Resolves Redis versions and `.deb` artifacts from the official APT repo.
///
/// When `existing` already carries an artifact with the same `(engine, version,
/// url)` and its `sha256` and `size_bytes` match the verified `Packages.gz`
/// index, the builder skips re-downloading the `.deb`. Otherwise it downloads
/// the `.deb` and cross-checks its SHA-256 and byte length against the index.
pub fn redis_from_apt(
    tracked: &TrackedVersion,
    source: &dyn Source,
    existing: Option<&CatalogDocument>,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let version = tracked.version;
    let deb_version = format!("{version}{REDIS_DEB_REVISION_SUFFIX}");
    let inrelease_url = format!("{REDIS_APT_BASE}/dists/{REDIS_APT_DIST}/InRelease");
    let inrelease_text = source.get_text(&inrelease_url)?;
    let inrelease_hashes = parse_inrelease_sha256(&inrelease_text)?;

    let versions = vec![CatalogVersion {
        engine: "redis".to_owned(),
        version: version.to_owned(),
        major_line: version.split('.').next().unwrap_or(version).to_owned(),
        release_date: None,
        eol: tracked.eol,
        line: Some(tracked.line.to_owned()),
        status: tracked.status.map(|s| s.as_str().to_owned()),
        license: Some(REDIS_74_LICENSE.to_owned()),
    }];

    let mut artifacts = Vec::with_capacity(REDIS_TARGETS.len());
    for target in REDIS_TARGETS {
        let index_rel = format!("main/binary-{}/Packages.gz", target.deb_arch);
        let (expected_idx_sha, expected_idx_size) =
            inrelease_hashes.get(&index_rel).ok_or_else(|| {
                SourceError::new(format!("InRelease is missing SHA256 entry for {index_rel}"))
            })?;

        let index_url = format!("{REDIS_APT_BASE}/dists/{REDIS_APT_DIST}/{index_rel}");
        let gz_bytes = source.get_bytes(&index_url)?;
        if gz_bytes.len() as u64 != *expected_idx_size {
            return Err(SourceError::new(format!(
                "{index_rel} size mismatch against InRelease: got {} bytes, expected {expected_idx_size}",
                gz_bytes.len()
            )));
        }
        let actual_idx_sha = hex(&Sha256::digest(&gz_bytes));
        if actual_idx_sha != *expected_idx_sha {
            return Err(SourceError::new(format!(
                "{index_rel} SHA-256 mismatch against InRelease: got {actual_idx_sha}, expected {expected_idx_sha}"
            )));
        }

        let mut dec = flate2::read::GzDecoder::new(gz_bytes.as_slice());
        let mut packages_text = String::new();
        dec.read_to_string(&mut packages_text)
            .map_err(|err| SourceError::new(format!("failed to gunzip {index_rel}: {err}")))?;

        let record = find_redis_tools_stanza(&packages_text, &deb_version)?;
        let url = format!("{REDIS_APT_BASE}/{}", record.filename);
        require_https(&url)?;

        let reused = existing.and_then(|doc| {
            doc.artifacts.iter().find(|a| {
                a.engine == "redis"
                    && a.version == version
                    && a.url == url
                    && a.sha256 == record.sha256
                    && a.size_bytes == record.size_bytes
            })
        });

        if reused.is_none() {
            let deb_bytes = source.get_bytes(&url)?;
            if deb_bytes.len() as u64 != record.size_bytes {
                return Err(SourceError::new(format!(
                    "{url} size mismatch against Packages.gz: got {} bytes, expected {}",
                    deb_bytes.len(),
                    record.size_bytes
                )));
            }
            let actual_sha = hex(&Sha256::digest(&deb_bytes));
            if actual_sha != record.sha256 {
                return Err(SourceError::new(format!(
                    "{url} SHA-256 mismatch against Packages.gz: got {actual_sha}, expected {}",
                    record.sha256
                )));
            }
        }

        artifacts.push(CatalogArtifact {
            id: format!(
                "redis-{version}-{}-{}-{REDIS_APT_DIST}",
                Os::Linux.as_str(),
                target.arch.as_str()
            ),
            engine: "redis".to_owned(),
            version: version.to_owned(),
            os: Os::Linux.as_str().to_owned(),
            arch: target.arch.as_str().to_owned(),
            url,
            sha256: record.sha256,
            size_bytes: record.size_bytes,
            format: "deb".to_owned(),
            strip_prefix: Some("usr".to_owned()),
            min_glibc: Some("2.34".to_owned()),
            variant: Some(REDIS_APT_DIST.to_owned()),
        });
    }

    Ok((versions, artifacts))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn gzip(text: &str) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(text.as_bytes()).expect("gz write");
        enc.finish().expect("gz finish")
    }

    struct FakeAptSource {
        text: BTreeMap<String, String>,
        bytes: BTreeMap<String, Vec<u8>>,
        deb_downloads: AtomicUsize,
    }

    impl Source for FakeAptSource {
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
            if url.ends_with(".deb") {
                self.deb_downloads.fetch_add(1, Ordering::SeqCst);
            }
            self.bytes
                .get(url)
                .cloned()
                .ok_or_else(|| SourceError::new(format!("missing bytes for {url}")))
        }

        fn get_text_with(&self, url: &str, _accept_encoding: &str) -> Result<String, SourceError> {
            self.text
                .get(url)
                .cloned()
                .ok_or_else(|| SourceError::new(format!("missing text for {url}")))
        }

        fn head_size(&self, url: &str, _accept_encoding: &str) -> Result<u64, SourceError> {
            Err(SourceError::new(format!("unexpected HEAD {url}")))
        }
    }

    fn fixture_source(corrupt_deb: bool, corrupt_packages_hash: bool) -> FakeAptSource {
        let amd64_deb = b"fake-amd64-deb-bytes".to_vec();
        let arm64_deb = b"fake-arm64-deb-bytes".to_vec();
        let amd64_sha = hex(&Sha256::digest(&amd64_deb));
        let arm64_sha = hex(&Sha256::digest(&arm64_deb));

        let pkg_amd64 = format!(
            "Package: redis-tools\nVersion: 6:7.4.12-1rl1~jammy1\nArchitecture: amd64\nFilename: pool/jammy/r/re/redis-tools_7.4.12-1rl1~jammy1_amd64.deb\nSize: {}\nSHA256: {amd64_sha}\n\n",
            amd64_deb.len()
        );
        let pkg_arm64 = format!(
            "Package: redis-tools\nVersion: 6:7.4.12-1rl1~jammy1\nArchitecture: arm64\nFilename: pool/jammy/r/re/redis-tools_7.4.12-1rl1~jammy1_arm64.deb\nSize: {}\nSHA256: {arm64_sha}\n\n",
            arm64_deb.len()
        );

        let gz_amd64 = gzip(&pkg_amd64);
        let gz_arm64 = gzip(&pkg_arm64);
        let mut gz_amd64_sha = hex(&Sha256::digest(&gz_amd64));
        if corrupt_packages_hash {
            gz_amd64_sha = "0".repeat(64);
        }
        let gz_arm64_sha = hex(&Sha256::digest(&gz_arm64));

        let inrelease = format!(
            "Origin: Redis\nSHA256:\n {gz_amd64_sha} {} main/binary-amd64/Packages.gz\n {gz_arm64_sha} {} main/binary-arm64/Packages.gz\n-----BEGIN PGP SIGNATURE-----\n",
            gz_amd64.len(),
            gz_arm64.len()
        );

        let mut text = BTreeMap::new();
        text.insert(
            format!("{REDIS_APT_BASE}/dists/{REDIS_APT_DIST}/InRelease"),
            inrelease,
        );

        let mut bytes = BTreeMap::new();
        bytes.insert(
            format!("{REDIS_APT_BASE}/dists/{REDIS_APT_DIST}/main/binary-amd64/Packages.gz"),
            gz_amd64,
        );
        bytes.insert(
            format!("{REDIS_APT_BASE}/dists/{REDIS_APT_DIST}/main/binary-arm64/Packages.gz"),
            gz_arm64,
        );
        bytes.insert(
            format!("{REDIS_APT_BASE}/pool/jammy/r/re/redis-tools_7.4.12-1rl1~jammy1_amd64.deb"),
            if corrupt_deb {
                b"tampered-amd64-bytes".to_vec()
            } else {
                amd64_deb
            },
        );
        bytes.insert(
            format!("{REDIS_APT_BASE}/pool/jammy/r/re/redis-tools_7.4.12-1rl1~jammy1_arm64.deb"),
            arm64_deb,
        );

        FakeAptSource {
            text,
            bytes,
            deb_downloads: AtomicUsize::new(0),
        }
    }

    fn tracked_7412() -> TrackedVersion {
        TrackedVersion {
            version: "7.4.12",
            line: "7.4",
            eol: false,
            status: None,
        }
    }

    #[test]
    fn resolves_and_cross_checks_both_arches_and_reuses_unchanged_entries() {
        let source = fixture_source(false, false);
        let (versions, artifacts) =
            redis_from_apt(&tracked_7412(), &source, None).expect("resolve redis");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].engine, "redis");
        assert_eq!(versions[0].version, "7.4.12");
        assert_eq!(versions[0].license.as_deref(), Some(REDIS_74_LICENSE));
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].format, "deb");
        assert_eq!(artifacts[0].strip_prefix.as_deref(), Some("usr"));
        assert_eq!(source.deb_downloads.load(Ordering::SeqCst), 2);

        // Second run with existing catalog reuses both entries without downloading .deb files.
        let existing = super::super::assemble(versions, artifacts, None);
        let source2 = fixture_source(false, false);
        let (_, reused_artifacts) =
            redis_from_apt(&tracked_7412(), &source2, Some(&existing)).expect("reuse");
        assert_eq!(reused_artifacts.len(), 2);
        assert_eq!(source2.deb_downloads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rejects_packages_gz_mismatch_against_inrelease() {
        let source = fixture_source(false, true);
        let err = redis_from_apt(&tracked_7412(), &source, None).unwrap_err();
        assert!(err.to_string().contains("InRelease"), "{err}");
    }

    #[test]
    fn rejects_deb_hash_mismatch_against_packages_gz() {
        let source = fixture_source(true, false);
        let err = redis_from_apt(&tracked_7412(), &source, None).unwrap_err();
        assert!(err.to_string().contains("SHA-256 mismatch"), "{err}");
    }
}
