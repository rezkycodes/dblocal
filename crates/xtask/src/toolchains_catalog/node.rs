//! Node.js source adapter (`nodejs.org/dist/index.json` + `SHASUMS256.txt`).
//!
//! Selects the latest stable patch for release lines `26`, `24`, and `22`,
//! records `linux-x64` (`tar.gz`) and `win-x64` (`zip`) with upstream hashes
//! and `HEAD` sizes, and smoke-tests each Linux tarball (`node -v`, `npm -v`)
//! before deleting it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::schema::{SkippedItem, ToolArtifact, ToolVersion, is_sha256_hex, version_rank};
use super::smoke::SmokeRunner;
use super::transport::ToolchainSource;

pub const INDEX_JSON_URL: &str = "https://nodejs.org/dist/index.json";
const DIST_BASE_URL: &str = "https://nodejs.org/dist";
const SOURCE_NAME: &str = "nodejs.org";

/// Node.js major release lines included in the catalog.
pub const ALLOWED_MAJORS: &[u64] = &[26, 24, 22];

#[derive(Debug, Deserialize)]
struct NodeIndexEntry {
    version: String,
    #[serde(default)]
    lts: Value,
}

/// Selected latest stable Node.js release for one major line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeReleaseCandidate {
    pub version: String,
    pub series: String,
    pub lts: bool,
}

/// Parses `nodejs.org/dist/index.json` and picks the latest stable patch for
/// each major line in [`ALLOWED_MAJORS`].
pub fn discover_latest_releases(index_json: &str) -> Result<Vec<NodeReleaseCandidate>, String> {
    let entries: Vec<NodeIndexEntry> = serde_json::from_str(index_json)
        .map_err(|err| format!("invalid nodejs.org index.json: {err}"))?;

    let mut best_by_major: BTreeMap<u64, NodeReleaseCandidate> = BTreeMap::new();
    for entry in entries {
        let Some(ver) = entry.version.strip_prefix('v') else {
            continue;
        };
        if ver.contains('-') {
            continue;
        }
        let Some(major) = ver.split('.').next().and_then(|p| p.parse::<u64>().ok()) else {
            continue;
        };
        if !ALLOWED_MAJORS.contains(&major) {
            continue;
        }
        let is_lts = match &entry.lts {
            Value::Bool(flag) => *flag,
            Value::String(name) => !name.trim().is_empty(),
            _ => false,
        };
        let candidate = NodeReleaseCandidate {
            version: ver.to_owned(),
            series: major.to_string(),
            lts: is_lts,
        };
        let replace = match best_by_major.get(&major) {
            None => true,
            Some(prev) => version_rank(&candidate.version) > version_rank(&prev.version),
        };
        if replace {
            best_by_major.insert(major, candidate);
        }
    }

    Ok(best_by_major.into_values().collect())
}

/// Parses a Node.js `SHASUMS256.txt` file into `filename -> lowercase sha256`.
pub fn parse_shasums256(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        let lower = hash.to_ascii_lowercase();
        if is_sha256_hex(&lower) {
            map.insert(name.trim_start_matches('*').to_owned(), lower);
        }
    }
    map
}

