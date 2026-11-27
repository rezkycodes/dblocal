//! Schema types, deterministic assembly, and strict validation for
//! `catalog/toolchains-v1.json`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Schema version for `catalog/toolchains-v1.json`.
pub const TOOLCHAINS_SCHEMA_VERSION: u32 = 1;

const VALID_TOOLS: &[&str] = &["php", "node", "composer"];
const VALID_OS: &[&str] = &["linux", "windows", "any"];
const VALID_ARCH: &[&str] = &["x86_64", "any"];
const VALID_PROFILES: &[&str] = &["common", "bulk"];
const VALID_FORMATS: &[&str] = &["tar.gz", "zip", "phar"];
const VALID_HASH_ORIGINS: &[&str] = &["pinned", "upstream"];

/// Top-level `catalog/toolchains-v1.json` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainsDocument {
    pub schema: u32,
    pub generated_at: String,
    pub tools: Vec<ToolEntry>,
}

/// One tool entry (`php`, `node`, or `composer`) and its available versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolEntry {
    pub id: String,
    pub versions: Vec<ToolVersion>,
}

/// One version of a tool and its downloadable artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVersion {
    pub version: String,
    pub series: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lts: Option<bool>,
    pub artifacts: Vec<ToolArtifact>,
}

/// One downloadable artifact for a tool version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolArtifact {
    pub os: String,
    pub arch: String,
    pub profile: Option<String>,
    pub format: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub source: String,
    pub hash_origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
}

/// One skipped version or artifact recorded during generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedItem {
    pub tool: String,
    pub target: String,
    pub reason: String,
}

/// Whether two documents have identical content, ignoring `generated_at`.
pub fn same_content(left: &ToolchainsDocument, right: &ToolchainsDocument) -> bool {
    left.schema == right.schema && left.tools == right.tools
}

/// Assembles tools, merges artifacts sharing `(tool, version)`, and sorts
/// everything deterministically.
pub fn assemble(tools: Vec<ToolEntry>, generated_at: String) -> ToolchainsDocument {
    let mut by_tool: BTreeMap<String, Vec<ToolVersion>> = BTreeMap::new();
    for tool in tools {
        by_tool.entry(tool.id).or_default().extend(tool.versions);
    }

    let sorted_tools = by_tool
        .into_iter()
        .map(|(id, versions)| ToolEntry {
            id,
            versions: merge_and_sort_versions(versions),
        })
        .collect();

    ToolchainsDocument {
        schema: TOOLCHAINS_SCHEMA_VERSION,
        generated_at,
        tools: sorted_tools,
    }
}

fn merge_and_sort_versions(versions: Vec<ToolVersion>) -> Vec<ToolVersion> {
    let mut by_version: BTreeMap<String, ToolVersion> = BTreeMap::new();
    for version in versions {
        match by_version.get_mut(&version.version) {
            Some(existing) => {
                if existing.lts.is_none() {
                    existing.lts = version.lts;
                }
                existing.artifacts.extend(version.artifacts);
            }
            None => {
                by_version.insert(version.version.clone(), version);
            }
        }
    }

    let mut out: Vec<ToolVersion> = by_version
        .into_values()
        .map(|mut version| {
            for artifact in &mut version.artifacts {
                if let Some(exts) = &mut artifact.extensions {
                    exts.sort();
                    exts.dedup();
                }
            }
            version.artifacts.sort_by(|a, b| {
                a.os.cmp(&b.os)
                    .then_with(|| a.arch.cmp(&b.arch))
                    .then_with(|| {
                        a.profile
                            .as_deref()
                            .unwrap_or("")
                            .cmp(b.profile.as_deref().unwrap_or(""))
                    })
                    .then_with(|| a.url.cmp(&b.url))
            });
            version
        })
        .collect();

    out.sort_by_key(|b| std::cmp::Reverse(version_rank(&b.version)));
    out
}

/// Sort key for newest-first semver ordering.
pub fn version_rank(version: &str) -> (u64, u64, u64, String) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch, version.to_owned())
}

