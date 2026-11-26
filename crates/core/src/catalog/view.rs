//! Host-filtered read model built from a snapshot.
//!
//! The catalog is engine-agnostic data; the UI needs a list it can render
//! without re-implementing filtering (CONTRACT, `CatalogView`). This module is
//! the single place that decides what "installable on this host" means.
//!
//! Rules, straight from the PRD:
//!
//! - Only versions that have an artifact for the host `(os, arch)` are listed
//!   as installable (F-CAT-02 AC1).
//! - An engine with no artifact at all for this host is still listed, marked
//!   unavailable with a textual reason, never hidden and never an error
//!   (F-CAT-02 AC2, D-24).
//! - Versions are ordered newest first, and major lines are grouped so the
//!   picker can render `18.x` and `17.x` sections.

use std::collections::BTreeSet;

use semver::Version;

use crate::catalog::select::select_artifact;
use crate::model::{Artifact, CatalogSnapshot, CatalogStatus, EngineKey};
use crate::platform::{Arch, Os};
use crate::service::{CatalogView, EngineEntry, VersionEntry};
use crate::strings;

/// Reason shown for an engine that has no build on this host.
///
/// The wording is the PRD's ("not available on this platform", F-CAT-02 AC2).
/// It lives here rather than in `strings` only because that module belongs to
/// another worker this wave; see the W1-B report.
pub const UNAVAILABLE_ON_PLATFORM: &str = "Not available on this platform";

/// Versions already installed locally, as `(engine, version)` pairs.
///
/// Install state lives in the store, which the catalog does not depend on, so
/// the caller passes it in and the catalog only marks rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledVersions {
    pairs: BTreeSet<(EngineKey, String)>,
}

impl InstalledVersions {
    /// No installed runtimes.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Builds the set from `(engine, version)` pairs.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (EngineKey, String)>) -> Self {
        Self {
            pairs: pairs.into_iter().collect(),
        }
    }

    /// Marks one installed runtime.
    pub fn with(mut self, engine: EngineKey, version: impl Into<String>) -> Self {
        self.pairs.insert((engine, version.into()));
        self
    }

    /// Whether this exact runtime is installed.
    pub fn contains(&self, engine: EngineKey, version: &str) -> bool {
        self.pairs
            .iter()
            .any(|(key, tag)| *key == engine && tag == version)
    }
}

/// Builds the platform-filtered view the GUI and CLI render.
pub fn build_catalog_view(
    snapshot: &CatalogSnapshot,
    os: Os,
    arch: Arch,
    installed: &InstalledVersions,
    host_glibc: Option<(u32, u32)>,
) -> CatalogView {
    let status = snapshot.status();
    let available: Vec<&Artifact> = snapshot.artifacts_for(os, arch);

    let engines = EngineKey::ALL
        .iter()
        .map(|engine| engine_entry(snapshot, *engine, &available, installed, host_glibc))
        .collect();

    CatalogView {
        status,
        banner: banner_for(status),
        engines,
    }
}

/// Banner text for a freshness state, if any.
pub fn banner_for(status: CatalogStatus) -> Option<String> {
    match status {
        CatalogStatus::Fresh => None,
        CatalogStatus::Stale => Some(strings::CATALOG_STALE.to_owned()),
        CatalogStatus::Offline => Some(strings::CATALOG_OFFLINE.to_owned()),
    }
}

