//! Catalog, runtime and download value types.

use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use crate::DblError;
use crate::model::{EngineKey, major_line_of};
use crate::platform::{Arch, Os};

/// Packaging of a published engine artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArchiveFormat {
    /// `.tar.gz`.
    TarGz,
    /// `.tar.xz`.
    TarXz,
    /// `.zip`.
    Zip,
}

impl ArchiveFormat {
    /// Identifier used in the catalog JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::Zip => "zip",
        }
    }
}

impl FromStr for ArchiveFormat {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self, DblError> {
        match value {
            "tar.gz" => Ok(Self::TarGz),
            "tar.xz" => Ok(Self::TarXz),
            "zip" => Ok(Self::Zip),
            other => Err(DblError::InvalidInput {
                field: "format",
                reason: format!("unknown archive format: {other}"),
            }),
        }
    }
}

/// One downloadable engine build for one `(os, arch)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    id: String,
    engine: EngineKey,
    version: String,
    os: Os,
    arch: Arch,
    url: String,
    sha256: String,
    size_bytes: u64,
    format: ArchiveFormat,
    strip_prefix: Option<String>,
    min_glibc: Option<String>,
}

impl Artifact {
    /// Builds an artifact, rejecting entries that cannot be installed.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        engine: EngineKey,
        version: impl Into<String>,
        os: Os,
        arch: Arch,
        url: impl Into<String>,
        sha256: impl Into<String>,
        size_bytes: u64,
        format: ArchiveFormat,
    ) -> crate::Result<Self> {
        let sha256 = sha256.into();
        if !is_sha256_hex(&sha256) {
            return Err(DblError::InvalidInput {
                field: "sha256",
                reason: "must be 64 lowercase hex characters".to_owned(),
            });
        }
        Ok(Self {
            id: id.into(),
            engine,
            version: version.into(),
            os,
            arch,
            url: url.into(),
            sha256,
            size_bytes,
            format,
            strip_prefix: None,
            min_glibc: None,
        })
    }

    /// Artifact identifier.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Engine the artifact unpacks.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Target OS.
    pub const fn os(&self) -> Os {
        self.os
    }
    /// Target architecture.
    pub const fn arch(&self) -> Arch {
        self.arch
    }
    /// Download URL.
    pub fn url(&self) -> &str {
        &self.url
    }
    /// Expected SHA-256, lowercase hex.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    /// Expected size in bytes, used for progress and free-space checks.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    /// Archive format.
    pub const fn format(&self) -> ArchiveFormat {
        self.format
    }
    /// Directory to strip while extracting.
    pub fn strip_prefix(&self) -> Option<&str> {
        self.strip_prefix.as_deref()
    }
    /// Minimum glibc version required on Linux.
    pub fn min_glibc(&self) -> Option<&str> {
        self.min_glibc.as_deref()
    }

    /// Returns a copy with a strip prefix.
    pub fn with_strip_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.strip_prefix = Some(prefix.into());
        self
    }

    /// Returns a copy with a minimum glibc requirement.
    pub fn with_min_glibc(mut self, version: impl Into<String>) -> Self {
        self.min_glibc = Some(version.into());
        self
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

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

/// Whether an installed runtime is usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RuntimeState {
    /// Binaries present and checksum history clean.
    Installed,
    /// Files missing or corrupt; reinstall required.
    Broken,
}

impl RuntimeState {
    /// Stable identifier used in SQLite.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Broken => "broken",
        }
    }
}

impl FromStr for RuntimeState {
    type Err = DblError;

    fn from_str(value: &str) -> Result<Self, DblError> {
        match value {
            "installed" => Ok(Self::Installed),
            "broken" => Ok(Self::Broken),
            other => Err(DblError::InvalidInput {
                field: "runtime_state",
                reason: format!("unknown state: {other}"),
            }),
        }
    }
}

/// An installed engine runtime shared by every instance of that version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    id: String,
    engine: EngineKey,
    version: String,
    arch: Arch,
    path: PathBuf,
    size_bytes: u64,
    installed_at: SystemTime,
    state: RuntimeState,
}

