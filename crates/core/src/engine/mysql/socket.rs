//! MySQL socket-path decisions, expressed over the shared [`crate::engine::sockdir`].
//!
//! The fallback resolution, directory verification, cleanup and per-start
//! preparation live once in [`crate::engine::sockdir`]; this module only knows
//! the MySQL names and how they map onto the instance layout. Each function is
//! a one-line delegation, and the `_with_env` forms exist so tests can inject
//! `XDG_RUNTIME_DIR` and the uid instead of mutating the process environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::engine::sockdir;
use crate::error::Result;

/// `sun_path` limit in bytes (107 on Linux, 103 on macOS/BSD).
pub const fn socket_path_limit() -> usize {
    sockdir::socket_path_limit()
}

/// Preferred socket path under the instance run directory.
pub fn socket_path(run_dir: &Path, port: u16) -> PathBuf {
    run_dir.join(format!("mysqld-{port}.sock"))
}

/// Resolves the socket path for `my.cnf`, falling back to a short directory
/// when `<run_dir>/mysqld-<port>.sock` does not fit.
pub fn resolve_socket_path(run_dir: &Path, port: u16, instance_dir: &Path) -> Result<PathBuf> {
    resolve_socket_path_with_env(run_dir, port, instance_dir, std::env::var_os("XDG_RUNTIME_DIR"), sockdir::current_uid())
}

pub(in crate::engine::mysql) fn resolve_socket_path_with_env(
    run_dir: &Path,
    port: u16,
    instance_dir: &Path,
    xdg_runtime_dir: Option<OsString>,
    uid: u32,
) -> Result<PathBuf> {
    let natural = socket_path(run_dir, port);
    sockdir::resolve_socket_path(&natural, port, instance_dir, xdg_runtime_dir, uid)
}

/// Validates that a socket path fits the platform limit.
pub fn validate_socket_path_len(socket: &Path) -> Result<()> {
    sockdir::validate_socket_path_len(socket)
}

/// Reads the `socket=` directive from a MySQL `my.cnf`.
pub fn socket_path_from_my_cnf(my_cnf: &Path) -> Option<PathBuf> {
    sockdir::read_directive(my_cnf, "socket").map(PathBuf::from)
}

/// Deletes the fallback socket recorded in `my.cnf`, when it is provably ours.
pub(in crate::engine::mysql) fn cleanup_fallback_socket_from_my_cnf(my_cnf: &Path, port: u16) {
    cleanup_fallback_socket_from_my_cnf_with_env(
        my_cnf,
        port,
        std::env::var_os("XDG_RUNTIME_DIR"),
        sockdir::current_uid(),
    );
}

pub(in crate::engine::mysql) fn cleanup_fallback_socket_from_my_cnf_with_env(
    my_cnf: &Path,
    port: u16,
    xdg_runtime_dir: Option<OsString>,
    uid: u32,
) {
    sockdir::cleanup_socket_from_config(my_cnf, port, xdg_runtime_dir, uid);
}

/// Rebuilds the private `tmpdir` and the socket directory before every start.
pub(in crate::engine::mysql) fn prepare_runtime_dirs_for_start(
    run_dir: &Path,
    port: u16,
) -> Result<()> {
    prepare_runtime_dirs_for_start_with_env(
        run_dir,
        port,
        std::env::var_os("XDG_RUNTIME_DIR"),
        sockdir::current_uid(),
    )
}

pub(in crate::engine::mysql) fn prepare_runtime_dirs_for_start_with_env(
    run_dir: &Path,
    port: u16,
    xdg_runtime_dir: Option<OsString>,
    uid: u32,
) -> Result<()> {
    let spec = sockdir::PrepareSpec {
        config: &crate::engine::mysql::my_cnf_path(run_dir),
        natural_socket: &socket_path(run_dir, port),
        port,
        tmpdir: &crate::engine::mysql::tmpdir_path(run_dir),
        xdg_runtime_dir,
        uid,
    };
    sockdir::prepare_run(&spec)
}

#[cfg(test)]
mod tests;
