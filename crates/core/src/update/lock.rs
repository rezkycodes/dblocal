//! Exclusive lock ensuring only one self-update runs at a time across GUI and CLI.
//!
//! Uses an atomic `create_new` lock file under `~/.dblocal/update/update.lock`
//! containing the owning PID, combined with an in-process guard. If a previous
//! updater crashed, a stale lock whose PID is no longer alive is reclaimed once.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::consts;

use super::error::{UpdateError, UpdateResult};

/// RAII guard for the exclusive update lock. Removes the lock file on drop.
#[derive(Debug)]
pub struct UpdateLock {
    path: PathBuf,
}

impl UpdateLock {
    /// Acquires the exclusive update lock inside `staging_dir`.
    ///
    /// Fails with [`UpdateError::InProgress`] if another live process (or task)
    /// already holds the lock.
    pub fn acquire(staging_dir: &Path) -> UpdateResult<Self> {
        std::fs::create_dir_all(staging_dir).map_err(|source| UpdateError::ApplyFailed {
            reason: format!("could not create update staging directory: {source}"),
        })?;
        let lock_path = staging_dir.join(consts::UPDATE_LOCK_FILE);
        match try_create_lock(&lock_path) {
            Ok(()) => Ok(Self { path: lock_path }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if is_stale_lock(&lock_path) {
                    let _ = std::fs::remove_file(&lock_path);
                    try_create_lock(&lock_path).map_err(|_| UpdateError::InProgress {
                        reason: "another update is already in progress".to_owned(),
                    })?;
                    return Ok(Self { path: lock_path });
                }
                Err(UpdateError::InProgress {
                    reason: "another update is already in progress".to_owned(),
                })
            }
            Err(source) => Err(UpdateError::ApplyFailed {
                reason: format!("could not acquire update lock: {source}"),
            }),
        }
    }

    /// Path to the lock file (for diagnostics/tests).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn try_create_lock(path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_all()?;
    Ok(())
}

/// Returns true only when the lock file records a PID that is definitely not
/// alive in the process table (and not our own PID).
fn is_stale_lock(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = contents.trim().parse::<u32>() else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        // Same process already holds the lock.
        return false;
    }
    let Ok(mut sys) = System::new() else {
        return false;
    };
    let target = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
    sys.process(target).is_none()
}
