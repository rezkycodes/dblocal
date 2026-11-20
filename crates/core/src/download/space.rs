//! Disk space preflight.
//!
//! F-RTM-02 requires refusing to start a download unless the target filesystem
//! has at least `size_bytes * 3` bytes free, so the `.part`, the extracted tree
//! and the final runtime can all coexist during installation.

use std::path::Path;
use std::sync::Arc;

use crate::{DblError, Result};

/// Multiplier applied to the artifact's compressed size when checking free space.
pub const DISK_SPACE_MULTIPLIER: u64 = 3;

/// Result of a disk-space probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSpace {
    /// Bytes available to the current user on the relevant filesystem.
    pub available: u64,
    /// Bytes required for this operation (`artifact_size * 3`).
    pub required: u64,
}

/// Injectable probe returning available bytes for the filesystem containing
/// `path`. Tests use this to simulate a full disk without needing a real one.
pub type FreeSpaceFn = Arc<dyn Fn(&Path) -> Result<u64> + Send + Sync>;

/// Ensures `available >= artifact_size_bytes * DISK_SPACE_MULTIPLIER`.
///
/// `probe` defaults to [`available_space_for_path`] when `None`.
pub fn check_disk_space(
    path: &Path,
    artifact_size_bytes: u64,
    probe: Option<&FreeSpaceFn>,
) -> Result<DiskSpace> {
    let required = artifact_size_bytes.saturating_mul(DISK_SPACE_MULTIPLIER);
    let available = match probe {
        Some(f) => f(path)?,
        None => available_space_for_path(path)?,
    };
    if available < required {
        return Err(insufficient_space_error(available, required));
    }
    Ok(DiskSpace {
        available,
        required,
    })
}

fn insufficient_space_error(available: u64, required: u64) -> DblError {
    let msg = format!(
        "insufficient disk space: need {} bytes ({}), have {} bytes ({}) available",
        required,
        human_bytes(required),
        available,
        human_bytes(available)
    );
    DblError::Io {
        source: std::io::Error::new(std::io::ErrorKind::StorageFull, msg),
    }
}

/// Returns the number of bytes available to unprivileged users on the filesystem
/// that contains `path` (or would contain it, if the path does not exist yet).
pub fn available_space_for_path(path: &Path) -> Result<u64> {
    let probe_path = existing_ancestor(path);
    let disks = sysinfo::Disks::new_with_refreshed_list().map_err(|e| DblError::Io {
        source: std::io::Error::other(format!("cannot read disk information: {e:?}")),
    })?;

    let probe = canonicalish(&probe_path);
    let mut best: Option<(usize, u64)> = None;
    for disk in disks.iter() {
        let mount = canonicalish(disk.mount_point());
        if path_starts_with(&probe, &mount) {
            let len = mount.as_os_str().len();
            if best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                best = Some((len, disk.available_space()));
            }
        }
    }

    best.map(|(_, space)| space).ok_or_else(|| DblError::Io {
        source: std::io::Error::other(format!(
            "cannot determine free space for {}",
            path.display()
        )),
    })
}

fn existing_ancestor(path: &Path) -> std::path::PathBuf {
    let mut cur = path.to_path_buf();
    loop {
        if cur.exists() {
            return cur;
        }
        if !cur.pop() {
            return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        }
    }
}

fn canonicalish(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_starts_with(path: &Path, base: &Path) -> bool {
    // Component-wise prefix match; canonicalish normalises separators/symlinks
    // where possible so mount point comparisons behave on both Unix and Windows.
    let mut p = path.components();
    for b in base.components() {
        match p.next() {
            Some(c) if c == b => {}
            _ => return false,
        }
    }
    true
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rejects_when_insufficient() {
        let dir = tempdir().expect("tmp");
        let probe: FreeSpaceFn = Arc::new(|_| Ok(100));
        let err = check_disk_space(dir.path(), 100, Some(&probe)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("insufficient disk space"), "{msg}");
        assert!(msg.contains("need"), "{msg}");
        assert!(msg.contains("have"), "{msg}");
    }

    #[test]
    fn accepts_when_enough() {
        let dir = tempdir().expect("tmp");
        let probe: FreeSpaceFn = Arc::new(|_| Ok(1024));
        let ok = check_disk_space(dir.path(), 100, Some(&probe)).expect("ok");
        assert_eq!(ok.required, 300);
        assert_eq!(ok.available, 1024);
    }
}
