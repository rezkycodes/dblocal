//! Windows x86_64 PHP source adapter (`windows.php.net` `releases.json`).
//!
//! Selects the NTS x64 zip for each available series in `7.4`..`8.6`, uses the
//! upstream SHA-256 (`hash_origin = "upstream"`), and verifies availability and
//! exact byte size with an HTTPS `HEAD` request.

use std::collections::BTreeMap;

use serde_json::Value;

use super::schema::{SkippedItem, ToolArtifact, ToolVersion, is_sha256_hex};
use super::transport::ToolchainSource;

pub const RELEASES_JSON_URL: &str = "https://windows.php.net/downloads/releases/releases.json";
const DOWNLOAD_BASE_URL: &str = "https://downloads.php.net/~windows/releases/";
const SOURCE_NAME: &str = "windows.php.net";

/// PHP series range to include for Windows x86_64.
pub const ALLOWED_SERIES: &[&str] = &["7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5", "8.6"];

/// Parsed Windows PHP NTS x64 entry before HEAD size resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpWindowsCandidate {
    pub series: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// Parses `windows.php.net` `releases.json` into verified NTS x64 candidates
/// plus any skipped series.
pub fn parse_releases_json(
    json_text: &str,
) -> Result<(Vec<PhpWindowsCandidate>, Vec<SkippedItem>), String> {
    let root: BTreeMap<String, Value> = serde_json::from_str(json_text)
        .map_err(|err| format!("invalid windows.php.net releases.json: {err}"))?;

    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for (series, entry) in root {
        if !ALLOWED_SERIES.contains(&series.as_str()) {
            continue;
        }
        match extract_nts_x64(&series, &entry) {
            Ok(cand) => candidates.push(cand),
            Err(reason) => skipped.push(SkippedItem {
                tool: "php".to_owned(),
                target: format!("{series} windows/x86_64"),
                reason,
            }),
        }
    }
    Ok((candidates, skipped))
}

fn extract_nts_x64(series: &str, entry: &Value) -> Result<PhpWindowsCandidate, String> {
    let obj = entry
        .as_object()
        .ok_or_else(|| format!("series {series} entry is not an object"))?;
    let version = obj
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("series {series} missing version field"))?;

    let nts_val = obj
        .iter()
        .find(|(k, _)| k.starts_with("nts-") && k.ends_with("-x64"))
        .map(|(_, v)| v)
        .ok_or_else(|| format!("series {series} has no nts-*-x64 build"))?;

    let zip = nts_val
        .get("zip")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("series {series} nts-*-x64 has no zip entry"))?;

    let path = zip
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty() && !p.contains(".."))
        .ok_or_else(|| format!("series {series} zip missing valid path"))?;

    let sha256 = zip
        .get("sha256")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .filter(|h| is_sha256_hex(h))
        .ok_or_else(|| format!("series {series} zip missing valid upstream sha256"))?;

    Ok(PhpWindowsCandidate {
        series: series.to_owned(),
        version: version.to_owned(),
        url: format!("{DOWNLOAD_BASE_URL}{path}"),
        sha256,
    })
}

/// Fetches `releases.json` and verifies every candidate URL with a `HEAD`
/// request to obtain its exact `Content-Length`.
pub fn collect(source: &dyn ToolchainSource) -> (Vec<ToolVersion>, Vec<SkippedItem>) {
    let mut skipped = Vec::new();
    let json_text = match source.get_text(RELEASES_JSON_URL) {
        Ok(text) => text,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "php".to_owned(),
                target: "windows/x86_64 releases.json".to_owned(),
                reason,
            });
            return (Vec::new(), skipped);
        }
    };

    let (candidates, parse_skips) = match parse_releases_json(&json_text) {
        Ok(pair) => pair,
        Err(reason) => {
            skipped.push(SkippedItem {
                tool: "php".to_owned(),
                target: "windows/x86_64 releases.json".to_owned(),
                reason,
            });
            return (Vec::new(), skipped);
        }
    };
    skipped.extend(parse_skips);

    let mut versions = Vec::new();
    for cand in candidates {
        match source.head_size(&cand.url) {
            Ok(size_bytes) => versions.push(ToolVersion {
                version: cand.version,
                series: cand.series,
                lts: None,
                artifacts: vec![ToolArtifact {
                    os: "windows".to_owned(),
                    arch: "x86_64".to_owned(),
                    profile: None,
                    format: "zip".to_owned(),
                    url: cand.url,
                    sha256: cand.sha256,
                    size_bytes,
                    source: SOURCE_NAME.to_owned(),
                    hash_origin: "upstream".to_owned(),
                    extensions: None,
                }],
            }),
            Err(reason) => skipped.push(SkippedItem {
                tool: "php".to_owned(),
                target: format!("{} windows/x86_64", cand.version),
                reason,
            }),
        }
    }

    (versions, skipped)
}
