//! RustFS builds from `rustfs/rustfs` GitHub releases.
//!
//! Resolves the latest stable release `>= 1.0.6` (never drafts, prereleases,
//! `-preview` tags, or `-latest.zip` assets), reads `SHA256SUMS`, cross-checks
//! each target archive by downloading and hashing it, and reuses unchanged
//! verified entries from an existing catalog to avoid redundant downloads.

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::helpers::require_https;
use super::postgresql::GithubAsset;
use super::{
    Arch, ArchiveFormat, CatalogArtifact, CatalogDocument, CatalogVersion, Os, Source, SourceError,
};

/// GitHub Releases API endpoint for `rustfs/rustfs`.
pub const RUSTFS_RELEASES_URL: &str =
    "https://api.github.com/repos/rustfs/rustfs/releases?per_page=30";

/// Minimum allowed RustFS version (43 advisories preceded 1.0.6; never offer older).
pub const RUSTFS_MIN_VERSION: (u64, u64, u64) = (1, 0, 6);

/// Upstream license expression for RustFS.
pub const RUSTFS_LICENSE: &str = "Apache-2.0";

/// `(os, arch)` targets published for RustFS in DBLocal.
const RUSTFS_TARGETS: [(Os, Arch, &str); 3] = [
    (Os::Linux, Arch::X86_64, "linux-x86_64-musl"),
    (Os::Linux, Arch::Aarch64, "linux-aarch64-musl"),
    (Os::Windows, Arch::X86_64, "windows-x86_64"),
];

/// One GitHub release entry for `rustfs/rustfs`.
#[derive(Debug, Clone, Deserialize)]
pub struct RustfsRelease {
    /// Release tag, e.g. `1.0.7`.
    pub tag_name: String,
    /// Whether GitHub marks this release as a prerelease.
    #[serde(default)]
    pub prerelease: bool,
    /// Whether GitHub marks this release as a draft.
    #[serde(default)]
    pub draft: bool,
    /// Publish timestamp, RFC 3339.
    #[serde(default)]
    pub published_at: Option<String>,
    /// Assets attached to the release.
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

/// Parses a strict `MAJOR.MINOR.PATCH` stable version (rejecting prerelease/build tags).
fn parse_stable_triple(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.strip_prefix('v').unwrap_or(raw);
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Selects the highest stable RustFS release `>= 1.0.6`.
pub fn select_latest_stable_release(
    releases: &[RustfsRelease],
) -> Result<(&RustfsRelease, (u64, u64, u64)), SourceError> {
    let min = RUSTFS_MIN_VERSION;
    let mut best: Option<(&RustfsRelease, (u64, u64, u64))> = None;
    for release in releases {
        if release.draft || release.prerelease {
            continue;
        }
        let Some(ver) = parse_stable_triple(&release.tag_name) else {
            continue;
        };
        if ver < min {
            continue;
        }
        if best.as_ref().is_none_or(|(_, current)| ver > *current) {
            best = Some((release, ver));
        }
    }
    best.ok_or_else(|| {
        SourceError::new(format!(
            "no stable RustFS release >= {}.{}.{} found in upstream releases",
            min.0, min.1, min.2
        ))
    })
}

/// Parses a `SHA256SUMS` file and returns the lowercase 64-hex hash for `filename`.
pub fn parse_sha256sums(contents: &str, filename: &str) -> Result<String, SourceError> {
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name_part) = parts.next() else {
            continue;
        };
        let clean_name = name_part.trim_start_matches('*');
        if clean_name == filename {
            let lowered = hash.to_ascii_lowercase();
            if lowered.len() == 64 && lowered.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(lowered);
            }
            return Err(SourceError::new(format!(
                "SHA256SUMS entry for {filename} is not a 64-hex SHA-256: {hash:?}"
            )));
        }
    }
    Err(SourceError::new(format!(
        "SHA256SUMS has no entry for {filename}"
    )))
}

