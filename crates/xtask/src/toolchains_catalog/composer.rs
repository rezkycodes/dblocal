//! Composer source adapter (`getcomposer.org/versions` + `.sha256sum`).
//!
//! Picks the latest stable 2.x `composer.phar`, downloads and hashes it,
//! verifies agreement with the published `.sha256sum`, and smoke-tests it with
//! a verified Linux PHP binary from the same run.

use std::path::Path;

use serde::Deserialize;

use super::schema::{SkippedItem, ToolArtifact, ToolVersion, is_sha256_hex, version_rank};
use super::smoke::SmokeRunner;
use super::transport::ToolchainSource;

pub const VERSIONS_JSON_URL: &str = "https://getcomposer.org/versions";
const BASE_ORIGIN: &str = "https://getcomposer.org";
const SOURCE_NAME: &str = "getcomposer.org";

#[derive(Debug, Deserialize)]
struct VersionsDoc {
    #[serde(default)]
    stable: Vec<ComposerRelease>,
}

#[derive(Debug, Deserialize)]
struct ComposerRelease {
    path: String,
    version: String,
}

/// Selected latest stable 2.x Composer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerCandidate {
    pub version: String,
    pub phar_url: String,
    pub sha256_url: String,
}

/// Parses `getcomposer.org/versions` and returns the latest stable 2.x release.
pub fn discover_latest_stable_2x(versions_json: &str) -> Result<ComposerCandidate, String> {
    let doc: VersionsDoc = serde_json::from_str(versions_json)
        .map_err(|err| format!("invalid getcomposer.org versions JSON: {err}"))?;

    let latest = doc
        .stable
        .into_iter()
        .filter(|rel| rel.version.starts_with("2.") && !rel.version.contains('-'))
        .max_by(|a, b| version_rank(&a.version).cmp(&version_rank(&b.version)))
        .ok_or_else(|| "no stable 2.x release found in getcomposer.org/versions".to_owned())?;

    if !latest.path.starts_with('/') {
        return Err(format!("unexpected composer path {:?}", latest.path));
    }
    let phar_url = format!("{BASE_ORIGIN}{}", latest.path);
    let sha256_url = format!("{phar_url}.sha256sum");

    Ok(ComposerCandidate {
        version: latest.version,
        phar_url,
        sha256_url,
    })
}

/// Parses `<64-hex>  composer.phar` from a `.sha256sum` file.
pub fn parse_sha256sum(text: &str) -> Result<String, String> {
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        let lower = hash.to_ascii_lowercase();
        if !is_sha256_hex(&lower) {
            continue;
        }
        if let Some(name) = fields.next() {
            let clean = name.trim_start_matches('*');
            if clean != "composer.phar" {
                return Err(format!(
                    "composer sha256sum names {clean:?}, expected \"composer.phar\""
                ));
            }
        }
        return Ok(lower);
    }
    Err("composer sha256sum contains no valid SHA-256".to_owned())
}

/// Collects the latest stable 2.x Composer phar, verifying its published hash
/// and running a smoke test against `php_binary`.
pub fn collect(
    source: &dyn ToolchainSource,
    smoke: &dyn SmokeRunner,
    work_dir: &Path,
    php_binary: Option<&Path>,
) -> (Vec<ToolVersion>, Vec<SkippedItem>) {
    let mut skipped = Vec::new();
    let versions_json = match source.get_text(VERSIONS_JSON_URL) {
        Ok(text) => text,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "composer".to_owned(),
                target: "versions".to_owned(),
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    let cand = match discover_latest_stable_2x(&versions_json) {
        Ok(c) => c,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "composer".to_owned(),
                target: "versions".to_owned(),
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    let target = format!("{} any/any", cand.version);
    let sha_text = match source.get_text(&cand.sha256_url) {
        Ok(text) => text,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "composer".to_owned(),
                target,
                reason,
            });
            return (Vec::new(), skipped);
        }
    };
    let expected_sha = match parse_sha256sum(&sha_text) {
        Ok(h) => h,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "composer".to_owned(),
                target,
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    let tmp_phar = work_dir.join(format!("composer-{}.phar", cand.version));
    let downloaded = match source.download_to_file(&cand.phar_url, &tmp_phar) {
        Ok(file) => file,
        Err(reason) => {
            let _ = std::fs::remove_file(&tmp_phar);
            skipped.push(SkippedItem {
                tool: "composer".to_owned(),
                target,
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    if downloaded.sha256 != expected_sha {
        let _ = std::fs::remove_file(&tmp_phar);
        skipped.push(SkippedItem {
            tool: "composer".to_owned(),
            target,
            reason: format!(
                "SHA-256 mismatch: upstream .sha256sum has {expected_sha}, download hashed {}",
                downloaded.sha256
            ),
        });
        return (Vec::new(), skipped);
    }

    let Some(php_bin) = php_binary else {
        let _ = std::fs::remove_file(&tmp_phar);
        skipped.push(SkippedItem {
            tool: "composer".to_owned(),
            target,
            reason: "no verified Linux PHP binary available to smoke-test composer.phar".to_owned(),
        });
        return (Vec::new(), skipped);
    };

    if let Err(reason) = smoke.verify_composer(&downloaded.path, php_bin, &cand.version) {
        let _ = std::fs::remove_file(&tmp_phar);
        skipped.push(SkippedItem {
            tool: "composer".to_owned(),
            target,
            reason,
        });
        return (Vec::new(), skipped);
    }
    let _ = std::fs::remove_file(&tmp_phar);

    let version = ToolVersion {
        version: cand.version,
        series: "2".to_owned(),
        lts: None,
        artifacts: vec![ToolArtifact {
            os: "any".to_owned(),
            arch: "any".to_owned(),
            profile: None,
            format: "phar".to_owned(),
            url: cand.phar_url,
            sha256: expected_sha,
            size_bytes: downloaded.size_bytes,
            source: SOURCE_NAME.to_owned(),
            hash_origin: "upstream".to_owned(),
            extensions: None,
        }],
    };
    (vec![version], skipped)
}
