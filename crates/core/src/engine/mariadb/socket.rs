//! Socket path selection for MariaDB.
//!
//! AF_UNIX `sun_path` is 107 bytes on Linux and 103 bytes on macOS/BSD. When the
//! natural `<run_dir>/mariadb-<port>.sock` path is too long, we fall back to a
//! short private directory so instance creation never fails on deep home paths.
//! The chosen path is written into `my.cnf`; initialize, bootstrap and start all
//! read the same config.

use std::fs;
use std::path::{Path, PathBuf};

use crate::consts;
use crate::error::{DblError, Result};

/// AF_UNIX socket path limit: Linux 107, macOS/BSD 103.
pub const fn socket_path_limit() -> usize {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    {
        103
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    {
        107
    }
}

/// Natural socket path under the instance run directory.
pub fn natural_socket_path(data_dir: &Path, port: u16) -> PathBuf {
    sibling_run_dir(data_dir).join(format!("mariadb-{port}.sock"))
}

/// Chooses the socket path for an instance.
///
/// Returns the natural path when it fits. Otherwise returns a short private
/// fallback path and ensures the fallback directory exists with secure
/// permissions. The fallback is deterministic for (data_dir, port).
pub fn choose_socket_path(data_dir: &Path, port: u16) -> Result<PathBuf> {
    let natural = natural_socket_path(data_dir, port);
    if path_fits(&natural) {
        return Ok(natural);
    }
    fallback_socket_path(data_dir, port)
}

/// Validates that a socket path fits the platform limit.
pub fn validate_socket_path_len(sock: &Path) -> Result<()> {
    let len = path_len(sock);
    let limit = socket_path_limit();
    if len > limit {
        return Err(DblError::InvalidInput {
            field: "socket",
            reason: format!(
                "socket path {} is {} bytes, maximum is {}",
                sock.display(),
                len,
                limit
            ),
        });
    }
    Ok(())
}

fn path_fits(path: &Path) -> bool {
    path_len(path) <= socket_path_limit()
}

fn path_len(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

fn sibling_run_dir(data_dir: &Path) -> PathBuf {
    match data_dir.parent() {
        Some(parent) => parent.join(consts::DIR_RUN),
        None => data_dir.join(consts::DIR_RUN),
    }
}

fn fallback_socket_path(data_dir: &Path, port: u16) -> Result<PathBuf> {
    let dir = fallback_dir()?;
    ensure_secure_runtime_dir(&dir)?;
    let name = fallback_socket_name(data_dir, port);
    let path = dir.join(name);
    if !path_fits(&path) {
        return Err(DblError::InvalidInput {
            field: "socket",
            reason: format!(
                "fallback socket path {} is {} bytes, maximum is {}",
                path.display(),
                path_len(&path),
                socket_path_limit()
            ),
        });
    }
    Ok(path)
}

fn fallback_dir() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let candidate = PathBuf::from(xdg).join("dblocal");
        if candidate_fits_dir(&candidate) {
            return Ok(candidate);
        }
    }
    Ok(PathBuf::from(format!("/tmp/dblocal-{}", current_euid())))
}

fn candidate_fits_dir(dir: &Path) -> bool {
    // Leave room for "/mariadb-65535-123456789abc.sock" (31 bytes) plus margin.
    path_len(dir).saturating_add(40) <= socket_path_limit()
}

fn fallback_socket_name(data_dir: &Path, port: u16) -> String {
    let hash = short_hash(data_dir);
    format!("mariadb-{port}-{hash}.sock")
}

fn short_hash(data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data_dir.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    // 6 bytes = 12 hex chars, enough to avoid collisions in practice.
    let mut out = String::with_capacity(12);
    for b in &digest[..6] {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(unix)]
fn current_euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

#[cfg(not(unix))]
fn current_euid() -> u32 {
    0
}

pub(super) fn ensure_secure_runtime_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::ErrorKind;
        use std::os::unix::fs::DirBuilderExt;
        if dir.exists() {
            verify_private_dir(dir)?;
            return Ok(());
        }
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).map_err(|e| DblError::Engine {
                reason: format!("cannot create parent {}: {e}", parent.display()),
            })?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            // Another instance/process created it between `exists` and `create`.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(DblError::Engine {
                    reason: format!("cannot create socket dir {}: {e}", dir.display()),
                });
            }
        }
        // Re-verify after creation to catch races/symlink replacement.
        verify_private_dir(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir).map_err(|e| DblError::Engine {
            reason: format!("cannot create socket dir {}: {e}", dir.display()),
        })?;
        Ok(())
    }
}

#[cfg(unix)]
fn verify_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot inspect socket dir {}: {e}", dir.display()),
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(DblError::Engine {
            reason: format!("socket dir {} is not a real directory", dir.display()),
        });
    }
    if meta.uid() != current_euid() {
        return Err(DblError::Engine {
            reason: format!("socket dir {} has wrong owner", dir.display()),
        });
    }
    let mode = meta.mode() & 0o777;
    if mode != 0o700 {
        return Err(DblError::Engine {
            reason: format!("socket dir {} has insecure mode {:o}", dir.display(), mode),
        });
    }
    Ok(())
}

/// Returns the sibling run dir for a data dir.
pub fn sibling_run_dir_path(data_dir: &Path) -> PathBuf {
    sibling_run_dir(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn limit_matches_platform() {
        let limit = socket_path_limit();
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        assert_eq!(limit, 103);
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
        assert_eq!(limit, 107);
    }

    #[test]
    fn natural_path_is_under_run_dir() {
        let data = Path::new("/home/u/.dblocal/instances/app/data");
        let p = natural_socket_path(data, 3307);
        assert_eq!(
            p,
            PathBuf::from("/home/u/.dblocal/instances/app/run/mariadb-3307.sock")
        );
    }

    #[test]
    fn short_path_is_accepted() {
        let dir = tempdir().expect("tmp");
        let data = dir.path().join("data");
        let p = choose_socket_path(&data, 3307).expect("path");
        assert!(path_fits(&p));
    }

    #[test]
    fn long_path_uses_fallback() {
        let long_base = format!("/tmp/{}", "x".repeat(120));
        let data = Path::new(&long_base).join("instances/app/data");
        let p = choose_socket_path(&data, 3307).expect("fallback");
        assert!(path_fits(&p), "fallback must fit: {}", p.display());
        let s = p.to_string_lossy();
        assert!(
            s.contains("mariadb-3307-"),
            "fallback name should include port+hash: {s}"
        );
    }

    #[test]
    fn fallback_is_deterministic() {
        let long_base = format!("/tmp/{}", "y".repeat(120));
        let data = Path::new(&long_base).join("instances/app/data");
        let a = choose_socket_path(&data, 3307).expect("a");
        let b = choose_socket_path(&data, 3307).expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn fallback_differs_by_data_dir() {
        let base = format!("/tmp/{}", "z".repeat(120));
        let a_dir = Path::new(&base).join("a/data");
        let b_dir = Path::new(&base).join("b/data");
        let a = choose_socket_path(&a_dir, 3307).expect("a");
        let b = choose_socket_path(&b_dir, 3307).expect("b");
        assert_ne!(a, b);
    }

    #[test]
    fn validate_rejects_over_limit() {
        let long = PathBuf::from(format!("/tmp/{}", "a".repeat(120)));
        let err = validate_socket_path_len(&long).expect_err("too long");
        assert!(matches!(
            err,
            DblError::InvalidInput {
                field: "socket",
                ..
            }
        ));
    }
}
