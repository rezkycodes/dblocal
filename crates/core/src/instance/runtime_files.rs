//! The disk side of removing a runtime (`F-RTM-04`).
//!
//! The install directory is first renamed to a sibling, which either happens or
//! does not: a file another program holds open (Windows) or a missing permission
//! then leaves the runtime intact. A deletion that stops half way leaves nothing
//! at the install path, so the next create can always install again, and the
//! leftovers are swept by the next attempt.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::DblError;
use crate::download::STAGING_MARKER;
use crate::error::Result;

use super::helpers::dir_size;

/// Marks a directory set aside for deletion: `.<name>.removing-<nonce>`.
const REMOVING_MARKER: &str = ".removing-";

/// The two file operations a removal is made of, replaceable in tests so that a
/// locked file can be simulated on every platform.
pub(crate) trait RuntimeFiles: Send + Sync {
    /// Renames the install directory out of the way: all of it or nothing.
    fn set_aside(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Deletes a set-aside tree or file. Already gone counts as deleted.
    fn delete(&self, path: &Path) -> io::Result<()>;
}

/// The real filesystem.
pub(crate) struct DiskRuntimeFiles;

impl RuntimeFiles for DiskRuntimeFiles {
    fn set_aside(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn delete(&self, path: &Path) -> io::Result<()> {
        let deleted = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(err) => Err(err),
        };
        match deleted {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

/// Deletes the runtime at `dir`, with whatever an earlier attempt or a crashed
/// install left beside it.
///
/// The caller has validated `dir` and holds the runtime's exclusive lock.
pub(crate) fn discard(files: &dyn RuntimeFiles, dir: &Path) -> Result<()> {
    set_aside(files, dir)?;
    // Every leftover is tried, so one locked file does not keep the rest on disk.
    let mut first_failure = None;
    for leftover in leftovers(dir).map_err(|err| failure(dir, &err))? {
        if let Err(err) = files.delete(&leftover) {
            first_failure.get_or_insert_with(|| failure(&leftover, &err));
        }
    }
    first_failure.map_or(Ok(()), Err)
}

/// Bytes the runtime at `dir` takes on disk, leftovers included.
pub(crate) fn size_on_disk(dir: &Path) -> u64 {
    // An unreadable parent only means the leftovers cannot be counted.
    let beside = leftovers(dir).unwrap_or_default();
    beside.iter().fold(dir_size(dir), |total, path| {
        total.saturating_add(dir_size(path))
    })
}

fn set_aside(files: &dyn RuntimeFiles, dir: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        // Deleted by hand: there is nothing to move.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(failure(dir, &err)),
    }
    let nonce: u64 = rand::random();
    let mut name = prefix(dir, REMOVING_MARKER);
    name.push(format!("{nonce:016x}"));
    files
        .set_aside(dir, &dir.with_file_name(name))
        .map_err(|err| failure(dir, &err))
}

/// Set-aside trees and stale staging directories of `dir`. Links are skipped:
/// DBLocal never creates one there.
fn leftovers(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let Some(parent) = dir.parent() else {
        return Ok(Vec::new());
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let prefixes = [REMOVING_MARKER, STAGING_MARKER].map(|marker| prefix(dir, marker));
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let ours = prefixes.iter().any(|prefix| {
            name.as_encoded_bytes()
                .starts_with(prefix.as_encoded_bytes())
        });
        if ours && !entry.file_type()?.is_symlink() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

fn prefix(dir: &Path, marker: &str) -> OsString {
    let mut prefix = OsString::from(".");
    prefix.push(dir.file_name().unwrap_or_default());
    prefix.push(marker);
    prefix
}

fn failure(path: &Path, err: &io::Error) -> DblError {
    DblError::RuntimeRemoval {
        reason: format!("{}: {err}", path.display()),
    }
}