/// Builds RustFS catalog version and artifacts from GitHub releases.
pub fn rustfs_from_releases(
    releases: &[RustfsRelease],
    source: &dyn Source,
    existing: Option<&CatalogDocument>,
) -> Result<(Vec<CatalogVersion>, Vec<CatalogArtifact>), SourceError> {
    let (release, ver) = select_latest_stable_release(releases)?;
    let version = format!("{}.{}.{}", ver.0, ver.1, ver.2);

    let sums_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS")
        .ok_or_else(|| {
            SourceError::new(format!("RustFS {version} release has no SHA256SUMS asset"))
        })?;
    require_https(&sums_asset.browser_download_url)?;
    let sums_text = source.get_text(&sums_asset.browser_download_url)?;

    let release_date = release
        .published_at
        .as_deref()
        .map(|stamp| stamp.split('T').next().unwrap_or(stamp).to_owned());
    let catalog_version = CatalogVersion {
        engine: "rustfs".to_owned(),
        version: version.clone(),
        major_line: ver.0.to_string(),
        release_date,
        eol: false,
        line: Some(format!("{}.{}", ver.0, ver.1)),
        status: None,
        license: Some(RUSTFS_LICENSE.to_owned()),
    };

    let mut artifacts = Vec::with_capacity(RUSTFS_TARGETS.len());
    for (os, arch, slug) in RUSTFS_TARGETS {
        let asset_name = format!("rustfs-{slug}-v{version}.zip");
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| {
                SourceError::new(format!(
                    "RustFS {version} release has no asset {asset_name}"
                ))
            })?;
        require_https(&asset.browser_download_url)?;
        let expected_sha = parse_sha256sums(&sums_text, &asset_name)?;

        let reused = existing.and_then(|doc| {
            doc.artifacts.iter().find(|a| {
                a.engine == "rustfs"
                    && a.version == version
                    && a.url == asset.browser_download_url
                    && a.sha256 == expected_sha
                    && a.size_bytes == asset.size
            })
        });

        let size_bytes = if let Some(prev) = reused {
            prev.size_bytes
        } else {
            let bytes = source.get_bytes(&asset.browser_download_url)?;
            let actual_sha = sha256_hex(&bytes);
            if actual_sha != expected_sha {
                return Err(SourceError::new(format!(
                    "RustFS {asset_name} SHA-256 mismatch: SHA256SUMS says {expected_sha}, downloaded bytes hash to {actual_sha}"
                )));
            }
            let actual_size = bytes.len() as u64;
            if asset.size > 0 && actual_size != asset.size {
                return Err(SourceError::new(format!(
                    "RustFS {asset_name} size mismatch: release says {} bytes, downloaded {} bytes",
                    asset.size, actual_size
                )));
            }
            actual_size
        };

        artifacts.push(CatalogArtifact {
            id: format!("rustfs-{version}-{}-{}", os.as_str(), arch.as_str()),
            engine: "rustfs".to_owned(),
            version: version.clone(),
            os: os.as_str().to_owned(),
            arch: arch.as_str().to_owned(),
            url: asset.browser_download_url.clone(),
            sha256: expected_sha,
            size_bytes,
            format: ArchiveFormat::Zip.as_str().to_owned(),
            strip_prefix: None,
            min_glibc: None,
            variant: None,
        });
    }

    Ok((vec![catalog_version], artifacts))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct FakeSource {
        text: BTreeMap<String, String>,
        bytes: BTreeMap<String, Vec<u8>>,
        byte_calls: AtomicUsize,
    }

    impl Source for FakeSource {
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
            self.byte_calls.fetch_add(1, Ordering::SeqCst);
            self.bytes
                .get(url)
                .cloned()
                .ok_or_else(|| SourceError::new(format!("no bytes for {url}")))
        }

        fn get_text_with(&self, url: &str, _accept_encoding: &str) -> Result<String, SourceError> {
            self.text
                .get(url)
                .cloned()
                .ok_or_else(|| SourceError::new(format!("no text for {url}")))
        }

        fn head_size(&self, url: &str, _accept_encoding: &str) -> Result<u64, SourceError> {
            Err(SourceError::new(format!("no head for {url}")))
        }
    }

    fn release(
        tag: &str,
        prerelease: bool,
        draft: bool,
        payloads: &[(&str, &[u8])],
    ) -> RustfsRelease {
        let mut assets = Vec::new();
        for (name, data) in payloads {
            assets.push(GithubAsset {
                name: (*name).to_owned(),
                size: data.len() as u64,
                browser_download_url: format!(
                    "https://github.com/rustfs/rustfs/releases/download/{tag}/{name}"
                ),
            });
        }
        assets.push(GithubAsset {
            name: "SHA256SUMS".to_owned(),
            size: 100,
            browser_download_url: format!(
                "https://github.com/rustfs/rustfs/releases/download/{tag}/SHA256SUMS"
            ),
        });
        RustfsRelease {
            tag_name: tag.to_owned(),
            prerelease,
            draft,
            published_at: Some("2026-11-26T08:09:20Z".to_owned()),
            assets,
        }
    }

    #[test]
    fn selects_highest_stable_at_least_1_0_6_and_skips_previews_and_older() {
        let releases = vec![
            release("1.0.8-preview.1", false, false, &[]),
            release("1.0.9", true, false, &[]),
            release("1.0.10", false, true, &[]),
            release("1.0.5", false, false, &[]),
            release("1.0.6", false, false, &[]),
            release("1.0.7", false, false, &[]),
        ];
        let (picked, ver) = select_latest_stable_release(&releases).expect("picked");
        assert_eq!(picked.tag_name, "1.0.7");
        assert_eq!(ver, (1, 0, 7));

        let only_old = vec![release("1.0.5", false, false, &[])];
        assert!(select_latest_stable_release(&only_old).is_err());
    }

    #[test]
    fn builds_cross_checked_artifacts_and_reuses_unchanged_entries() {
        let linux_x64 = b"rustfs-x64-zip-bytes";
        let linux_arm = b"rustfs-arm64-zip-bytes";
        let win_x64 = b"rustfs-win-zip-bytes";

        let payloads: &[(&str, &[u8])] = &[
            ("rustfs-linux-x86_64-musl-v1.0.7.zip", linux_x64),
            ("rustfs-linux-x86_64-musl-latest.zip", b"never-use-latest"),
            ("rustfs-linux-aarch64-musl-v1.0.7.zip", linux_arm),
            ("rustfs-windows-x86_64-v1.0.7.zip", win_x64),
        ];
        let rel = release("1.0.7", false, false, payloads);
        let sums = format!(
            "{}  rustfs-linux-x86_64-musl-v1.0.7.zip\n{}  rustfs-linux-aarch64-musl-v1.0.7.zip\n{}  rustfs-windows-x86_64-v1.0.7.zip\n",
            sha256_hex(linux_x64),
            sha256_hex(linux_arm),
            sha256_hex(win_x64),
        );
        let mut text = BTreeMap::new();
        text.insert(
            "https://github.com/rustfs/rustfs/releases/download/1.0.7/SHA256SUMS".to_owned(),
            sums,
        );
        let mut bytes = BTreeMap::new();
        for (name, data) in payloads {
            bytes.insert(
                format!("https://github.com/rustfs/rustfs/releases/download/1.0.7/{name}"),
                data.to_vec(),
            );
        }
        let source = FakeSource {
            text,
            bytes,
            byte_calls: AtomicUsize::new(0),
        };

        let (versions, artifacts) =
            rustfs_from_releases(std::slice::from_ref(&rel), &source, None).expect("build");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "1.0.7");
        assert_eq!(versions[0].license.as_deref(), Some("Apache-2.0"));
        assert_eq!(artifacts.len(), 3);
        assert!(artifacts.iter().all(|a| !a.url.contains("-latest")));
        assert_eq!(source.byte_calls.load(Ordering::SeqCst), 3);

        // Second build with existing catalog reuses all 3 entries with zero downloads.
        let existing = CatalogDocument {
            schema_version: 3,
            generated_at: None,
            source_url: "https://example.test/v3.json".to_owned(),
            versions,
            artifacts,
        };
        let (_, reused) =
            rustfs_from_releases(&[rel], &source, Some(&existing)).expect("reuse build");
        assert_eq!(reused.len(), 3);
        assert_eq!(source.byte_calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn rejects_sha256_mismatch_between_sums_and_downloaded_zip() {
        let payloads: &[(&str, &[u8])] = &[
            ("rustfs-linux-x86_64-musl-v1.0.7.zip", b"tampered"),
            ("rustfs-linux-aarch64-musl-v1.0.7.zip", b"arm"),
            ("rustfs-windows-x86_64-v1.0.7.zip", b"win"),
        ];
        let rel = release("1.0.7", false, false, payloads);
        let sums = format!(
            "{}  rustfs-linux-x86_64-musl-v1.0.7.zip\n{}  rustfs-linux-aarch64-musl-v1.0.7.zip\n{}  rustfs-windows-x86_64-v1.0.7.zip\n",
            "a".repeat(64),
            sha256_hex(b"arm"),
            sha256_hex(b"win"),
        );
        let mut text = BTreeMap::new();
        text.insert(
            "https://github.com/rustfs/rustfs/releases/download/1.0.7/SHA256SUMS".to_owned(),
            sums,
        );
        let mut bytes = BTreeMap::new();
        for (name, data) in payloads {
            bytes.insert(
                format!("https://github.com/rustfs/rustfs/releases/download/1.0.7/{name}"),
                data.to_vec(),
            );
        }
        let source = FakeSource {
            text,
            bytes,
            byte_calls: AtomicUsize::new(0),
        };
        let err = rustfs_from_releases(&[rel], &source, None).expect_err("mismatch");
        assert!(err.to_string().contains("SHA-256 mismatch"), "{err}");
    }
}
