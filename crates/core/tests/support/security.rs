//! Security invariant checks used by case 7.
//!
//! Each check returns the violations it found so a test can assert once with the
//! whole picture instead of failing on the first one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use dblocal_core::paths::DblPaths;

use super::grep_tree;
use super::proc;

/// Every listening socket held by `pids` that is not bound to loopback.
pub fn non_loopback_listeners(pids: &[u32]) -> Vec<String> {
    let mut violations = Vec::new();
    for pid in pids {
        for socket in proc::listen_sockets_of(*pid) {
            if !socket.is_loopback() {
                violations.push(format!(
                    "pid {pid} listens on {} port {} (inode {})",
                    socket.addr_hex, socket.port, socket.inode
                ));
            }
        }
    }
    violations
}

/// Files under `root` (excluding `shadowed`) that contain `secret`.
pub fn secret_in_files(root: &Path, secret: &str, shadowed: &[&Path]) -> Vec<String> {
    grep_tree(root, secret, shadowed)
}

/// Command lines of `pids` that contain `secret`.
pub fn secret_in_cmdlines(pids: &[u32], secret: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for pid in pids {
        if let Some(cmdline) = proc::cmdline(*pid)
            && cmdline.contains(secret)
        {
            violations.push(format!("pid {pid} argv: {cmdline}"));
        }
    }
    violations
}

/// A mode check: returns a violation when `path` is not a directory with `mode`.
pub fn dir_mode_violation(path: &Path, mode: u32) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(format!("{} is not a directory", path.display()));
    }
    let actual = meta.permissions().mode() & 0o777;
    (actual != mode).then(|| {
        format!(
            "{} has mode {:o}, expected {:o}",
            path.display(),
            actual,
            mode
        )
    })
}

/// A mode check for a regular file.
pub fn file_mode_violation(path: &Path, mode: u32) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return Some(format!("{} is not a regular file", path.display()));
    }
    let actual = meta.permissions().mode() & 0o777;
    (actual != mode).then(|| {
        format!(
            "{} has mode {:o}, expected {:o}",
            path.display(),
            actual,
            mode
        )
    })
}

/// The credential files the file backend wrote under `paths`.
pub fn credential_files(paths: &DblPaths) -> Vec<PathBuf> {
    let dir = paths.secrets_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("cred-"))
        })
        .collect();
    out.sort();
    out
}