fn engine_entry(
    snapshot: &CatalogSnapshot,
    engine: EngineKey,
    available: &[&Artifact],
    installed: &InstalledVersions,
    host_glibc: Option<(u32, u32)>,
) -> EngineEntry {
    let mut versions = Vec::new();
    let mut versioned = snapshot
        .versions()
        .iter()
        .filter(|entry| entry.engine() == engine)
        .collect::<Vec<_>>();

    // Newest first. Semver is the tie breaker inside a major line; a tag that
    // is not semver sorts last instead of failing the whole view.
    versioned.sort_by(
        |left, right| match (parse(left.version()), parse(right.version())) {
            (Some(a), Some(b)) => b.cmp(&a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => right.version().cmp(left.version()),
        },
    );

    for entry in versioned {
        let Some(artifact) = select_artifact(available, engine, entry.version(), host_glibc) else {
            continue;
        };
        versions.push(VersionEntry {
            version: entry.version().to_owned(),
            eol: entry.is_eol(),
            installed: installed.contains(engine, entry.version()),
            size_bytes: artifact.size_bytes(),
        });
    }

    let available_here = !versions.is_empty();
    EngineEntry {
        engine,
        available: available_here,
        unavailable_reason: (!available_here).then(|| UNAVAILABLE_ON_PLATFORM.to_owned()),
        versions,
    }
}

/// One major line of one engine, newest version first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MajorLineGroup {
    /// Major line, for example `18`.
    pub major_line: String,
    /// Versions in that line, newest first.
    pub versions: Vec<VersionEntry>,
}

/// Groups an engine's versions by major line, keeping newest-first order.
pub fn major_line_groups(entry: &EngineEntry) -> Vec<MajorLineGroup> {
    let mut groups: Vec<MajorLineGroup> = Vec::new();
    for version in &entry.versions {
        let major = version
            .version
            .split('.')
            .next()
            .unwrap_or(&version.version)
            .to_owned();
        match groups.last_mut() {
            Some(group) if group.major_line == major => group.versions.push(version.clone()),
            _ => groups.push(MajorLineGroup {
                major_line: major,
                versions: vec![version.clone()],
            }),
        }
    }
    groups
}

