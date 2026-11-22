use std::fs;
use std::path::{Path, PathBuf};

use sha2::Digest as _;

use crate::error::{DblError, Result};

/// Returns the platform Unix socket path limit for MySQL.
///
/// Linux `sun_path` allows 107 bytes; macOS and BSDs allow 103 usable bytes.
pub const fn socket_path_limit() -> usize {
    #[cfg(target_os = "linux")]
    {
        107
    }
    #[cfg(not(target_os = "linux"))]
    {
        103
    }
}

/// Preferred socket path under the instance run directory.
pub fn socket_path(run_dir: &Path, port: u16) -> PathBuf {
    run_dir.join(format!("mysqld-{port}.sock"))
}

/// Resolves the socket path for `my.cnf`.
///
/// Uses the run-dir socket when it fits within the platform limit; otherwise
/// falls back to a short private directory so instance creation does not fail
/// on long homes (notably macOS). The fallback is verified as a real 0700
/// directory owned by the current euid and not a symlink.
pub fn resolve_socket_path(run_dir: &Path, port: u16, instance_dir: &Path) -> Result<PathBuf> {
    let preferred = socket_path(run_dir, port);
    if socket_path_fits(&preferred) {
        return Ok(preferred);
    }
    let fallback_dir = ensure_fallback_socket_dir()?;
    resolve_socket_path_with_fallback_dir(run_dir, port, instance_dir, &fallback_dir)
}

pub(in crate::engine::mysql) fn resolve_socket_path_with_fallback_dir(
    run_dir: &Path,
    port: u16,
    instance_dir: &Path,
    fallback_dir: &Path,
) -> Result<PathBuf> {
    let preferred = socket_path(run_dir, port);
    if socket_path_fits(&preferred) {
        return Ok(preferred);
    }
    let name = fallback_socket_name(port, instance_dir);
    let fallback = fallback_dir.join(name);
    if !socket_path_fits(&fallback) {
        return Err(DblError::InvalidInput {
            field: "socket",
            reason: format!(
                "mysql socket path {} is {} bytes, maximum is {}",
                fallback.display(),
                fallback.as_os_str().as_encoded_bytes().len(),
                socket_path_limit()
            ),
        });
    }
    tracing::info!(
        preferred = %preferred.display(),
        fallback = %fallback.display(),
        limit = socket_path_limit(),
        "mysql socket path too long; using short fallback socket directory"
    );
    Ok(fallback)
}

pub(super) fn socket_path_fits(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().len() <= socket_path_limit()
}

fn fallback_socket_name(port: u16, instance_dir: &Path) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(instance_dir.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let short_hash = &hex[..12];
    format!("m{port}-{short_hash}.sock")
}

fn fallback_socket_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if !runtime.as_os_str().is_empty() {
            candidates.push(runtime.join("dblocal"));
        }
    }
    let uid = current_euid();
    candidates.push(PathBuf::from(format!("/tmp/dblocal-{uid}")));
    candidates
}

/// Returns the fallback socket directory if one can be created/verified safely.
pub fn ensure_fallback_socket_dir() -> Result<PathBuf> {
    let uid = current_euid();
    let mut last_err = None;
    for dir in fallback_socket_dir_candidates() {
        match ensure_verified_private_dir(&dir, uid) {
            Ok(()) => return Ok(dir),
            Err(err) => last_err = Some((dir, err)),
        }
    }
    let (dir, err) = last_err.unwrap_or_else(|| {
        (
            PathBuf::from("/tmp/dblocal-unknown"),
            DblError::Engine {
                reason: "no fallback socket directory candidate".to_owned(),
            },
        )
    });
    Err(DblError::Engine {
        reason: format!(
            "cannot create verified mysql fallback socket dir {}: {err}",
            dir.display()
        ),
    })
}

fn current_euid() -> u32 {
    #[cfg(unix)]
    {
        nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn ensure_verified_private_dir(dir: &Path, uid: u32) -> Result<()> {
    if dir.exists() {
        verify_secure_dir(dir, uid)?;
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| DblError::Engine {
            reason: format!(
                "cannot create fallback socket parent {}: {e}",
                parent.display()
            ),
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(DblError::Engine {
                    reason: format!("cannot create fallback socket dir {}: {e}", dir.display()),
                });
            }
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir).map_err(|e| DblError::Engine {
            reason: format!("cannot create fallback socket dir {}: {e}", dir.display()),
        })?;
    }
    verify_secure_dir(dir, uid)
}

fn verify_secure_dir(dir: &Path, uid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(dir).map_err(|e| DblError::Engine {
            reason: format!("cannot stat fallback socket dir {}: {e}", dir.display()),
        })?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(DblError::Engine {
                reason: format!(
                    "fallback socket dir {} is not a real directory",
                    dir.display()
                ),
            });
        }
        if meta.uid() != uid {
            return Err(DblError::Engine {
                reason: format!("fallback socket dir {} has wrong owner", dir.display()),
            });
        }
        if meta.mode() & 0o777 != 0o700 {
            return Err(DblError::Engine {
                reason: format!("fallback socket dir {} is not mode 0700", dir.display()),
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, uid);
        Ok(())
    }
}

/// Validates that a socket path fits within the platform limit.
pub fn validate_socket_path_len(sock: &Path) -> Result<()> {
    let len = sock.as_os_str().as_encoded_bytes().len();
    let limit = socket_path_limit();
    if len > limit {
        return Err(DblError::InvalidInput {
            field: "socket",
            reason: format!(
                "mysql socket path {} is {} bytes, maximum is {}",
                sock.display(),
                len,
                limit
            ),
        });
    }
    Ok(())
}

pub(in crate::engine::mysql) fn cleanup_fallback_socket(socket: &Path, run_dir: &Path, port: u16) {
    let preferred = socket_path(run_dir, port);
    if socket == preferred {
        return;
    }
    let _ = fs::remove_file(socket);
}

pub(in crate::engine::mysql) fn cleanup_fallback_socket_from_my_cnf(my_cnf: &Path, run_dir: &Path, port: u16) {
    let Some(socket) = socket_path_from_my_cnf(my_cnf) else {
        return;
    };
    cleanup_fallback_socket(&socket, run_dir, port);
}

pub(in crate::engine::mysql) fn socket_path_from_my_cnf(my_cnf: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(my_cnf).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("socket=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}
