//! Tests for the shared socket-fallback and private-directory module.
//!
//! Environment is never mutated: every function takes `XDG_RUNTIME_DIR` and the
//! uid as arguments, so the tests drive the pure functions and inject candidate
//! lists directly.
//!
//! The pure resolution/name/limit tests run on every platform. The filesystem
//! proofs (mode, owner, real AF_UNIX sockets) are POSIX-only: Windows has no
//! `PermissionsExt::mode`/`MetadataExt::uid` and no unix-domain socket path of
//! this shape, so those groups are declared under `#[cfg(unix)]` in `mod.rs`.

pub(super) use super::super::*;
pub(super) use tempfile::tempdir;

#[cfg(unix)]
pub(super) use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

pub(super) fn uid() -> u32 {
    current_uid()
}

#[cfg(unix)]
pub(super) fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    fs::symlink_metadata(path)
        .expect("meta")
        .permissions()
        .mode()
        & 0o777
}

#[cfg(unix)]
pub(super) fn private(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    fs::create_dir_all(dir).expect("mkdir");
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    dir.to_path_buf()
}

/// A directory `verify_secure_dir` accepts. On Windows the mode/owner checks are
/// compiled out, so a plain directory is enough.
#[cfg(not(unix))]
pub(super) fn private(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("mkdir");
    dir.to_path_buf()
}

pub(super) fn long_path(dir: &Path) -> PathBuf {
    dir.join("r".repeat(140)).join("run")
}

#[cfg(unix)]
pub(super) fn bind_socket(path: &Path) {
    use std::os::unix::fs::FileTypeExt as _;
    let _ = fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path).expect("bind socket");
    drop(listener);
    assert!(
        fs::symlink_metadata(path)
            .expect("meta")
            .file_type()
            .is_socket()
    );
}

#[cfg(unix)]
pub(super) fn xdg_candidate(dir: &Path) -> (OsString, PathBuf) {
    let xdg = dir.join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    (xdg.as_os_str().to_os_string(), fb)
}
