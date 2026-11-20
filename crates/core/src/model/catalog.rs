//! Catalog entries: released versions and the snapshot that lists them.

use std::time::{Duration, SystemTime};

use crate::model::artifact::Artifact;
use crate::model::{EngineKey, major_line_of};
use crate::platform::{Arch, Os};

/// One released version of one engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineVersion {
    engine: EngineKey,
    version: String,
    major_line: String,
    release_date: Option<String>,
    eol: bool,
}

impl EngineVersion {
    /// Creates a version entry and derives its major line.
    pub fn new(
        engine: EngineKey,
        version: impl Into<String>,
        release_date: Option<String>,
        eol: bool,
    ) -> crate::Result<Self> {
        let version = version.into();
        super::validate_version_tag("version", &version)?;
        let major_line = major_line_of(&version);
        Ok(Self {
            engine,
            version,
            major_line,
            release_date,
            eol,
        })
    }

    /// Engine this version belongs to.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Major line, for example `18`.
    pub fn major_line(&self) -> &str {
        &self.major_line
    }
    /// Upstream release date, if known.
    pub fn release_date(&self) -> Option<&str> {
        self.release_date.as_deref()
    }
    /// Whether upstream no longer supports this version.
    pub const fn is_eol(&self) -> bool {
        self.eol
    }

    /// Returns a copy with the EOL flag changed.
    pub fn with_eol(mut self, eol: bool) -> Self {
        self.eol = eol;
        self
    }
}

/// Freshness of the catalog snapshot in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CatalogStatus {
    /// Refreshed within the TTL.
    Fresh,
    /// Older than the TTL, still usable.
    Stale,
    /// No network and no snapshot newer than what is bundled.
    Offline,
}

impl CatalogStatus {
    /// Stable identifier used in the UI state.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Offline => "offline",
        }
    }
}

/// A cached catalog payload, derived data and never the source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSnapshot {
    id: String,
    fetched_at: SystemTime,
    etag: Option<String>,
    source_url: String,
    schema_version: u32,
    versions: Vec<EngineVersion>,
    artifacts: Vec<Artifact>,
    status: CatalogStatus,
}

impl CatalogSnapshot {
    /// Creates a snapshot.
    pub fn new(
        id: impl Into<String>,
        fetched_at: SystemTime,
        source_url: impl Into<String>,
        schema_version: u32,
        versions: Vec<EngineVersion>,
        artifacts: Vec<Artifact>,
        status: CatalogStatus,
    ) -> Self {
        Self {
            id: id.into(),
            fetched_at,
            etag: None,
            source_url: source_url.into(),
            schema_version,
            versions,
            artifacts,
            status,
        }
    }

