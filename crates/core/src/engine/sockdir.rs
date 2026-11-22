//! Short-socket fallback and private runtime directories, shared by the MySQL
//! and MariaDB adapters.
//!
//! AF_UNIX `sun_path` is 107 bytes on Linux and 103 bytes on macOS/BSD. When the
//! natural `<run_dir>/<engine>-<port>.sock` path is too long, both adapters fall
//! back to a short private directory so instance creation never fails on a deep
//! home path. This module implements that behaviour exactly once:
//!
//! * [`fallback_dir_candidates`] is a pure function of `(XDG_RUNTIME_DIR, uid)`;
//! * [`resolve_socket_path`] returns the natural path when it fits, otherwise
//!   the first candidate that verifies and whose `m<port>-<12 hex>.sock` fits,
//!   otherwise an error naming every rejected candidate with both lengths;
//! * [`verify_secure_dir`] / [`ensure_secure_dir`] check a private directory is
//!   a real 0700 directory owned by us and never silently `chmod` an existing
//!   one that is not;
//! * [`cleanup_socket`] deletes only a path that is provably this instance's
//!   stale socket and only inside a fallback candidate directory;
//! * [`read_directive`] reads `socket=` / `tmpdir=` style keys from `my.cnf`;
//! * [`prepare_run`] rebuilds and re-verifies the volatile fallback directory and
//!   the private tmpdir before every start.
//!
//! Everything takes the environment as a parameter (`XDG_RUNTIME_DIR`, uid), so
//! the behaviour is a pure function of its inputs and tests never mutate the
//! process environment.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{DblError, Result};

/// AF_UNIX socket path limit in bytes: 107 on Linux, 103 on macOS/BSD.
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

/// Effective uid of this process, or `0` on non-Unix targets.
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Short fallback directories, in the order they are tried.
///
/// `$XDG_RUNTIME_DIR/dblocal` when the variable is set and non-empty, then
/// `/tmp/dblocal-<uid>`. The result is a pure function of its arguments; the
/// caller reads the environment.
pub fn fallback_dir_candidates(xdg_runtime_dir: Option<OsString>, uid: u32) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(runtime) = xdg_runtime_dir
        && !runtime.is_empty()
    {
        candidates.push(PathBuf::from(runtime).join("dblocal"));
    }
    candidates.push(PathBuf::from(format!("/tmp/dblocal-{uid}")));
    candidates
}

/// Byte length of a socket path, as `sun_path` sees it.
pub fn path_len(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

/// Whether `path` fits the platform socket path limit.
pub fn socket_path_fits(path: &Path) -> bool {
    path_len(path) <= socket_path_limit()
}

/// Fails with a typed error when `path` exceeds the platform socket path limit.
pub fn validate_socket_path_len(path: &Path) -> Result<()> {
    let len = path_len(path);
    let limit = socket_path_limit();
    if len > limit {
        return Err(DblError::InvalidInput {
            field: "socket",
            reason: format!(
                "socket path {} is {len} bytes, maximum is {limit}",
                path.display()
            ),
        });
    }
    Ok(())
}

/// Short fallback socket file name for one instance: `m<port>-<12 hex>.sock`.
///
/// The hash is derived from `identity` (the instance data directory), so two
/// instances never share a fallback socket name, and the name is deterministic
/// across restarts.
pub fn fallback_socket_name(port: u16, identity: &Path) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(identity.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    let mut hash = String::with_capacity(12);
    for byte in &digest[..6] {
        use std::fmt::Write as _;
        let _ = write!(hash, "{byte:02x}");
    }
    format!("m{port}-{hash}.sock")
}

/// Whether `name` is a fallback socket name for exactly `port`.
pub fn is_fallback_socket_name_for_port(name: &str, port: u16) -> bool {
    let Some(rest) = name.strip_prefix(&format!("m{port}-")) else {
        return false;
    };
    let Some(hash) = rest.strip_suffix(".sock") else {
        return false;
    };
    hash.len() == 12 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Refuses anything that is not a real private directory owned by `uid`.
///
/// A symlink, a regular file, a foreign owner or a mode other than `0700` is an
/// error. The mode is never repaired here: a pre-existing directory with the
/// wrong mode is reported, not `chmod`ed, because it may belong to someone else.
pub fn verify_secure_dir(dir: &Path, uid: u32) -> Result<()> {
    let meta = fs::symlink_metadata(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot inspect private dir {}: {e}", dir.display()),
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(DblError::Engine {
            reason: format!("private dir {} is not a real directory", dir.display()),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != uid {
            return Err(DblError::Engine {
                reason: format!("private dir {} has wrong owner", dir.display()),
            });
        }
        let mode = meta.mode() & 0o777;
        if mode != 0o700 {
            return Err(DblError::Engine {
                reason: format!(
                    "private dir {} is mode {mode:04o}, expected 0700",
                    dir.display()
                ),
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = uid;
    }
    Ok(())
}

/// Creates `dir` as a `0700` directory when missing, then verifies it.
///
/// Parent directories are created as needed. An existing directory is only
/// verified, never repaired: see [`verify_secure_dir`]. Racing processes that
/// created the directory between the check and the create are tolerated and the
/// result is re-verified.
pub fn ensure_secure_dir(dir: &Path, uid: u32) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(_) => return verify_secure_dir(dir, uid),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(DblError::Engine {
                reason: format!("cannot inspect private dir {}: {err}", dir.display()),
            });
        }
    }
    if let Some(parent) = dir.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| DblError::Engine {
            reason: format!("cannot create parent {}: {e}", parent.display()),
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(DblError::Engine {
                    reason: format!("cannot create private dir {}: {e}", dir.display()),
                });
            }
        }
    }
    #[cfg(not(unix))]
    {
        match fs::create_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(DblError::Engine {
                    reason: format!("cannot create private dir {}: {e}", dir.display()),
                });
            }
        }
    }
    verify_secure_dir(dir, uid)
}

