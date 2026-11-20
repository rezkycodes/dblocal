//! Installed runtimes and download progress.

use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use crate::DblError;
use crate::model::EngineKey;
use crate::platform::Arch;

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
    fn runtime_accessors() {
        let runtime = Runtime::new(
            "r1",
            EngineKey::Mariadb,
            "11.4",
            Arch::Aarch64,
            PathBuf::from("/tmp/mariadb"),
            10,
            SystemTime::UNIX_EPOCH,
            RuntimeState::Installed,
        );
        assert_eq!(runtime.id(), "r1");
        assert_eq!(runtime.engine(), EngineKey::Mariadb);
        assert_eq!(runtime.version(), "11.4");
        assert_eq!(runtime.arch(), Arch::Aarch64);
        assert_eq!(runtime.path(), std::path::Path::new("/tmp/mariadb"));
        assert_eq!(runtime.size_bytes(), 10);
        assert_eq!(runtime.installed_at(), SystemTime::UNIX_EPOCH);
        assert_eq!(runtime.state(), RuntimeState::Installed);
        assert_eq!(RuntimeState::Installed.as_str(), "installed");
        assert!("new".parse::<RuntimeState>().is_err());
    }
}
