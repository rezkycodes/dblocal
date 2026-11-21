//! Installed runtimes and archives (`F-RTM-04`, D-18).

use std::sync::Arc;

use crate::DblError;
use crate::error::Result;
use crate::model::{ArchivedData, Runtime};

use super::helpers::{blocking, normalize, remove_tree, safe_within};
use super::service::Inner;

pub(crate) fn list_runtimes(inner: &Inner) -> Result<Vec<Runtime>> {
    inner.db(|store| store.list_runtimes())
}

pub(crate) async fn remove_runtime(inner: &Arc<Inner>, runtime_id: &str) -> Result<()> {
    let runtimes = inner.db(|store| store.list_runtimes())?;
    let target = runtimes
        .iter()
        .find(|runtime| runtime.id() == runtime_id)
        .ok_or_else(|| DblError::NotFound {
            kind: "runtime",
            name: runtime_id.to_owned(),
        })?
        .clone();

    // `F-RTM-04` AC2: refuse while any instance still uses this engine+version.
    let users: Vec<String> = inner
        .db(|store| store.list_instances())?
        .into_iter()
        .filter(|instance| {
            instance.engine() == target.engine() && instance.version() == target.version()
        })
        .map(|instance| instance.name().as_str().to_owned())
        .collect();
    if !users.is_empty() {
        return Err(DblError::InvalidInput {
            field: "runtime",
            reason: format!("still used by: {}", users.join(", ")),
        });
    }

    // A row is data we wrote; re-validate before deleting what it points at.
    let path = target.path().to_path_buf();
    safe_within(&inner.paths, &path)?;
    blocking(move || remove_tree(&path)).await?;
    inner.db(|store| store.remove_runtime(runtime_id))
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