/// Creates or tightens a directory this process owns, without trusting it blindly.
///
/// Unlike [`ensure_secure_dir`] this may `chmod` an existing directory to `0700`
/// because the caller owns the location by construction (the per-instance run
/// directory and its `tmpdir`). It still refuses a symlink, a regular file and a
/// directory owned by another uid, so a foreign path can never be tightened.
pub fn ensure_owned_dir(dir: &Path, uid: u32) -> Result<()> {
    if let Err(err) = ensure_secure_dir(dir, uid) {
        let owned_loose = fs::symlink_metadata(dir).ok().is_some_and(|meta| {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.uid() == uid
            }
            #[cfg(not(unix))]
            {
                let _ = uid;
                true
            }
        });
        if !owned_loose {
            return Err(err);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
                DblError::Engine {
                    reason: format!("cannot tighten private dir {}: {e}", dir.display()),
                }
            })?;
        }
        return verify_secure_dir(dir, uid);
    }
    Ok(())
}

/// Resolves the socket path for `my.cnf`.
///
/// Returns `natural` unchanged when it fits the platform limit. Otherwise tries
/// [`fallback_dir_candidates`] in order and returns the first
/// `<candidate>/m<port>-<12 hex>.sock` whose candidate verifies and whose full
/// path fits. Errors name every rejected candidate, with the path length of each
/// rejected socket and the limit.
pub fn resolve_socket_path(
    natural: &Path,
    port: u16,
    identity: &Path,
    xdg_runtime_dir: Option<OsString>,
    uid: u32,
) -> Result<PathBuf> {
    let candidates = fallback_dir_candidates(xdg_runtime_dir, uid);
    resolve_socket_path_with_candidates(natural, port, identity, candidates, uid)
}

/// [`resolve_socket_path`] with an explicit candidate list, for tests and for a
/// caller that already knows the directories to consider.
pub fn resolve_socket_path_with_candidates(
    natural: &Path,
    port: u16,
    identity: &Path,
    candidates: Vec<PathBuf>,
    uid: u32,
) -> Result<PathBuf> {
    if socket_path_fits(natural) {
        return Ok(natural.to_path_buf());
    }
    let name = fallback_socket_name(port, identity);
    let mut rejections: Vec<String> = Vec::new();
    for dir in candidates {
        if let Err(err) = ensure_secure_dir(&dir, uid) {
            rejections.push(format!("{}: {err}", dir.display()));
            continue;
        }
        let fallback = dir.join(&name);
        if !socket_path_fits(&fallback) {
            rejections.push(format!(
                "{}: socket path {} is {} bytes, maximum is {}",
                dir.display(),
                fallback.display(),
                path_len(&fallback),
                socket_path_limit()
            ));
            continue;
        }
        tracing::info!(
            natural = %natural.display(),
            fallback = %fallback.display(),
            limit = socket_path_limit(),
            "socket path too long; using a short fallback socket directory"
        );
        return Ok(fallback);
    }
    Err(DblError::InvalidInput {
        field: "socket",
        reason: format!(
            "socket path {} is {} bytes, maximum is {}, and no fallback directory produced a fitting socket path: {}",
            natural.display(),
            path_len(natural),
            socket_path_limit(),
            if rejections.is_empty() {
                "no candidates".to_owned()
            } else {
                rejections.join("; ")
            }
        ),
    })
}

