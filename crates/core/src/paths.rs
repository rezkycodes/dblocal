//! The only module that knows the `~/.dblocal` layout.
//!
//! [`DblPaths`] is the root every other module must ask for a location.
//! Construct one with [`DblPaths::from_env`] in production or
//! [`DblPaths::with_root`] in tests.

use std::path::{Component, Path, PathBuf};

use crate::DblError;
use crate::consts;
use crate::error::Result;
use crate::model::{EngineKey, InstanceName};
use crate::platform::{Arch, Os};

/// Canonical locations under the DBLocal home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DblPaths {
    root: PathBuf,
}

impl DblPaths {
    /// Builds paths from `$DBLOCAL_HOME`, falling back to `~/.dblocal`.
    pub fn from_env() -> Result<Self> {
        Self::resolve(std::env::var_os(consts::ENV_DBLOCAL_HOME), home_dir())
    }

    /// Pure resolution step, split out so it can be tested without mutating
    /// process-wide environment state.
    fn resolve(override_home: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Result<Self> {
        if let Some(raw) = override_home.filter(|value| !value.is_empty()) {
            let root = PathBuf::from(raw);
            Self::ensure_within_base(&root, &root)?;
            return Ok(Self { root });
        }
        let home = home.ok_or_else(|| DblError::Unsupported {
            reason: "home directory could not be determined".to_owned(),
        })?;
        let root = home.join(consts::HOME_DIR_NAME);
        Self::ensure_within_base(&root, &home)?;
        Ok(Self { root })
    }

    /// Builds paths anchored at `root`, used by tests and isolated workers.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The home directory for everything DBLocal owns.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding every installed runtime.
    pub fn runtimes_dir(&self) -> PathBuf {
        self.root.join(consts::DIR_RUNTIMES)
    }

    /// Directory holding in-flight `.part` downloads, on the same filesystem as
    /// the runtimes so disk accounting stays predictable.
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join(consts::DIR_DOWNLOADS)
    }

    /// Directory holding one runtime: `runtimes/<engine>/<version>/<arch>/`.
    pub fn runtime_dir(&self, engine: EngineKey, version: &str, arch: Arch) -> Result<PathBuf> {
        self.checked_join(&[
            consts::DIR_RUNTIMES,
            engine.as_str(),
            version,
            arch.as_str(),
        ])
    }

    /// Lock file of one runtime: `locks/runtime-<id>.lock`. It lives outside
    /// `runtimes/` so removing the runtime never removes its lock.
    pub fn runtime_lock_file(&self, runtime_id: &str) -> Result<PathBuf> {
        self.checked_join(&[consts::DIR_LOCKS, &format!("runtime-{runtime_id}.lock")])
    }

    /// Directory holding every instance.
    pub fn instances_dir(&self) -> PathBuf {
        self.root.join(consts::DIR_INSTANCES)
    }

    /// Directory holding one instance: `instances/<name>/`.
    pub fn instance_dir(&self, name: &InstanceName) -> PathBuf {
        self.root.join(consts::DIR_INSTANCES).join(name.as_str())
    }

    /// Engine data directory for an instance.
    pub fn data_dir(&self, name: &InstanceName) -> PathBuf {
        self.instance_dir(name).join(consts::DIR_DATA)
    }

    /// Log directory for an instance.
    pub fn logs_dir(&self, name: &InstanceName) -> PathBuf {
        self.instance_dir(name).join(consts::DIR_LOGS)
    }

    /// Run directory (pidfile, socket) for an instance.
    pub fn run_dir(&self, name: &InstanceName) -> PathBuf {
        self.instance_dir(name).join(consts::DIR_RUN)
    }

    /// Directory where archived data directories are moved.
    pub fn archive_dir(&self) -> PathBuf {
        self.root.join(consts::DIR_ARCHIVE)
    }

    /// Directory that stores the credential file fallback.
    pub fn secrets_dir(&self) -> PathBuf {
        self.root.join(consts::DIR_SECRETS)
    }

    /// Path to the SQLite state database.
    pub fn state_db(&self) -> PathBuf {
        self.root.join(consts::STATE_DB_FILE)
    }

    /// Path to the application log.
    pub fn app_log(&self) -> PathBuf {
        self.root.join(consts::APP_LOG_FILE)
    }

    /// Relative runtime path under the root, useful for catalog matching.
    pub fn runtime_rel(&self, engine: EngineKey, version: &str, arch: Arch, os: Os) -> String {
        let _ = os; // the catalog distinguishes platforms, the install tree does not
        format!(
            "{}/{}/{}/{}",
            consts::DIR_RUNTIMES,
            engine.as_str(),
            version,
            arch.as_str()
        )
    }

