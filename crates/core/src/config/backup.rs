//! Backups of a config file and the "last successful start" marker
//! (`F-CFG-01` AC3).
//!
//! Layout under the DBLocal home:
//!
//! ```text
//! <home>/dblocal-backups/<instance>/<file>.<unix-ts>.bak   (0600)
//! <home>/dblocal-backups/<instance>/<file>.last-good        (hash of the file
//!                                                          at last Running)
//! ```
//!
//! The directory is `0700`; the files are `0600`, because `valkey.conf` can hold
//! `requirepass` and a backup must be exactly as secret as the file. Nothing in
//! this module ever reads a backup back into a string that could be logged: the
//! only content it handles is bytes copied file-to-file, and the hash.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fsutil;

/// How many backups to keep per file; the oldest is removed beyond this.
pub const KEEP: usize = 5;

/// One backup on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// When the backup was taken (Unix seconds), parsed from the file name.
    pub taken_at: u64,
    /// When the backup was taken as a `SystemTime`, for view types.
    pub taken_at_secs: u64,
    /// The backup file path.
    pub path: PathBuf,
}

/// Backups for one instance, rooted at `<home>/dblocal-backups/<instance>`.
#[derive(Debug, Clone)]
pub struct BackupStore {
    dir: PathBuf,
}

impl BackupStore {
    /// Builds a store rooted at the backups directory for `instance`.
    pub fn for_instance(home: &Path, instance: &str) -> Self {
        Self {
            dir: home.join(crate::consts::DIR_BACKUPS).join(instance),
        }
    }

    /// The instance backup directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Creates the directory `0700`, returning it.
    fn ensure_dir(&self) -> io::Result<()> {
        fsutil::ensure_private_dir(&self.dir)
    }

    /// Copies `source` to a timestamped backup, prunes to [`KEEP`], and returns
    /// the new backup.
    ///
    /// The copy is private (0600). It never opens the file as text, so a file
    /// with a `requirepass` line is copied verbatim and never parsed here.
    pub fn take(&self, source: &Path, now_secs: u64) -> io::Result<BackupInfo> {
        self.ensure_dir()?;
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config".to_owned());
        // A file that does not exist yet has nothing to back up.
        //
        // Two backups in the same second (an apply immediately followed by a
        // restore) must not collide on the timestamp name, or the second would
        // silently overwrite the first and break "a restore is undoable". Bump to
        // the next free second when the name is taken.
        let mut ts = now_secs;
        let target = loop {
            let candidate = self.dir.join(format!("{name}.{ts}.bak"));
            if !candidate.exists() {
                break candidate;
            }
            ts = ts.saturating_add(1);
        };
        copy_private(source, &target)?;
        self.prune(&name);
        Ok(BackupInfo {
            taken_at: ts,
            taken_at_secs: ts,
            path: target,
        })
    }

    /// Keeps the newest [`KEEP`] backups named `<file>.<ts>.bak`.
    fn prune(&self, file: &str) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let prefix = format!("{file}.");
        let mut backups: Vec<(u64, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(&prefix) else {
                continue;
            };
            let Some(ts) = rest.strip_suffix(".bak") else {
                continue;
            };
            if let Ok(secs) = ts.parse::<u64>() {
                backups.push((secs, entry.path()));
            }
        }
        backups.sort_by_key(|(secs, _)| *secs);
        while backups.len() > KEEP {
            let (_, oldest) = backups.remove(0);
            let _ = fs::remove_file(oldest);
        }
    }

    /// The newest backup for `file`, if any.
    pub fn newest(&self, file: &str) -> Option<BackupInfo> {
        let entries = fs::read_dir(&self.dir).ok()?;
        let prefix = format!("{file}.");
        let mut newest: Option<u64> = None;
        let mut path = PathBuf::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(&prefix) else {
                continue;
            };
            let Some(ts) = rest.strip_suffix(".bak") else {
                continue;
            };
            if let Ok(secs) = ts.parse::<u64>()
                && newest.is_none_or(|current| secs > current)
            {
                newest = Some(secs);
                path = entry.path();
            }
        }
        newest.map(|taken_at| BackupInfo {
            taken_at,
            taken_at_secs: taken_at,
            path,
        })
    }

    /// Marker path that records the file hash at the last successful start.
    pub fn marker_path(&self, file: &str) -> PathBuf {
        self.dir.join(format!("{file}.last-good"))
    }

    /// Records `hash` as the hash of `file` at the last successful start.
    pub fn record_last_good(&self, file: &str, hash: &str) -> io::Result<()> {
        self.ensure_dir()?;
        let path = self.marker_path(file);
        fsutil::write_private_file_atomic(&path, hash.as_bytes())
    }

    /// Reads the recorded last-good hash for `file`.
    pub fn last_good(&self, file: &str) -> Option<String> {
        let text = fs::read_to_string(self.marker_path(file)).ok()?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

/// Copies `source` to `target` as a private file, without buffering a copy in a
/// `String`.
fn copy_private(source: &Path, target: &Path) -> io::Result<()> {
    let bytes = fs::read(source)?;
    fsutil::write_private_file_atomic(target, &bytes)
}

/// Hashes bytes with SHA-256, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hash of a file's bytes, or `None` when it cannot be read.
pub fn file_sha256(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

#[cfg(test)]
mod tests;