/// Collects Node.js versions and artifacts, verifying hashes, sizes, and Linux
/// tarball execution.
pub fn collect(
    source: &dyn ToolchainSource,
    smoke: &dyn SmokeRunner,
    work_dir: &Path,
) -> (Vec<ToolVersion>, Vec<SkippedItem>) {
    let mut skipped = Vec::new();
    let index_json = match source.get_text(INDEX_JSON_URL) {
        Ok(text) => text,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "node".to_owned(),
                target: "index.json".to_owned(),
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    let candidates = match discover_latest_releases(&index_json) {
        Ok(list) => list,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "node".to_owned(),
                target: "index.json".to_owned(),
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    let mut versions = Vec::new();
    for cand in candidates {
        let shasums_url = format!("{DIST_BASE_URL}/v{}/SHASUMS256.txt", cand.version);
        let shasums_text = match source.get_text(&shasums_url) {
            Ok(text) => text,
            Err(reason) => {
                skipped.push(SkippedItem {
                    tool: "node".to_owned(),
                    target: format!("{} SHASUMS256.txt", cand.version),
                    reason,
                });
                continue;
            }
        };
        let shasums = parse_shasums256(&shasums_text);
        let mut artifacts = Vec::new();

        if let Some(linux_art) = collect_linux_artifact(
            &cand.version,
            &shasums,
            source,
            smoke,
            work_dir,
            &mut skipped,
        ) {
            artifacts.push(linux_art);
        }
        if let Some(win_art) =
            collect_windows_artifact(&cand.version, &shasums, source, &mut skipped)
        {
            artifacts.push(win_art);
        }

        if !artifacts.is_empty() {
            versions.push(ToolVersion {
                version: cand.version,
                series: cand.series,
                lts: Some(cand.lts),
                artifacts,
            });
        }
    }

    (versions, skipped)
}

fn collect_linux_artifact(
    version: &str,
    shasums: &BTreeMap<String, String>,
    source: &dyn ToolchainSource,
    smoke: &dyn SmokeRunner,
    work_dir: &Path,
    skipped: &mut Vec<SkippedItem>,
) -> Option<ToolArtifact> {
    let filename = format!("node-v{version}-linux-x64.tar.gz");
    let target = format!("{version} linux/x86_64");
    let Some(expected_sha) = shasums.get(&filename) else {
        skipped.push(SkippedItem {
            tool: "node".to_owned(),
            target,
            reason: format!("{filename} missing from SHASUMS256.txt"),
        });
        return None;
    };

    let url = format!("{DIST_BASE_URL}/v{version}/{filename}");
    let head_size = match source.head_size(&url) {
        Ok(sz) => sz,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "node".to_owned(),
                target,
                reason,
            });
            return None;
        }
    };

    let tmp_archive = work_dir.join(&filename);
    let downloaded = match source.download_to_file(&url, &tmp_archive) {
        Ok(file) => file,
        Err(reason) => {
            let _ = std::fs::remove_file(&tmp_archive);
            skipped.push(SkippedItem {
                tool: "node".to_owned(),
                target,
                reason,
            });
            return None;
        }
    };

    if &downloaded.sha256 != expected_sha {
        let _ = std::fs::remove_file(&tmp_archive);
        skipped.push(SkippedItem {
            tool: "node".to_owned(),
            target,
            reason: format!(
                "SHA-256 mismatch: SHASUMS256.txt has {expected_sha}, download hashed {}",
                downloaded.sha256
            ),
        });
        return None;
    }

    if downloaded.size_bytes != head_size {
        let _ = std::fs::remove_file(&tmp_archive);
        skipped.push(SkippedItem {
            tool: "node".to_owned(),
            target,
            reason: format!(
                "size mismatch: HEAD reported {head_size}, download was {} bytes",
                downloaded.size_bytes
            ),
        });
        return None;
    }

    if let Err(reason) = smoke.verify_node_linux(&downloaded.path, version) {
        let _ = std::fs::remove_file(&tmp_archive);
        skipped.push(SkippedItem {
            tool: "node".to_owned(),
            target,
            reason,
        });
        return None;
    }
    let _ = std::fs::remove_file(&tmp_archive);

    Some(ToolArtifact {
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        profile: None,
        format: "tar.gz".to_owned(),
        url,
        sha256: expected_sha.clone(),
        size_bytes: head_size,
        source: SOURCE_NAME.to_owned(),
        hash_origin: "upstream".to_owned(),
        extensions: None,
    })
}

fn collect_windows_artifact(
    version: &str,
    shasums: &BTreeMap<String, String>,
    source: &dyn ToolchainSource,
    skipped: &mut Vec<SkippedItem>,
) -> Option<ToolArtifact> {
    let filename = format!("node-v{version}-win-x64.zip");
    let target = format!("{version} windows/x86_64");
    let Some(expected_sha) = shasums.get(&filename) else {
        skipped.push(SkippedItem {
            tool: "node".to_owned(),
            target,
            reason: format!("{filename} missing from SHASUMS256.txt"),
        });
        return None;
    };

    let url = format!("{DIST_BASE_URL}/v{version}/{filename}");
    let head_size = match source.head_size(&url) {
        Ok(sz) => sz,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "node".to_owned(),
                target,
                reason,
            });
            return None;
        }
    };

    Some(ToolArtifact {
        os: "windows".to_owned(),
        arch: "x86_64".to_owned(),
        profile: None,
        format: "zip".to_owned(),
        url,
        sha256: expected_sha.clone(),
        size_bytes: head_size,
        source: SOURCE_NAME.to_owned(),
        hash_origin: "upstream".to_owned(),
        extensions: None,
    })
}
