//! Linux x86_64 PHP source adapter (`dl.static-php.dev`, static-php-cli).
//!
//! Discovers the latest patch per series (`8.0`..`8.5`) for both `common` and
//! `bulk` profiles from the upstream JSON directory listing, stream-hashes each
//! tarball (`hash_origin = "pinned"`), smoke-tests `php -n -v` and `php -n -m`,
//! and deletes each tarball immediately after verification.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::schema::{SkippedItem, ToolArtifact, ToolVersion, version_rank};
use super::smoke::SmokeRunner;
use super::transport::ToolchainSource;

pub const COMMON_INDEX_URL: &str = "https://dl.static-php.dev/static-php-cli/common/?format=json";
pub const BULK_INDEX_URL: &str = "https://dl.static-php.dev/static-php-cli/bulk/?format=json";
const BASE_ORIGIN: &str = "https://dl.static-php.dev";
const SOURCE_NAME: &str = "static-php-cli";

/// PHP series published for Linux x86_64 (no 7.4 on Linux).
pub const ALLOWED_SERIES: &[&str] = &["8.0", "8.1", "8.2", "8.3", "8.4", "8.5"];

#[derive(Debug, Deserialize)]
struct IndexEntry {
    #[serde(default)]
    is_dir: bool,
    name: String,
    full_path: String,
}

/// Discovered candidate artifact from the static-php-cli index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpLinuxCandidate {
    pub profile: String,
    pub series: String,
    pub version: String,
    pub url: String,
}

/// Parses a `static-php-cli` `?format=json` listing and selects the latest
/// patch per allowed series for `profile`.
pub fn discover_latest_for_profile(
    profile: &str,
    index_json: &str,
) -> Result<Vec<PhpLinuxCandidate>, String> {
    let entries: Vec<IndexEntry> = serde_json::from_str(index_json)
        .map_err(|err| format!("invalid static-php-cli {profile} index JSON: {err}"))?;
    let mut best_by_series: BTreeMap<String, PhpLinuxCandidate> = BTreeMap::new();

    for entry in entries {
        if entry.is_dir {
            continue;
        }
        let Some((version, series)) = parse_cli_linux_x64_filename(&entry.name) else {
            continue;
        };
        if !ALLOWED_SERIES.contains(&series.as_str()) {
            continue;
        }
        let url = format!("{BASE_ORIGIN}{}", entry.full_path);
        let candidate = PhpLinuxCandidate {
            profile: profile.to_owned(),
            series: series.clone(),
            version,
            url,
        };
        let replace = match best_by_series.get(&series) {
            None => true,
            Some(prev) => version_rank(&candidate.version) > version_rank(&prev.version),
        };
        if replace {
            best_by_series.insert(series, candidate);
        }
    }

    Ok(best_by_series.into_values().collect())
}

/// Parses `php-<major>.<minor>.<patch>-cli-linux-x86_64.tar.gz` into `(version, series)`.
pub fn parse_cli_linux_x64_filename(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("php-")?;
    let version = rest.strip_suffix("-cli-linux-x86_64.tar.gz")?;
    let mut parts = version.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    let _patch: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((version.to_owned(), format!("{major}.{minor}")))
}

/// Outcome of collecting Linux x86_64 PHP builds.
#[derive(Debug)]
pub struct PhpLinuxOutcome {
    pub versions: Vec<ToolVersion>,
    pub skipped: Vec<SkippedItem>,
    pub saved_php_binary: Option<PathBuf>,
}

/// Fetches, hashes, and smoke-tests all Linux x86_64 PHP artifacts.
///
/// Fails hard if an already-pinned URL returns a different SHA-256 than
/// `existing_pinned` (supply-chain tamper/drift protection).
pub fn collect(
    source: &dyn ToolchainSource,
    smoke: &dyn SmokeRunner,
    work_dir: &Path,
    existing_pinned: &BTreeMap<String, String>,
) -> Result<PhpLinuxOutcome, String> {
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for (profile, index_url) in [("common", COMMON_INDEX_URL), ("bulk", BULK_INDEX_URL)] {
        match source.get_text(index_url) {
            Ok(json) => match discover_latest_for_profile(profile, &json) {
                Ok(found) => candidates.extend(found),
                Err(reason) => skipped.push(SkippedItem {
                    tool: "php".to_owned(),
                    target: format!("linux/x86_64/{profile} index"),
                    reason,
                }),
            },
            Err(reason) => skipped.push(SkippedItem {
                tool: "php".to_owned(),
                target: format!("linux/x86_64/{profile} index"),
                reason,
            }),
        }
    }

    let mut versions = Vec::new();
    let mut saved_php: Option<PathBuf> = None;

    for cand in candidates {
        let tmp_archive = work_dir.join(format!("php-{}-{}.tar.gz", cand.version, cand.profile));
        let downloaded = match source.download_to_file(&cand.url, &tmp_archive) {
            Ok(file) => file,
            Err(reason) => {
                let _ = std::fs::remove_file(&tmp_archive);
                skipped.push(SkippedItem {
                    tool: "php".to_owned(),
                    target: format!("{} linux/x86_64/{}", cand.version, cand.profile),
                    reason,
                });
                continue;
            }
        };

        if let Some(prev_hash) = existing_pinned.get(&cand.url)
            && prev_hash != &downloaded.sha256
        {
            let _ = std::fs::remove_file(&tmp_archive);
            return Err(format!(
                "pinned SHA-256 changed for unchanged URL {}: previous {prev_hash}, got {}",
                cand.url, downloaded.sha256
            ));
        }

        let keep_path = if saved_php.is_none() && cand.profile == "common" {
            Some(work_dir.join("smoke-php-bin"))
        } else {
            None
        };

        let extensions =
            match smoke.verify_php_linux(&downloaded.path, &cand.version, keep_path.as_deref()) {
                Ok(exts) => {
                    if let Some(path) = keep_path {
                        saved_php = Some(path);
                    }
                    exts
                }
                Err(reason) => {
                    let _ = std::fs::remove_file(&tmp_archive);
                    skipped.push(SkippedItem {
                        tool: "php".to_owned(),
                        target: format!("{} linux/x86_64/{}", cand.version, cand.profile),
                        reason,
                    });
                    continue;
                }
            };

        let _ = std::fs::remove_file(&tmp_archive);

        versions.push(ToolVersion {
            version: cand.version,
            series: cand.series,
            lts: None,
            artifacts: vec![ToolArtifact {
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                profile: Some(cand.profile),
                format: "tar.gz".to_owned(),
                url: cand.url,
                sha256: downloaded.sha256,
                size_bytes: downloaded.size_bytes,
                source: SOURCE_NAME.to_owned(),
                hash_origin: "pinned".to_owned(),
                extensions: Some(extensions),
            }],
        });
    }

    Ok(PhpLinuxOutcome {
        versions,
        skipped,
        saved_php_binary: saved_php,
    })
}
