//! Temp-home isolation and the panic-safe cleanup guard.
//!
//! Both `DBLOCAL_HOME` and `HOME` point into one temp directory, so nothing the
//! suite does can reach the developer's `~/.dblocal`, keychain, or any file
//! outside the temp tree. Credentials land in the `0600` file backend under that
//! temp home because the keychain is never asked (no D-Bus access is set up).
//!
//! Env mutation is process-global, so tests that mutate it hold a global lock.
//! `cargo test` runs tests in one binary on several threads; every test here
//! takes the lock for its whole body.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use dblocal_core::paths::DblPaths;
use tempfile::TempDir;

use super::proc;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// A temp home with `HOME`/`DBLOCAL_HOME` pointed inside it.
pub struct TempHome {
    dir: TempDir,
    previous_home: Option<OsString>,
    previous_dblocal_home: Option<OsString>,
    /// Held for the whole test so no two env-mutating tests overlap.
    _env: MutexGuard<'static, ()>,
}

impl TempHome {
    /// Creates the temp home and points the environment at it.
    pub fn new(prefix: &str) -> Self {
        // Capture the real home before redirecting it, so the artifact cache in
        // `~/.cache/dblocal-dev` stays reachable from inside a temp home.
        super::cache::capture_real_home();
        let env = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("create the temp home");
        let previous_home = std::env::var_os("HOME");
        let previous_dblocal_home = std::env::var_os("DBLOCAL_HOME");
        // SAFETY: this binary serialises every env-mutating test behind
        // `ENV_LOCK`, and no other thread in this process reads these variables.
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
}

impl Drop for TempHome {
    fn drop(&mut self) {
        // Terminate every engine the test started, by validated command line.
        proc::kill_under(self.dir.path());
        proc::wait_none_under(self.dir.path(), std::time::Duration::from_secs(10));
        // SAFETY: same lock serialisation as `new`.
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

/// Sorted direct children of `dir`, for the "nothing outside the home" check.
pub fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}