    /// Snapshot identifier.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// When the snapshot was fetched.
    pub const fn fetched_at(&self) -> SystemTime {
        self.fetched_at
    }
    /// HTTP ETag of the last fetch.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }
    /// Where the snapshot came from.
    pub fn source_url(&self) -> &str {
        &self.source_url
    }
    /// Payload schema version.
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }
    /// Versions listed in this snapshot.
    pub fn versions(&self) -> &[EngineVersion] {
        &self.versions
    }
    /// Artifacts listed in this snapshot.
    pub fn artifacts(&self) -> &[Artifact] {
        &self.artifacts
    }
    /// Freshness of the snapshot.
    pub const fn status(&self) -> CatalogStatus {
        self.status
    }

    /// Returns a copy with a different freshness.
    pub fn with_status(mut self, status: CatalogStatus) -> Self {
        self.status = status;
        self
    }

    /// Returns a copy with an ETag.
    pub fn with_etag(mut self, etag: impl Into<String>) -> Self {
        self.etag = Some(etag.into());
        self
    }

    /// Artifacts that can actually run on `os`/`arch`.
    pub fn artifacts_for(&self, os: Os, arch: Arch) -> Vec<&Artifact> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.os() == os && artifact.arch() == arch)
            .collect()
    }

    /// Whether the snapshot is older than `ttl` at `now`.
    pub fn is_stale_at(&self, now: SystemTime, ttl: Duration) -> bool {
        match now.duration_since(self.fetched_at) {
            Ok(age) => age > ttl,
            Err(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EngineVersion as Version;

    fn sha() -> String {
        "a".repeat(64)
    }

    #[test]
    fn engine_version_derives_major_line() {
        let version = EngineVersion::new(EngineKey::Postgresql, "18.7.0", None, false)
            .expect("valid version");
        assert_eq!(version.major_line(), "18");
        assert!(!version.is_eol());
        assert!(version.clone().with_eol(true).is_eol());
    }

    #[test]
    fn snapshot_filters_and_ages() {
        let artifact = Artifact::new(
            "a1",
            EngineKey::Valkey,
            "8.0",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/valkey.tar.gz",
            sha(),
            10,
            crate::model::ArchiveFormat::TarGz,
        )
        .expect("valid artifact");
        let version = Version::new(EngineKey::Valkey, "8.0", None, false).expect("valid version");
        let snapshot = CatalogSnapshot::new(
            "s1",
            SystemTime::UNIX_EPOCH,
            "https://example.test/catalog.json",
            1,
            vec![version],
            vec![artifact],
            CatalogStatus::Fresh,
        )
        .with_etag("\"abc\"");

        assert_eq!(snapshot.artifacts_for(Os::Linux, Arch::X86_64).len(), 1);
        assert_eq!(snapshot.artifacts_for(Os::Windows, Arch::X86_64).len(), 0);
        assert!(!snapshot.is_stale_at(SystemTime::UNIX_EPOCH, Duration::from_secs(1)));
        assert!(snapshot.is_stale_at(
            SystemTime::UNIX_EPOCH + Duration::from_secs(86_400),
            Duration::from_secs(3_600)
        ));
        assert_eq!(snapshot.status().as_str(), "fresh");
        assert_eq!(snapshot.etag(), Some("\"abc\""));
    }

    #[test]
    fn snapshot_accessors_and_mutation() {
        let artifact = Artifact::new(
            "a1",
            EngineKey::Mysql,
            "8.0.40",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/mysql.tar.gz",
            sha(),
            10,
            crate::model::ArchiveFormat::TarGz,
        )
        .expect("valid artifact");
        let version = Version::new(EngineKey::Mysql, "8.0.40", Some("2025-01-01".into()), false)
            .expect("valid version");
        assert_eq!(version.engine(), EngineKey::Mysql);
        assert_eq!(version.version(), "8.0.40");
        assert_eq!(version.release_date(), Some("2025-01-01"));

        let snapshot = CatalogSnapshot::new(
            "s1",
            SystemTime::UNIX_EPOCH,
            "https://example.test/catalog.json",
            1,
            vec![version],
            vec![artifact],
            CatalogStatus::Stale,
        );
        assert_eq!(snapshot.id(), "s1");
        assert_eq!(snapshot.fetched_at(), SystemTime::UNIX_EPOCH);
        assert!(snapshot.etag().is_none());
        assert_eq!(snapshot.source_url(), "https://example.test/catalog.json");
        assert_eq!(snapshot.schema_version(), 1);
        assert_eq!(snapshot.versions().len(), 1);
        assert_eq!(snapshot.artifacts().len(), 1);
        assert_eq!(snapshot.status(), CatalogStatus::Stale);
        assert_eq!(
            snapshot
                .clone()
                .with_status(CatalogStatus::Offline)
                .status(),
            CatalogStatus::Offline
        );
        assert_eq!(CatalogStatus::Offline.as_str(), "offline");

        // An age equal to the TTL is not stale; larger ages are.
        assert!(!snapshot.is_stale_at(SystemTime::UNIX_EPOCH, Duration::from_secs(0)));
        assert!(snapshot.clone().is_stale_at(
            SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            Duration::from_secs(1)
        ));
    }
}
