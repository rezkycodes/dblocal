//! Cross-process lock for one installed runtime (`F-RTM-04`).
//!
//! The GUI and the CLI are separate processes over one home, so "does anything
//! use this runtime?" followed by "delete it" must exclude "install it and create
//! an instance from it". A create holds the lock shared from before it looks for
//! the runtime until it has finished or rolled back; a removal holds it
//! exclusive. It is an OS advisory lock (the mechanism the self-updater uses), so
//! a process that crashes releases it by itself.

use std::fs::{File, OpenOptions, TryLockError};
use std::time::{Duration, Instant};

use crate::DblError;
use crate::consts;
use crate::download::CancellationToken;
use crate::error::Result;
use crate::model::EngineKey;
use crate::paths::DblPaths;
use crate::platform::Arch;

use super::helpers::{ensure_private_dir, safe_within};

/// The identifier of a runtime, shared by its store row and its lock file.
pub(crate) fn runtime_id(engine: EngineKey, version: &str, arch: Arch) -> String {
    format!("{}-{version}-{}", engine.as_str(), arch.as_str())
}

/// A held runtime lock. Closing the file releases it.
#[derive(Debug)]
pub(crate) struct RuntimeLock {
    _file: File,
}

impl RuntimeLock {
    /// The lock a removal needs, or `None` while anyone else holds the runtime.
    pub(crate) fn try_exclusive(paths: &DblPaths, runtime_id: &str) -> Result<Option<Self>> {
        Self::take(open_lock_file(paths, runtime_id)?, File::try_lock)
    }

    /// The lock a create needs, or `None` while a removal holds the runtime.
    fn try_shared(paths: &DblPaths, runtime_id: &str) -> Result<Option<Self>> {
        Self::take(open_lock_file(paths, runtime_id)?, File::try_lock_shared)
    }

    fn take(
        file: File,
        lock: fn(&File) -> std::result::Result<(), TryLockError>,
    ) -> Result<Option<Self>> {
        match lock(&file) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => Err(DblError::Io { source }),
        }
    }
}

fn open_lock_file(paths: &DblPaths, runtime_id: &str) -> Result<File> {
    let path = paths.runtime_lock_file(runtime_id)?;
    // Checked before anything is created: a link here would let the directory
    // setup below touch something outside the home.
    safe_within(paths, &path).map_err(|_| DblError::UnsafeRuntimePath {
        path: path.display().to_string(),
    })?;
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|source| DblError::Io { source })
}

/// Takes the shared lock for a create, waiting (cancellably, at most `wait`)
/// while a removal of the same runtime finishes.
pub(crate) async fn shared_for_create(
    paths: &DblPaths,
    runtime_id: &str,
    wait: Duration,
    cancel: &CancellationToken,
) -> Result<RuntimeLock> {
    let deadline = Instant::now() + wait;
    loop {
        if cancel.is_cancelled() {
            return Err(DblError::Cancelled {
                operation: "create".to_owned(),
            });
        }
        if let Some(lock) = RuntimeLock::try_shared(paths, runtime_id)? {
            return Ok(lock);
        }
        if Instant::now() >= deadline {
            return Err(DblError::RuntimeBusy {
                runtime: runtime_id.to_owned(),
            });
        }
        tokio::time::sleep(Duration::from_millis(consts::RUNTIME_LOCK_POLL_MS)).await;
    }
}