    fn checked_join(&self, parts: &[&str]) -> Result<PathBuf> {
        for part in parts {
            validate_segment(part)?;
        }
        let mut path = self.root.clone();
        for part in parts {
            path.push(part);
        }
        Self::ensure_within_base(&path, &self.root)?;
        Ok(path)
    }

    fn ensure_within_base(candidate: &Path, base: &Path) -> Result<()> {
        // Lexical check: reject traversal and absolute children before touching
        // the filesystem. Symlink checks belong to the extraction layer.
        for component in candidate.components() {
            if matches!(component, Component::ParentDir) {
                return Err(DblError::UnsafeArchive {
                    entry: candidate.display().to_string(),
                });
            }
        }
        let base_norm = normalize(base);
        let cand_norm = normalize(candidate);
        if cand_norm.starts_with(&base_norm) || base_norm.as_os_str().is_empty() {
            Ok(())
        } else {
            Err(DblError::UnsafeArchive {
                entry: candidate.display().to_string(),
            })
        }
    }
}

fn validate_segment(segment: &str) -> Result<()> {
    if segment.is_empty() || segment == "." || segment == ".." {
        return Err(DblError::InvalidInput {
            field: "path",
            reason: "empty or relative path segment".to_owned(),
        });
    }
    let safe = segment
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if safe {
        Ok(())
    } else {
        Err(DblError::InvalidInput {
            field: "path",
            reason: format!("unsafe path segment: {segment}"),
        })
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(raw));
    }
    #[cfg(windows)]
    if let Some(raw) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(raw));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Os;
    use tempfile::tempdir;

    fn name(s: &str) -> InstanceName {
        InstanceName::new(s).expect("valid name")
    }

    #[test]
    fn layout_under_root_stays_inside_it() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        let instance = name("my-db");
        let root = dir.path();

        assert_eq!(paths.downloads_dir(), root.join("downloads"));

        let locations = vec![
            paths.runtimes_dir(),
            paths.downloads_dir(),
            paths
                .runtime_dir(EngineKey::Postgresql, "18.7", Arch::X86_64)
                .expect("ok"),
            paths.instances_dir(),
            paths.instance_dir(&instance),
            paths.data_dir(&instance),
            paths.logs_dir(&instance),
            paths.run_dir(&instance),
            paths.archive_dir(),
            paths.secrets_dir(),
            paths.state_db(),
            paths.app_log(),
        ];
        for loc in locations {
            assert!(loc.starts_with(root), "escaped root: {}", loc.display());
        }
        assert!(
            paths
                .runtime_rel(EngineKey::Postgresql, "18.7", Arch::X86_64, Os::Linux)
                .starts_with(consts::DIR_RUNTIMES)
        );
    }

    #[test]
    fn rejects_unsafe_runtime_segments() {
        let dir = tempdir().expect("tempdir");
        let paths = DblPaths::with_root(dir.path());
        assert!(
            paths
                .runtime_dir(EngineKey::Valkey, "../evil", Arch::X86_64)
                .is_err()
        );
        assert!(
            paths
                .runtime_dir(EngineKey::Valkey, "8.0/../../x", Arch::X86_64)
                .is_err()
        );
        assert!(
            paths
                .runtime_dir(EngineKey::Valkey, "", Arch::X86_64)
                .is_err()
        );
    }

    #[test]
    fn resolve_honours_dblocal_home() {
        let dir = tempdir().expect("tempdir");
        let override_home = Some(dir.path().as_os_str().to_owned());
        let paths = DblPaths::resolve(override_home, None).expect("resolve from override");
        assert_eq!(paths.root(), dir.path());
    }

    #[test]
    fn resolve_falls_back_to_home() {
        let home = tempdir().expect("tempdir");
        let paths = DblPaths::resolve(None, Some(home.path().to_owned())).expect("resolve");
        assert_eq!(paths.root(), home.path().join(consts::HOME_DIR_NAME));
        assert!(DblPaths::resolve(None, None).is_err());
    }

    #[test]
    fn with_root_is_exact() {
        let paths = DblPaths::with_root("/tmp/dblocal-test-root");
        assert_eq!(paths.root(), Path::new("/tmp/dblocal-test-root"));
        assert_eq!(
            paths.state_db(),
            Path::new("/tmp/dblocal-test-root/state.db")
        );
    }
}