/// Whether `socket` is a fallback socket for `port` inside a fallback candidate.
///
/// Used by cleanup: a path outside the shared candidates (including the natural
/// run-dir socket) is not ours to delete.
pub fn is_allowed_fallback_socket(
    socket: &Path,
    port: u16,
    xdg_runtime_dir: Option<OsString>,
    uid: u32,
) -> bool {
    let Some(parent) = socket.parent() else {
        return false;
    };
    if !fallback_dir_candidates(xdg_runtime_dir, uid)
        .iter()
        .any(|candidate| candidate == parent)
    {
        return false;
    }
    socket
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| is_fallback_socket_name_for_port(name, port))
}

/// Deletes a stale fallback socket, and nothing else.
///
/// A path is removed only when its parent is one of the fallback candidates, its
/// file name is `m<port>-<12 hex>.sock` for this port, and it is a real unix
/// socket. Anything else is logged with [`tracing::warn`] and left alone: a
/// regular file or a foreign name may belong to another program or user.
pub fn cleanup_socket(socket: &Path, port: u16, xdg_runtime_dir: Option<OsString>, uid: u32) {
    if !is_allowed_fallback_socket(socket, port, xdg_runtime_dir, uid) {
        tracing::warn!(
            socket = %socket.display(),
            port,
            "refusing to remove a path that is not a verified fallback socket"
        );
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        match fs::symlink_metadata(socket) {
            Ok(meta) if meta.file_type().is_socket() => {}
            Ok(meta) => {
                tracing::warn!(
                    socket = %socket.display(),
                    port,
                    file_type = ?meta.file_type(),
                    "refusing to remove a fallback path that is not a socket"
                );
                return;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => {
                tracing::warn!(
                    socket = %socket.display(),
                    port,
                    "cannot stat fallback socket for cleanup: {err}"
                );
                return;
            }
        }
        if let Err(err) = fs::remove_file(socket)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                socket = %socket.display(),
                port,
                "failed to remove fallback socket: {err}"
            );
        }
    }
    #[cfg(not(unix))]
    {
        // Non-Unix targets have no unix sockets; there is nothing to remove.
        let _ = (socket, port);
    }
}

/// Cleans up the socket recorded in `config`, when it is a fallback socket.
pub fn cleanup_socket_from_config(
    config: &Path,
    port: u16,
    xdg_runtime_dir: Option<OsString>,
    uid: u32,
) {
    let Some(socket) = read_directive(config, "socket") else {
        return;
    };
    cleanup_socket(Path::new(&socket), port, xdg_runtime_dir, uid);
}

/// Reads a `key=value` directive from `my.cnf`-style text.
///
/// Returns `None` when the key is absent or its value is empty. Leading and
/// trailing whitespace around the key and the value is ignored, so
/// ` socket = /path ` reads as `socket=/path`. Whole-line comments start with
/// `#` or `;`; an inline comment must be preceded by whitespace so a `#` inside
/// a path is kept. Surrounding single or double quotes around the value are
/// stripped, as `my.cnf` allows quoted option values. The first declaration wins.
pub fn parse_directive(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = strip_inline_comment(value.trim());
        let value = unquote(value.trim());
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

fn strip_inline_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for index in 1..bytes.len() {
        if matches!(bytes[index], b'#' | b';') && bytes[index - 1].is_ascii_whitespace() {
            return value[..index].trim_end();
        }
    }
    value
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Reads a directive from a config file; a missing or unreadable file reads as
/// "no such key".
pub fn read_directive(config: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(config).ok()?;
    parse_directive(&text, key)
}

/// Everything [`prepare_run`] needs, expressed as paths so both adapters can
/// share one implementation.
#[derive(Debug, Clone)]
pub struct PrepareSpec<'a> {
    /// The instance `my.cnf` holding `socket=` and `tmpdir=`.
    pub config: &'a Path,
    /// The socket path used when no fallback is needed
    /// (`<run_dir>/<engine>-<port>.sock`).
    pub natural_socket: &'a Path,
    /// Port of the instance whose match socket is prepared.
    pub port: u16,
    /// The per-instance private `tmpdir` the engine runs with.
    pub tmpdir: &'a Path,
    /// Value of `XDG_RUNTIME_DIR`, read by the caller.
    pub xdg_runtime_dir: Option<OsString>,
    /// Effective uid of this process, read by the caller.
    pub uid: u32,
}