fn parse(tag: &str) -> Option<Version> {
    Version::parse(tag).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArchiveFormat, CatalogSnapshot};
    use std::time::SystemTime;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn pg_version(tag: &str, eol: bool) -> crate::model::EngineVersion {
        crate::model::EngineVersion::new(EngineKey::Postgresql, tag, None, eol).expect("version")
    }

    fn artifact(engine: EngineKey, tag: &str, os: Os, arch: Arch, size: u64) -> Artifact {
        Artifact::new(
            format!("{}-{tag}-{os}-{arch}", engine.as_str()),
            engine,
            tag,
            os,
            arch,
            format!("https://example.test/{engine}-{tag}.tar.gz"),
            SHA,
            size,
            ArchiveFormat::TarGz,
        )
        .expect("artifact")
    }

    fn snapshot(status: CatalogStatus) -> CatalogSnapshot {
        CatalogSnapshot::new(
            "s1",
            SystemTime::UNIX_EPOCH,
            "https://example.test/catalog.json",
            1,
            vec![
                pg_version("18.7.0", false),
                pg_version("17.12.0", true),
                crate::model::EngineVersion::new(EngineKey::Valkey, "9.2.0", None, false)
                    .expect("version"),
            ],
            vec![
                artifact(
                    EngineKey::Postgresql,
                    "18.7.0",
                    Os::Linux,
                    Arch::X86_64,
                    100,
                ),
                artifact(
                    EngineKey::Postgresql,
                    "17.12.0",
                    Os::Linux,
                    Arch::X86_64,
                    90,
                ),
                artifact(EngineKey::Valkey, "9.2.0", Os::Linux, Arch::Aarch64, 70),
            ],
            status,
        )
    }

    #[test]
    fn lists_only_versions_installable_here() {
        let view = build_catalog_view(
            &snapshot(CatalogStatus::Fresh),
            Os::Linux,
            Arch::X86_64,
            &InstalledVersions::empty(),
            None,
        );
        let postgres = &view.engines[0];
        assert_eq!(postgres.engine, EngineKey::Postgresql);
        assert!(postgres.available);
        assert_eq!(postgres.versions.len(), 2);
        assert_eq!(postgres.versions[0].version, "18.7.0");
        assert_eq!(postgres.versions[1].version, "17.12.0");
        assert!(postgres.versions[1].eol);
        assert_eq!(postgres.versions[0].size_bytes, 100);
        assert!(postgres.unavailable_reason.is_none());
    }

    #[test]
    fn missing_engine_is_visible_and_disabled() {
        let view = build_catalog_view(
            &snapshot(CatalogStatus::Fresh),
            Os::Windows,
            Arch::X86_64,
            &InstalledVersions::empty(),
            None,
        );
        assert_eq!(view.engines.len(), EngineKey::ALL.len());
        for entry in &view.engines {
            assert!(!entry.available, "{} should be unavailable", entry.engine);
            assert_eq!(
                entry.unavailable_reason.as_deref(),
                Some(UNAVAILABLE_ON_PLATFORM)
            );
            assert!(entry.versions.is_empty());
        }
    }

    #[test]
    fn one_engine_can_be_available_while_another_is_not() {
        let view = build_catalog_view(
            &snapshot(CatalogStatus::Fresh),
            Os::Linux,
            Arch::Aarch64,
            &InstalledVersions::empty(),
            None,
        );
        let postgres = &view.engines[0];
        let valkey = &view.engines[3];
        assert!(!postgres.available);
        assert!(valkey.available);
        assert_eq!(valkey.versions[0].size_bytes, 70);
    }

    #[test]
    fn installed_versions_are_marked() {
        let installed = InstalledVersions::empty().with(EngineKey::Postgresql, "17.12.0");
        let view = build_catalog_view(
            &snapshot(CatalogStatus::Fresh),
            Os::Linux,
            Arch::X86_64,
            &installed,
            None,
        );
        let versions = &view.engines[0].versions;
        assert!(!versions[0].installed);
        assert!(versions[1].installed);
        assert!(installed.contains(EngineKey::Postgresql, "17.12.0"));
        assert!(!installed.contains(EngineKey::Postgresql, "18.7.0"));
    }

    #[test]
    fn banner_tracks_freshness() {
        for (status, expected) in [
            (CatalogStatus::Fresh, None),
            (CatalogStatus::Stale, Some(strings::CATALOG_STALE)),
            (CatalogStatus::Offline, Some(strings::CATALOG_OFFLINE)),
        ] {
            let view = build_catalog_view(
                &snapshot(status),
                Os::Linux,
                Arch::X86_64,
                &InstalledVersions::empty(),
                None,
            );
            assert_eq!(view.status, status);
            assert_eq!(view.banner.as_deref(), expected);
        }
    }

    #[test]
    fn major_lines_group_newest_first() {
        let view = build_catalog_view(
            &snapshot(CatalogStatus::Fresh),
            Os::Linux,
            Arch::X86_64,
            &InstalledVersions::empty(),
            None,
        );
        let groups = major_line_groups(&view.engines[0]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].major_line, "18");
        assert_eq!(groups[1].major_line, "17");
        assert_eq!(groups[0].versions[0].version, "18.7.0");
    }

    #[test]
    fn glibc_choice_controls_the_reported_size() {
        let valkey = |variant: &str, floor: &str, size: u64| {
            artifact(EngineKey::Valkey, "9.2.0", Os::Linux, Arch::X86_64, size)
                .with_variant(variant)
                .with_min_glibc(floor)
        };
        let snapshot = CatalogSnapshot::new(
            "s2",
            SystemTime::UNIX_EPOCH,
            "https://example.test/catalog.json",
            1,
            vec![
                crate::model::EngineVersion::new(EngineKey::Valkey, "9.2.0", None, false)
                    .expect("version"),
            ],
            vec![valkey("noble", "2.38", 456), valkey("jammy", "2.34", 467)],
            CatalogStatus::Fresh,
        );
        let modern = build_catalog_view(
            &snapshot,
            Os::Linux,
            Arch::X86_64,
            &InstalledVersions::empty(),
            Some((2, 39)),
        );
        let old = build_catalog_view(
            &snapshot,
            Os::Linux,
            Arch::X86_64,
            &InstalledVersions::empty(),
            Some((2, 34)),
        );
        assert_eq!(modern.engines[3].versions[0].size_bytes, 456);
        assert_eq!(old.engines[3].versions[0].size_bytes, 467);
    }
}
