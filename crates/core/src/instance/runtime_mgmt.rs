//! Installed runtimes and archives (`F-RTM-04`, D-18).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::DblError;
use crate::error::Result;
use crate::model::{ArchivedData, Runtime};
use crate::paths::DblPaths;

use super::helpers::{blocking, normalize, remove_tree, safe_within};
use super::runtime_files;
use super::runtime_lock::RuntimeLock;
use super::service::Inner;

/// Installed runtimes with the space each takes on disk right now.
pub(crate) fn list_runtimes(inner: &Inner) -> Result<Vec<Runtime>> {
    let runtimes = inner.db(|store| store.list_runtimes())?;
    Ok(runtimes
        .into_iter()
        .map(|runtime| with_disk_size(&inner.paths, runtime))
        .collect())
}

fn with_disk_size(paths: &DblPaths, runtime: Runtime) -> Runtime {
    // A row pointing anywhere else keeps its recorded size: it is not walked.
    match removable_dir(paths, &runtime) {
        Ok(dir) => runtime.with_size_bytes(runtime_files::size_on_disk(&dir)),
        Err(_) => runtime,
    }
}

/// Removes one installed runtime (`F-RTM-04`).
///
/// Refused while an instance uses it. Safe against a create in this or another
/// process, and retryable: the row goes only once every file is gone.
pub(crate) async fn remove_runtime(inner: &Arc<Inner>, runtime_id: &str) -> Result<()> {
    let seen = find_runtime(inner, runtime_id)?;
    let Some(_lock) = RuntimeLock::try_exclusive(&inner.paths, seen.id())? else {
        // Name the users when they are why it cannot go; otherwise another
        // operation is installing or removing it right now.
        refuse_if_used(inner, &seen)?;
        return Err(DblError::RuntimeBusy {
            runtime: runtime_id.to_owned(),
        });
    };

    // Read again under the lock: another process may have created an instance, or
    // removed the runtime, since the look above.
    let target = find_runtime(inner, runtime_id)?;
    refuse_if_used(inner, &target)?;
    let dir = removable_dir(&inner.paths, &target)?;
    let files = Arc::clone(&inner.runtime_files);
    let doomed = dir.clone();
    blocking(move || runtime_files::discard(files.as_ref(), &doomed)).await?;
    inner.db(|store| store.remove_runtime(runtime_id))?;
    prune_version_dir(&dir);
    Ok(())
}

fn find_runtime(inner: &Inner, runtime_id: &str) -> Result<Runtime> {
    inner
        .db(|store| store.list_runtimes())?
        .into_iter()
        .find(|runtime| runtime.id() == runtime_id)
        .ok_or_else(|| DblError::NotFound {
            kind: "runtime",
            name: runtime_id.to_owned(),
        })
}

/// `F-RTM-04` AC2: refuse while any instance still uses this engine+version.
fn refuse_if_used(inner: &Inner, runtime: &Runtime) -> Result<()> {
    let users: Vec<String> = inner
        .db(|store| store.list_instances())?
        .into_iter()
        .filter(|instance| {
            instance.engine() == runtime.engine() && instance.version() == runtime.version()
        })
        .map(|instance| instance.name().as_str().to_owned())
        .collect();
    if users.is_empty() {
        Ok(())
    } else {
        Err(DblError::RuntimeInUse { users })
    }
}

/// The directory a removal may delete.
///
/// A row is data we wrote, so it is re-validated: only the install directory of
/// that very engine, version and architecture qualifies, reached without a link.
fn removable_dir(paths: &DblPaths, runtime: &Runtime) -> Result<PathBuf> {
    let refused = || DblError::UnsafeRuntimePath {
        path: runtime.path().display().to_string(),
    };
    let expected = paths
        .runtime_dir(runtime.engine(), runtime.version(), runtime.arch())
        .map_err(|_| refused())?;
    if normalize(runtime.path()) != normalize(&expected) {
        return Err(refused());
    }
    safe_within(paths, &expected).map_err(|_| refused())?;
    Ok(expected)
}

/// Drops the version directory once it is empty. The engine directory stays:
/// another version of it may be installing right now.
fn prune_version_dir(runtime_dir: &Path) {
    if let Some(version_dir) = runtime_dir.parent()
        && let Err(err) = std::fs::remove_dir(version_dir)
    {
        tracing::debug!(error = %err, "kept the runtime version directory");
    }
}

pub(crate) fn list_archives(inner: &Inner) -> Result<Vec<ArchivedData>> {
    inner.db(|store| store.list_archives())
}

/// Removes one archive record and its directory.
///
/// Only an id the service issued is accepted (it must exist in `archives`), and
/// the recorded path is confined to `archive/` after normalisation, so `..`,
/// absolute paths and symlinked rows cannot reach outside the home.
pub(crate) async fn purge_archive(inner: &Arc<Inner>, archive_id: &str) -> Result<()> {
    let archives = inner.db(|store| store.list_archives())?;
    let target = archives
        .into_iter()
        .find(|archive| archive.id() == archive_id)
        .ok_or_else(|| DblError::NotFound {
            kind: "archive",
            name: archive_id.to_owned(),
        })?;

    let archive_root = normalize(&inner.paths.archive_dir());
    let recorded = normalize(target.path());
    if !recorded.starts_with(&archive_root) || recorded == archive_root {
        return Err(DblError::UnsafeArchive {
            entry: target.path().display().to_string(),
        });
    }
    safe_within(&inner.paths, &recorded)?;

    // A symlink inside `archive/` is never something the service wrote; it could
    // point anywhere, so both its own metadata and its resolved target are
    // checked before anything is removed.
    let root_for_check = archive_root.clone();
    let path = recorded.clone();
    blocking(move || {
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && meta.file_type().is_symlink()
        {
            return Err(DblError::UnsafeArchive {
                entry: path.display().to_string(),
            });
        }
        if path.exists() {
            let resolved = path
                .canonicalize()
                .map_err(|err| DblError::Io { source: err })?;
            let root = root_for_check
                .canonicalize()
                .map_err(|err| DblError::Io { source: err })?;
            if !normalize(&resolved).starts_with(normalize(&root)) {
                return Err(DblError::UnsafeArchive {
                    entry: path.display().to_string(),
                });
            }
        }
        remove_tree(&path)
    })
    .await?;
    inner.db(|store| store.remove_archive(archive_id))
}
