//! The active-version registry and the installed-version scan.
//!
//! Two sources of truth, deliberately separate:
//!
//! * **What is installed** is the directory layout under `<root>/toolchains`. A
//!   version directory that holds a valid [`crate::toolchain::consts::INSTALL_MANIFEST_FILE`]
//!   is installed. A half-removed directory, a stray file, or a dot-prefixed
//!   staging directory is ignored.
//! * **What is active** is `<root>/toolchains/active.json`, a tiny file DBLocal
//!   owns. It is written atomically and read tolerantly: a missing, unreadable or
//!   corrupt file means "nothing active", never a crash, and unknown fields are
//!   ignored so a newer build's file still loads.
//!
//! `state.db` is not touched: bumping its schema would make a downgrade to
//! v0.2.x impossible, which is why this choice gets its own file.

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::DblError;
use crate::toolchain::catalog::ToolId;
use crate::toolchain::consts;
use crate::toolchain::fsops::ToolchainPaths;

/// One active version of one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveVersion {
    tool: ToolId,
    version: String,
    profile: Option<String>,
}

impl ActiveVersion {
    /// Builds an active-version record.
    pub fn new(tool: ToolId, version: impl Into<String>, profile: Option<String>) -> Self {
        Self {
            tool,
            version: version.into(),
            profile: profile.filter(|p| !p.is_empty()),
        }
    }

    /// The tool.
    pub const fn tool(&self) -> ToolId {
        self.tool
    }

    /// The version tag.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The profile, when the install has one.
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Directory leaf under `toolchains/<tool>/`, e.g. `8.4.15-common`.
    pub fn dir_leaf(&self) -> String {
        match &self.profile {
            Some(profile) => format!("{}-{profile}", self.version),
            None => self.version.clone(),
        }
    }
}

/// The whole registry: at most one active version per tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    active: Vec<ActiveVersion>,
}

impl Registry {
    /// An empty registry (nothing active).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Every active version.
    pub fn active(&self) -> &[ActiveVersion] {
        &self.active
    }

    /// The active version of `tool`, if any.
    pub fn get(&self, tool: ToolId) -> Option<&ActiveVersion> {
        self.active.iter().find(|entry| entry.tool() == tool)
    }

    /// Returns a copy with `tool` set to `version`.
    pub fn with_active(&self, active: ActiveVersion) -> Self {
        let mut next: Vec<ActiveVersion> = self
            .active
            .iter()
            .filter(|entry| entry.tool() != active.tool())
            .cloned()
            .collect();
        next.push(active);
        next.sort_by_key(|entry| entry.tool());
        Self { active: next }
    }

    /// Returns a copy with `tool`'s entry cleared.
    pub fn without(&self, tool: ToolId) -> Self {
        let next: Vec<ActiveVersion> = self
            .active
            .iter()
            .filter(|entry| entry.tool() != tool)
            .cloned()
            .collect();
        Self { active: next }
    }

    /// Whether nothing is active.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

/// Reads the registry, tolerating every failure as "nothing active".
pub fn read_registry(paths: &ToolchainPaths) -> Registry {
    read_registry_at(&paths.active_registry())
}

/// Reads a registry file at an explicit path (tests and refactors).
pub fn read_registry_at(path: &Path) -> Registry {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Registry::empty(),
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "toolchain registry unreadable");
            return Registry::empty();
        }
    };
    let wire: WireRegistry = match serde_json::from_slice(&bytes) {
        Ok(wire) => wire,
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "toolchain registry is not valid JSON");
            return Registry::empty();
        }
    };
    if wire.schema != 0 && wire.schema != consts::ACTIVE_REGISTRY_SCHEMA_VERSION {
        tracing::debug!(path = %path.display(), schema = wire.schema, "toolchain registry schema not understood");
        return Registry::empty();
    }
    let mut active: Vec<ActiveVersion> = Vec::new();
    for entry in wire.active {
        let Ok(tool) = entry.tool.parse::<ToolId>() else {
            tracing::debug!(tool = %entry.tool, "toolchain registry: ignoring unknown tool");
            continue;
        };
        if entry.version.trim().is_empty() {
            continue;
        }
        if active.iter().any(|a| a.tool() == tool) {
            // A duplicate tool entry is a corrupt file; keep the first.
            continue;
        }
        active.push(ActiveVersion::new(tool, entry.version, entry.profile));
    }
    active.sort_by_key(|entry| entry.tool());
    Registry { active }
}

/// Writes the registry atomically (temp file + fsync + rename).
pub fn write_registry(paths: &ToolchainPaths, registry: &Registry) -> crate::Result<()> {
    write_registry_at(&paths.active_registry(), registry)
}