impl Runtime {
    /// Creates a runtime record.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        engine: EngineKey,
        version: impl Into<String>,
        arch: Arch,
        path: PathBuf,
        size_bytes: u64,
        installed_at: SystemTime,
        state: RuntimeState,
    ) -> Self {
        Self {
            id: id.into(),
            engine,
            version: version.into(),
            arch,
            path,
            size_bytes,
            installed_at,
            state,
        }
    }

    /// Runtime identifier.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Engine the runtime unpacks.
    pub const fn engine(&self) -> EngineKey {
        self.engine
    }
    /// Version tag.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Architecture.
    pub const fn arch(&self) -> Arch {
        self.arch
    }
    /// Install directory, below `runtimes/`.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    /// Installed size in bytes.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    /// When it was installed.
    pub const fn installed_at(&self) -> SystemTime {
        self.installed_at
    }
    /// Usability.
    pub const fn state(&self) -> RuntimeState {
        self.state
    }

    /// Returns a copy with a new state.
    pub fn with_state(mut self, state: RuntimeState) -> Self {
        self.state = state;
        self
    }

    /// Whether instances of this engine and version can reuse it.
    pub fn matches(&self, engine: EngineKey, version: &str) -> bool {
        self.engine == engine && self.version == version && self.state == RuntimeState::Installed
    }
}

/// Progress of an in-flight download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Bytes written so far.
    pub bytes_done: u64,
    /// Total bytes expected, from the catalog.
    pub bytes_total: u64,
    /// Current throughput, bytes per second.
    pub bytes_per_sec: u64,
    /// Estimated time remaining.
    pub eta: Option<Duration>,
}

impl DownloadProgress {
    /// Builds a progress sample.
    pub const fn new(
        bytes_done: u64,
        bytes_total: u64,
        bytes_per_sec: u64,
        eta: Option<Duration>,
    ) -> Self {
        Self {
            bytes_done,
            bytes_total,
            bytes_per_sec,
            eta,
        }
    }

    /// Completion from 0.0 to 1.0, clamped when totals are unknown.
    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            0.0
        } else {
            (self.bytes_done as f64 / self.bytes_total as f64).clamp(0.0, 1.0) as f32
        }
    }

    /// Whether every expected byte has arrived.
    pub const fn is_complete(&self) -> bool {
        self.bytes_total > 0 && self.bytes_done >= self.bytes_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EngineVersion as Version, InstanceId, Port, PortAllocation};

    fn sha() -> String {
        "a".repeat(64)
    }

    #[test]
    fn artifact_rejects_bad_checksum() {
        let bad = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg.tar.gz",
            "not-a-hash",
            10,
            ArchiveFormat::TarGz,
        );
        assert!(bad.is_err());

        let ok = Artifact::new(
            "a1",
            EngineKey::Postgresql,
            "18.7",
            Os::Linux,
            Arch::X86_64,
            "https://example.test/pg.tar.gz",
            sha(),
            10,
            ArchiveFormat::TarGz,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn archive_formats_round_trip() {
        for format in [
            ArchiveFormat::TarGz,
            ArchiveFormat::TarXz,
            ArchiveFormat::Zip,
        ] {
            assert_eq!(format.as_str().parse::<ArchiveFormat>().ok(), Some(format));
        }
        assert!("rar".parse::<ArchiveFormat>().is_err());
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
            ArchiveFormat::TarGz,
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
    fn runtime_matches_only_when_installed() {
        let runtime = Runtime::new(
            "r1",
            EngineKey::Mysql,
            "8.0.40",
            Arch::X86_64,
            PathBuf::from("/tmp/mysql"),
            10,
            SystemTime::UNIX_EPOCH,
            RuntimeState::Installed,
        );
        assert!(runtime.matches(EngineKey::Mysql, "8.0.40"));
        assert!(!runtime.matches(EngineKey::Mariadb, "8.0.40"));
        assert!(
            !runtime
                .clone()
                .with_state(RuntimeState::Broken)
                .matches(EngineKey::Mysql, "8.0.40")
        );
        assert_eq!(
            "broken".parse::<RuntimeState>().ok(),
            Some(RuntimeState::Broken)
        );
    }

    #[test]
    fn download_progress_is_clamped() {
        let half = DownloadProgress::new(50, 100, 10, Some(Duration::from_secs(5)));
        assert!((half.fraction() - 0.5).abs() < f32::EPSILON);
        assert!(!half.is_complete());
        assert!(DownloadProgress::new(100, 100, 0, None).is_complete());
        assert_eq!(DownloadProgress::new(10, 0, 0, None).fraction(), 0.0);
        assert_eq!(
            DownloadProgress::new(500, 100, 0, None).fraction(),
            1.0,
            "fraction must clamp above 1.0"
        );
    }

    #[test]
    fn port_allocation_keeps_its_owner() {
        let id = InstanceId::new();
        let allocation = PortAllocation::new(
            Port::new(5432).expect("valid port"),
            id.clone(),
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(allocation.port().get(), 5432);
        assert_eq!(allocation.instance_id(), &id);
    }
}