/// Whether `value` is exactly 64 lowercase ASCII hex characters.
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Strictly validates a generated `ToolchainsDocument`.
pub fn validate_document(doc: &ToolchainsDocument) -> Result<(), String> {
    if doc.schema != TOOLCHAINS_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schema {}, expected {TOOLCHAINS_SCHEMA_VERSION}",
            doc.schema
        ));
    }
    if doc.generated_at.trim().is_empty() {
        return Err("generated_at must not be empty".to_owned());
    }
    if doc.tools.is_empty() {
        return Err("tools must not be empty".to_owned());
    }

    let mut seen_tools = BTreeSet::new();
    for tool in &doc.tools {
        validate_tool(tool, &mut seen_tools)?;
    }
    Ok(())
}

fn validate_tool(tool: &ToolEntry, seen_tools: &mut BTreeSet<String>) -> Result<(), String> {
    if !VALID_TOOLS.contains(&tool.id.as_str()) {
        return Err(format!("unknown tool id {:?}", tool.id));
    }
    if !seen_tools.insert(tool.id.clone()) {
        return Err(format!("duplicate tool id {:?}", tool.id));
    }
    if tool.versions.is_empty() {
        return Err(format!("tool {:?} has zero versions", tool.id));
    }
    let mut seen_versions = BTreeSet::new();
    for version in &tool.versions {
        if !seen_versions.insert(version.version.clone()) {
            return Err(format!(
                "duplicate version {:?} for tool {:?}",
                version.version, tool.id
            ));
        }
        validate_version(&tool.id, version)?;
    }
    Ok(())
}

fn validate_version(tool_id: &str, version: &ToolVersion) -> Result<(), String> {
    if version.version.trim().is_empty() || version.series.trim().is_empty() {
        return Err(format!("empty version or series in tool {tool_id}"));
    }
    if version.artifacts.is_empty() {
        return Err(format!(
            "tool {tool_id} version {} has zero artifacts",
            version.version
        ));
    }
    let mut seen_keys = BTreeSet::new();
    for artifact in &version.artifacts {
        let key = (
            artifact.os.clone(),
            artifact.arch.clone(),
            artifact.profile.clone(),
        );
        if !seen_keys.insert(key.clone()) {
            return Err(format!(
                "duplicate artifact key {key:?} in {tool_id} {}",
                version.version
            ));
        }
        validate_artifact(tool_id, &version.version, artifact)?;
    }
    Ok(())
}

fn validate_artifact(tool_id: &str, version: &str, artifact: &ToolArtifact) -> Result<(), String> {
    if !VALID_OS.contains(&artifact.os.as_str()) {
        return Err(format!(
            "invalid os {:?} in {tool_id} {version}",
            artifact.os
        ));
    }
    if !VALID_ARCH.contains(&artifact.arch.as_str()) {
        return Err(format!(
            "invalid arch {:?} in {tool_id} {version}",
            artifact.arch
        ));
    }
    if let Some(profile) = &artifact.profile
        && !VALID_PROFILES.contains(&profile.as_str())
    {
        return Err(format!(
            "invalid profile {profile:?} in {tool_id} {version}"
        ));
    }
    if !VALID_FORMATS.contains(&artifact.format.as_str()) {
        return Err(format!(
            "invalid format {:?} in {tool_id} {version}",
            artifact.format
        ));
    }
    if !artifact.url.starts_with("https://") {
        return Err(format!(
            "non-HTTPS url {:?} in {tool_id} {version}",
            artifact.url
        ));
    }
    if !is_sha256_hex(&artifact.sha256) {
        return Err(format!(
            "invalid sha256 {:?} in {tool_id} {version}",
            artifact.sha256
        ));
    }
    if artifact.size_bytes == 0 {
        return Err(format!("zero size_bytes in {tool_id} {version}"));
    }
    if artifact.source.trim().is_empty() {
        return Err(format!("empty source in {tool_id} {version}"));
    }
    if !VALID_HASH_ORIGINS.contains(&artifact.hash_origin.as_str()) {
        return Err(format!(
            "invalid hash_origin {:?} in {tool_id} {version}",
            artifact.hash_origin
        ));
    }
    if let Some(exts) = &artifact.extensions {
        if tool_id != "php" {
            return Err(format!("extensions only allowed on php, not {tool_id}"));
        }
        if !exts.windows(2).all(|w| w[0] < w[1]) {
            return Err(format!(
                "extensions not strictly sorted in {tool_id} {version}"
            ));
        }
    }
    Ok(())
}