/// Writes a registry to an explicit path (tests).
pub fn write_registry_at(path: &Path, registry: &Registry) -> crate::Result<()> {
    let wire = WireRegistry {
        schema: consts::ACTIVE_REGISTRY_SCHEMA_VERSION,
        active: registry
            .active
            .iter()
            .map(|entry| WireActive {
                tool: entry.tool.as_str().to_owned(),
                version: entry.version.clone(),
                profile: entry.profile.clone(),
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&wire).map_err(|err| DblError::Storage {
        reason: format!("could not encode toolchain registry: {err}"),
    })?;
    bytes.push(b'\n');
    crate::fsutil::write_private_file_atomic(path, &bytes).map_err(|source| DblError::Io { source })
}

#[derive(Debug, Serialize, Deserialize)]
struct WireRegistry {
    #[serde(default)]
    schema: u32,
    #[serde(default, rename = "active")]
    active: Vec<WireActive>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireActive {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
}

// ---------------------------------------------------------------------------
// Installed-version scan
// ---------------------------------------------------------------------------

/// One installed version found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledVersion {
    tool: ToolId,
    version: String,
    profile: Option<String>,
    dir: std::path::PathBuf,
    installed_at: SystemTime,
    size_bytes: u64,
}

impl InstalledVersion {
    /// The tool.
    pub const fn tool(&self) -> ToolId {
        self.tool
    }
    /// The version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// The profile, when present.
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }
    /// The install directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    /// Manifest mtime, used as an install time.
    pub const fn installed_at(&self) -> SystemTime {
        self.installed_at
    }
    /// Size on disk in bytes.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    /// Directory leaf under `toolchains/<tool>/`.
    pub fn dir_leaf(&self) -> String {
        match &self.profile {
            Some(profile) => format!("{}-{profile}", self.version),
            None => self.version.clone(),
        }
    }
}

/// Manifest written into each install root.
#[derive(Debug, Serialize, Deserialize)]
struct WireManifest {
    schema: u32,
    tool: String,
    version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    sha256: String,
}

/// Writes the per-install manifest. Called by install right before the atomic
/// rename so the scan can trust a directory without a network round trip.
pub fn write_manifest(
    install_dir: &Path,
    tool: ToolId,
    version: &str,
    profile: Option<&str>,
    url: &str,
    sha256: &str,
) -> crate::Result<()> {
    let wire = WireManifest {
        schema: 1,
        tool: tool.as_str().to_owned(),
        version: version.to_owned(),
        profile: profile.filter(|p| !p.is_empty()).map(str::to_owned),
        url: url.to_owned(),
        sha256: sha256.to_owned(),
    };
    let mut bytes = serde_json::to_vec_pretty(&wire).map_err(|err| DblError::Storage {
        reason: format!("could not encode install manifest: {err}"),
    })?;
    bytes.push(b'\n');
    let path = install_dir.join(consts::INSTALL_MANIFEST_FILE);
    crate::fsutil::write_private_file_atomic(&path, &bytes)
        .map_err(|source| DblError::Io { source })
}

/// Reads the manifest of a directory, or `None` when it is not a valid install.
fn read_manifest(dir: &Path) -> Option<WireManifest> {
    let path = dir.join(consts::INSTALL_MANIFEST_FILE);
    let bytes = std::fs::read(&path).ok()?;
    let wire: WireManifest = serde_json::from_slice(&bytes).ok()?;
    if wire.schema != 1 {
        return None;
    }
    Some(wire)
}

/// Scans `<root>/toolchains` for installed versions.
///
/// Every dot-prefixed entry (`.tmp`, `.locks`) is skipped, and a directory
/// without a valid manifest is ignored: it is either mid-install, mid-remove or
/// junk. The result is sorted by tool then version (newest first per tool).
pub fn scan_installed(paths: &ToolchainPaths) -> Vec<InstalledVersion> {
    scan_installed_at(&paths.toolchains_dir())
}

/// Scans an explicit `toolchains` directory (tests and refactors).
pub fn scan_installed_at(toolchains_dir: &Path) -> Vec<InstalledVersion> {
    let mut out: Vec<InstalledVersion> = Vec::new();
    for tool in ToolId::all() {
        let tool_dir = toolchains_dir.join(tool.as_str());
        let entries = match std::fs::read_dir(&tool_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                tracing::debug!(path = %tool_dir.display(), %err, "toolchain scan skipped a tool dir");
                continue;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') {
                continue;
            }
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(manifest) = read_manifest(&dir) else {
                continue;
            };
            let Some(parsed) = parse_leaf(name) else {
                tracing::debug!(leaf = %name, "toolchain scan: ignoring unrecognised leaf");
                continue;
            };
            // Trust the directory name for the identity, but only when it agrees
            // with the manifest; a mismatch means a hand-edited tree.
            if manifest.version != parsed.version
                || manifest.profile.as_deref() != parsed.profile.as_deref()
            {
                tracing::debug!(leaf = %name, "toolchain scan: manifest disagrees with its directory");
                continue;
            }
            let installed_at = std::fs::metadata(dir.join(consts::INSTALL_MANIFEST_FILE))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(InstalledVersion {
                tool,
                version: parsed.version,
                profile: parsed.profile,
                dir,
                installed_at,
                size_bytes: dir_size(&entry.path()),
            });
        }
    }
    out.sort_by(|a, b| {
        a.tool()
            .as_str()
            .cmp(b.tool().as_str())
            .then_with(|| super::catalog::compare_versions(b.version(), a.version()))
            .then_with(|| a.profile().cmp(&b.profile()))
    });
    out
}

/// Splits a leaf like `8.4.15-common` into version and optional profile.
///
/// Only the known profile names split; a version with a dash of its own
/// (`8.4.0-rc1`) keeps its dash when no known profile matches.
fn parse_leaf(leaf: &str) -> Option<ParsedLeaf> {
    if let Some((version, profile)) = leaf.rsplit_once('-')
        && matches!(profile, "common" | "bulk")
        && !version.is_empty()
    {
        return Some(ParsedLeaf {
            version: version.to_owned(),
            profile: Some(profile.to_owned()),
        });
    }
    if leaf.is_empty() {
        return None;
    }
    Some(ParsedLeaf {
        version: leaf.to_owned(),
        profile: None,
    })
}

struct ParsedLeaf {
    version: String,
    profile: Option<String>,
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total = total.saturating_add(dir_size(&entry.path()));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

#[cfg(test)]
mod tests;
