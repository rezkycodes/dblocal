//! A temp home with `HOME`/`DBLOCAL_HOME` pointed inside it, and a panic-safe
//! process guard.
//!
//! Identical in spirit to `tests/support/env.rs`, but a separate copy so this
//! task never edits a file another worker owns. Env mutation is process-global,
//! so every test body that mutates it holds one global mutex for its lifetime.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use dblocal_core::paths::DblPaths;
use tempfile::TempDir;

use super::cluster;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// The developer's real `HOME`, captured before any test redirects it, so the
/// artifact cache in `~/.cache/dblocal-dev` stays reachable.
static REAL_HOME: OnceLock<PathBuf> = OnceLock::new();

/// The real home as captured at the first [`TempHome::new`].
pub fn real_home() -> PathBuf {
    REAL_HOME
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// A temp home that owns `HOME`/`DBLOCAL_HOME` for the test's lifetime.
pub struct TempHome {
    dir: TempDir,
    previous_home: Option<OsString>,
    previous_dblocal_home: Option<OsString>,
    _env: MutexGuard<'static, ()>,
}

impl TempHome {
    /// Creates the temp home and points the environment at it.
    pub fn new(prefix: &str) -> Self {
        if let Some(home) = std::env::var_os("HOME") {
            let _ = REAL_HOME.set(PathBuf::from(home));
        }
        let env = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("create the temp home");
        let previous_home = std::env::var_os("HOME");
        let previous_dblocal_home = std::env::var_os("DBLOCAL_HOME");
        // SAFETY: this binary serialises every env-mutating test behind the one
        // global lock, and no other thread in the process reads these vars.
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("DBLOCAL_HOME", dir.path());
        }
        Self {
            dir,
            previous_home,
            previous_dblocal_home,
            _env: env,
        }
    }

    /// Root of the temp home (also `DBLOCAL_HOME`).
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// DBLocal state paths rooted at the temp home.
    pub fn paths(&self) -> DblPaths {
        DblPaths::with_root(self.dir.path())
    }

    /// The directory the extracted PostgreSQL runtime is placed under.
    pub fn runtime_dir(&self, version: &str) -> PathBuf {
        self.paths()
            .runtime_dir(
                dblocal_core::model::EngineKey::Postgresql,
                version,
                dblocal_core::platform::Arch::current(),
            )
            .expect("runtime dir under the temp home")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        cluster::kill_postgres_under(self.dir.path());
        cluster::wait_none_under(self.dir.path(), std::time::Duration::from_secs(10));
        // SAFETY: same lock held for the whole lifetime.
        unsafe {
            match &self.previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.previous_dblocal_home {
                Some(value) => std::env::set_var("DBLOCAL_HOME", value),
                None => std::env::remove_var("DBLOCAL_HOME"),
            }
        }
    }
}
