//! A cross-process lock for one toolchain install/remove.
//!
//! The CLI and the GUI are separate processes over one home, so "install this
//! version" and "remove it" must not run at the same time. This is the same OS
//! advisory lock the engine runtime uses ([`std::fs::File::try_lock`]): a crashed
//! process releases it automatically, so there is no stale PID file to clean up.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::DblError;
use crate::consts as crate_consts;
use crate::fsutil; // used for private dir creation
use crate::toolchain::consts;

/// A held toolchain lock. Dropping it (closing the file) releases the lock.
#[derive(Debug)]
pub struct ToolchainLock {
    _file: File,
}

impl ToolchainLock {
    /// Tries once to take the exclusive lock, returning `None` while another
    /// process holds it.
    pub fn try_exclusive(path: &Path) -> crate::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            fsutil::ensure_private_dir(parent).map_err(|source| DblError::Io { source })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|source| DblError::Io { source })?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => Err(DblError::Io { source }),
        }
    }

    /// Takes the exclusive lock, waiting (at most `wait`, polling every
    /// [`consts::LOCK_POLL_MS`]) while another operation holds it.
    pub fn acquire(path: &Path, wait: Duration) -> crate::Result<Self> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(lock) = Self::try_exclusive(path)? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                return Err(DblError::RuntimeBusy {
                    runtime: path.display().to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(consts::LOCK_POLL_MS));
        }
    }

    /// The default wait used by the CLI: [`consts::LOCK_WAIT_SECS`].
    pub fn acquire_default(path: &Path) -> crate::Result<Self> {
        Self::acquire(
            path,
            Duration::from_secs(crate_consts::RUNTIME_LOCK_WAIT_SECS.max(consts::LOCK_WAIT_SECS)),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn second_lock_in_the_same_process_is_refused() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("x.lock");
        let first = ToolchainLock::acquire_default(&path).expect("first");
        // Same process, second handle: flock is per-file-description, so a second
        // open of the same path is a distinct file description and is refused.
        let second = ToolchainLock::try_exclusive(&path).expect("try");
        assert!(second.is_none(), "second exclusive lock must be refused");
        drop(first);
        // After release, the lock is available again.
        assert!(ToolchainLock::try_exclusive(&path).expect("try").is_some());
    }
}