/// Rebuilds engine-owned volatile state before a start.
///
/// On every start it:
///
/// 1. creates and verifies the private `tmpdir` (`0700`, ours, a real
///    directory), taken from `tmpdir=` when `my.cnf` declares it and from
///    [`PrepareSpec::tmpdir`] otherwise, and removes stale files left there by a
///    crash. This happens even before the config exists, because `initialize`
///    runs the engine with that `TMPDIR`;
/// 2. reads `my.cnf`. A config that does not exist yet is skipped (nothing has
///    been created), a config without a `socket` directive is an error;
/// 3. recreates and re-verifies the socket's directory — tightened to `0700` for
///    the natural run dir, strictly verified (never `chmod`ed) for a fallback
///    socket whose name is `m<port>-<12 hex>.sock`;
/// 4. removes a stale socket file at that path. A non-socket at the configured
///    socket path is an error: the engine could not bind there and deleting a
///    foreign file would be wrong.
///
/// Every failure is a [`DblError::Engine`] naming the path it could not prepare.
pub fn prepare_run(spec: &PrepareSpec<'_>) -> Result<()> {
    let text = match fs::read_to_string(spec.config) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(DblError::Engine {
                reason: format!("cannot read {}: {err}", spec.config.display()),
            });
        }
    };

    let declared_tmpdir = text
        .as_deref()
        .and_then(|text| parse_directive(text, "tmpdir"))
        .map(PathBuf::from);
    let tmpdir = declared_tmpdir.as_deref().unwrap_or(spec.tmpdir);
    prepare_tmpdir(tmpdir, spec.uid)?;

    let Some(text) = text else {
        return Ok(());
    };
    let Some(socket) = parse_directive(&text, "socket") else {
        return Err(DblError::Engine {
            reason: format!("{} has no `socket` directive", spec.config.display()),
        });
    };
    let socket = PathBuf::from(socket);

    if socket == spec.natural_socket {
        if let Some(parent) = socket.parent() {
            ensure_owned_dir(parent, spec.uid).map_err(|e| DblError::Engine {
                reason: format!("cannot prepare run directory {}: {e}", parent.display()),
            })?;
        }
        remove_stale_socket(&socket, spec.port)?;
        return Ok(());
    }
    let allowed_name = socket
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| is_fallback_socket_name_for_port(name, spec.port));
    if !allowed_name {
        return Err(DblError::Engine {
            reason: format!(
                "socket path {} is not an allowed fallback socket path for port {}",
                socket.display(),
                spec.port
            ),
        });
    }
    let parent = socket.parent().ok_or_else(|| DblError::Engine {
        reason: format!("socket path {} has no parent", socket.display()),
    })?;
    ensure_secure_dir(parent, spec.uid).map_err(|e| DblError::Engine {
        reason: format!(
            "cannot prepare fallback socket dir {}: {e}",
            parent.display()
        ),
    })?;
    remove_stale_socket(&socket, spec.port)
}

/// Creates (or tightens) the private engine `tmpdir` and clears stale contents.
///
/// A directory we own is tightened to `0700`; a symlink, a regular file or a
/// foreign-owned directory is an error. The contents are volatile (a crashed
/// server leaves temp files behind), so every start begins empty.
pub fn prepare_tmpdir(dir: &Path, uid: u32) -> Result<()> {
    ensure_owned_dir(dir, uid).map_err(|e| DblError::Engine {
        reason: format!("cannot prepare private tmpdir {}: {e}", dir.display()),
    })?;
    clear_dir_contents(dir)
}

/// Removes every entry inside `dir`, keeping `dir` itself.
///
/// Used for the private engine tmpdir, whose contents are volatile: a crashed
/// server can leave temporary files that a later start must not inherit.
pub fn clear_dir_contents(dir: &Path) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot read private dir {}: {e}", dir.display()),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| DblError::Engine {
            reason: format!("cannot read entry in {}: {e}", dir.display()),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| DblError::Engine {
            reason: format!("cannot inspect {}: {e}", path.display()),
        })?;
        let removed = if file_type.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.map_err(|e| DblError::Engine {
            reason: format!("cannot remove stale {}: {e}", path.display()),
        })?;
    }
    Ok(())
}

fn remove_stale_socket(socket: &Path, port: u16) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        match fs::symlink_metadata(socket) {
            Ok(meta) if meta.file_type().is_socket() => {
                fs::remove_file(socket).map_err(|e| DblError::Engine {
                    reason: format!("cannot remove stale socket {}: {e}", socket.display()),
                })?;
            }
            Ok(meta) => {
                return Err(DblError::Engine {
                    reason: format!(
                        "socket path {} exists but is not a socket (port {port}, type {:?})",
                        socket.display(),
                        meta.file_type()
                    ),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(DblError::Engine {
                    reason: format!("cannot stat socket {}: {err}", socket.display()),
                });
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, port);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
