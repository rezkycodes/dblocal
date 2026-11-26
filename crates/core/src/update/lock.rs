//! Exclusive OS advisory lock ensuring only one self-update runs at a time
//! across GUI and CLI processes.
//!
//! Uses `std::fs::File::try_lock()` (flock on Unix, LockFileEx on Windows),
//! which the OS kernel releases automatically when the owning process exits or
//! crashes — avoiding stale PID files, empty/corrupt lock files, and
//! two-process reclaim races. An in-process set guards against same-process
//! re-entrancy.

use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::consts;

use super::error::{UpdateError, UpdateResult};

fn in_process_locks() -> &'static Mutex<HashSet<PathBuf>> {
    static LOCKS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII guard for the exclusive update lock. Releases the OS advisory lock on drop.
#[derive(Debug)]
pub struct UpdateLock {
    path: PathBuf,
    canonical: PathBuf,
    _file: File,
}

impl UpdateLock {
    /// Acquires the exclusive update lock inside `staging_dir`.
    ///
    /// Fails with [`UpdateError::InProgress`] if another live process or task
    /// already holds the lock.
    pub fn acquire(staging_dir: &Path) -> UpdateResult<Self> {
        std::fs::create_dir_all(staging_dir).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not create update staging directory: {source}"),
        })?;
        let lock_path = staging_dir.join(consts::UPDATE_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| UpdateError::ApplyFailed {
                reason: format!("could not open update lock file: {source}"),
            })?;

        let canonical = std::fs::canonicalize(&lock_path).unwrap_or_else(|_| lock_path.clone());
        {
            let mut held = in_process_locks().lock().unwrap_or_else(|e| e.into_inner());
            if !held.insert(canonical.clone()) {
                return Err(UpdateError::InProgress {
                    reason: "another update is already in progress".to_owned(),
                });
            }
        }

        match file.try_lock() {
            Ok(()) => Ok(Self {
                path: lock_path,
                canonical,
                _file: file,
            }),
            Err(TryLockError::WouldBlock) => {
                release_in_process(&canonical);
                Err(UpdateError::InProgress {
                    reason: "another update is already in progress".to_owned(),
                })
            }
            Err(TryLockError::Error(source)) => {
                release_in_process(&canonical);
                Err(UpdateError::ApplyFailed {
                    reason: format!("could not acquire update lock: {source}"),
                })
            }
        }
    }

    /// Path to the lock file (for diagnostics/tests).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
        release_in_process(&self.canonical);
    }
}

fn release_in_process(canonical: &Path) {
    if let Ok(mut held) = in_process_locks().lock() {
        held.remove(canonical);
    }
}
