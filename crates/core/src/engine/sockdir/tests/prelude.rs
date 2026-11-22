//! Tests for the shared socket-fallback and private-directory module.
//!
//! Environment is never mutated: every function takes `XDG_RUNTIME_DIR` and the
//! uid as arguments, so the tests drive the pure functions and inject candidate
//! lists directly.

pub(super) use super::super::*;
pub(super) use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
pub(super) use tempfile::tempdir;

pub(super) fn uid() -> u32 {
    current_uid()
}

pub(super) fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("meta")
        .permissions()
        .mode()
        & 0o777
}

pub(super) fn private(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("mkdir");
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    dir.to_path_buf()
}

pub(super) fn long_path(dir: &Path) -> PathBuf {
    dir.join("r".repeat(140)).join("run")
}

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

pub(super) fn xdg_candidate(dir: &Path) -> (OsString, PathBuf) {
    let xdg = dir.join("xdg");
    fs::create_dir_all(&xdg).expect("xdg");
    let fb = private(&xdg.join("dblocal"));
    (xdg.as_os_str().to_os_string(), fb)
}
