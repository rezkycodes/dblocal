//! Catalog entries: released versions and the snapshot that lists them.

use std::time::{Duration, SystemTime};

use std::fmt;

use crate::model::artifact::Artifact;
use crate::model::{EngineKey, major_line_of};
use crate::platform::{Arch, Os};

/// Support status a version line advertises in the picker.
///
/// EOL is not a variant here: it stays the `eol` boolean every existing reader
/// already understands, so a version can be both `Lts` and `Eol` only through
/// the two separate fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VersionStatus {
    /// Long-term support line.
    Lts,
    /// MySQL Innovation release.
    Innovation,
    /// MariaDB rolling stable release.
    Rolling,
}

impl VersionStatus {
    /// Stable identifier used in the catalog JSON and the UI badge.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lts => "lts",
            Self::Innovation => "innovation",
            Self::Rolling => "rolling",
        }
    }
}

impl fmt::Display for VersionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for VersionStatus {
    type Err = crate::DblError;

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "lts" => Ok(Self::Lts),
            "innovation" => Ok(Self::Innovation),
            "rolling" => Ok(Self::Rolling),
            other => Err(crate::DblError::InvalidInput {
                field: "status",
                reason: format!(
                    "unknown version status {other:?}, expected lts, innovation or rolling"
                ),
            }),
        }
    }
}

/// The release line a version belongs to, for grouping in the picker.
///
/// PostgreSQL tags are `MAJOR.MINOR.PATCH` where `MAJOR` is the release line
/// (`18.7.0` belongs to `18`); MySQL, MariaDB and Valkey use `MAJOR.MINOR` as
/// the line and the last component as the patch (`8.4.13` belongs to `8.4`).
pub fn version_line(engine: EngineKey, tag: &str) -> String {
    let mut parts = tag.split('.');
    let major = parts.next().unwrap_or(tag);
    match engine {
        EngineKey::Postgresql => major.to_owned(),
        _ => match parts.next() {
            Some(minor) => format!("{major}.{minor}"),
            None => major.to_owned(),
        },
    }
}

/// One released version of one engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineVersion {
    engine: EngineKey,
    version: String,
    major_line: String,
    line: String,
    release_date: Option<String>,
    eol: bool,
    status: Option<VersionStatus>,
    license: Option<String>,
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
        let line = version_line(engine, &version);
        Ok(Self {
            engine,
            version,
            major_line,
            line,
            release_date,
            eol,
            status: None,
            license: None,
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
    ///
    /// Kept as the first tag component so existing catalog readers (v0.1.x)
    /// keep matching it; see [`EngineVersion::line`] for the picker grouping.
    pub fn major_line(&self) -> &str {
        &self.major_line
    }

    /// Release line the picker groups by, for example `18` or `8.4`.
    pub fn line(&self) -> &str {
        &self.line
    }

    /// Advertised support status, if the catalog names one.
    pub const fn status(&self) -> Option<VersionStatus> {
        self.status
    }

    /// Returns a copy carrying a support status.
    pub fn with_status(mut self, status: Option<VersionStatus>) -> Self {
        self.status = status;
        self
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

    /// Upstream license expression, if the catalog records one.
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Returns a copy carrying a license expression.
    pub fn with_license(mut self, license: Option<String>) -> Self {
        self.license = license;
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
    fn line_is_the_major_for_postgres_and_major_minor_for_the_others() {
        // PostgreSQL: MAJOR.MINOR.PATCH, line is the major.
        assert_eq!(version_line(EngineKey::Postgresql, "18.7.0"), "18");
        assert_eq!(version_line(EngineKey::Postgresql, "19.0.0"), "19");
        assert_eq!(version_line(EngineKey::Postgresql, "14.25.0"), "14");
        // MySQL/MariaDB/Valkey: MAJOR.MINOR.PATCH, line keeps both.
        assert_eq!(version_line(EngineKey::Mysql, "8.4.13"), "8.4");
        assert_eq!(version_line(EngineKey::Mysql, "26.10.0"), "26.10");
        assert_eq!(version_line(EngineKey::Mariadb, "12.3.4"), "12.3");
        assert_eq!(version_line(EngineKey::Valkey, "9.2.0"), "9.2");
        // A bare line is its own line.
        assert_eq!(version_line(EngineKey::Mysql, "8.4"), "8.4");
    }

    #[test]
    fn engine_version_carries_line_and_status() {
        let version = EngineVersion::new(EngineKey::Mysql, "9.7.4", None, false)
            .expect("version")
            .with_status(Some(VersionStatus::Lts));
        assert_eq!(version.line(), "9.7");
        assert_eq!(version.major_line(), "9");
        assert_eq!(version.status(), Some(VersionStatus::Lts));
    }

    #[test]
    fn status_strings_round_trip() {
        for status in [
            VersionStatus::Lts,
            VersionStatus::Innovation,
            VersionStatus::Rolling,
        ] {
            assert_eq!(status.as_str().parse::<VersionStatus>().ok(), Some(status));
            assert_eq!(status.to_string(), status.as_str());
        }
        assert!("".parse::<VersionStatus>().is_err());
        assert!("beta".parse::<VersionStatus>().is_err());
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
