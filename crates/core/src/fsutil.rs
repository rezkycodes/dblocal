//! Small filesystem utilities shared by extraction, engine config, and instance layout.
//!
//! These helpers centralize atomic private-file writes and private-directory creation
//! so security-sensitive mode/fsync behavior is implemented once.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const EXEC_BUSY_RETRY_WINDOW: Duration = Duration::from_secs(5);
const EXEC_BUSY_RETRY_DELAY: Duration = Duration::from_millis(20);

/// A path as a value in a MySQL/MariaDB option file (`my.cnf`).
///
/// Option files treat a backslash as an escape (`\r`, `\s`, `\t`, `\b`, `\n`),
/// so `C:\dbl\home\instances\x\run` would be read with a carriage return in
/// it. Both servers accept forward slashes on Windows, so a Windows path is
/// written with them. `windows` is a parameter so both spellings are testable
/// on every platform.
#[must_use]
pub fn option_file_path(path: &Path, windows: bool) -> String {
    let text = path.display().to_string();
    if windows {
        text.replace('\\', "/")
    } else {
        text
    }
}

/// Creates `path` (and parents) and tightens it to `0700` on Unix.
///
/// Existing directories are also tightened, so a permissive pre-existing directory
/// does not leak local access.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Atomically writes `contents` to `path` with mode `0600` on Unix.
///
/// The write goes to a unique temporary sibling in the same directory, is flushed and
/// fsynced, then renamed into place. On Unix the parent directory is fsynced
/// best-effort after the rename. The temporary file is removed on failure.
pub fn write_private_file_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_file_atomic(path, contents, 0o600)
}

/// Atomically writes an executable file with mode `0755` on Unix.
///
/// Uses the same temp-write-fsync-rename pattern as [`write_private_file_atomic`].
/// Atomic rename does **not** by itself prevent ETXTBSY: the busy state belongs to
/// the inode, and a forked child that inherited the temp file's write fd keeps it
/// open until it execs, regardless of the file's name. Callers that may exec the
/// file from a process that also forks should combine atomic writes with a bounded
/// ETXTBSY retry on spawn (see [`output_retrying_text_file_busy`]).
pub fn write_executable_file_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_file_atomic(path, contents, 0o755)
}

/// Atomically writes a file with the given Unix mode.
///
/// Exposed for callers that need a specific mode (e.g. launcher scripts with 0600).
pub fn write_file_atomic_with_mode(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    write_file_atomic(path, contents, mode)
}

fn write_file_atomic(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    let nonce: u64 = rand::random();
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("file");
    let tmp = parent.join(format!(".{name}.tmp-{nonce:016x}"));

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = opts.open(&tmp)?;
    if let Err(e) = file.write_all(contents).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Runs a command, retrying ETXTBSY (`os error 26`) within a bounded window.
///
/// Concurrent test threads in the same process can inherit a write fd of a script
/// that another thread just wrote; while that fd stays open, exec fails with
/// ETXTBSY. The bounded retry covers that narrow window without hiding real errors.
pub fn output_retrying_text_file_busy(mut cmd: Command) -> io::Result<Output> {
    let spawn_deadline = std::time::Instant::now() + EXEC_BUSY_RETRY_WINDOW;
    loop {
        match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
            Ok(output) => return Ok(output),
            Err(source)
                if is_text_file_busy(&source) && std::time::Instant::now() < spawn_deadline =>
            {
                std::thread::sleep(EXEC_BUSY_RETRY_DELAY);
            }
            Err(source) => return Err(source),
        }
    }
}

fn is_text_file_busy(err: &io::Error) -> bool {
    err.raw_os_error() == Some(26)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("meta").permissions().mode() & 0o777
    }

    #[test]
    fn write_private_file_atomic_writes_and_is_private() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("conf").join("app.conf");
        write_private_file_atomic(&path, b"hello\n").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"hello\n");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600, "file must be 0600");
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[test]
    fn write_private_file_atomic_overwrites_atomically() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("f");
        write_private_file_atomic(&path, b"one").expect("first");
        write_private_file_atomic(&path, b"two").expect("second");
        assert_eq!(fs::read(&path).expect("read"), b"two");
    }

    #[cfg(unix)]
    #[test]
    fn write_executable_file_atomic_writes_0755_and_no_leftovers() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("bin").join("tool");
        write_executable_file_atomic(&path, b"#!/bin/sh\nexit 0\n").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"#!/bin/sh\nexit 0\n");
        assert_eq!(mode(&path), 0o755, "executable must be 0755");
        let leftovers: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[test]
    fn write_private_file_atomic_removes_temp_on_failure() {
        // `dir` is only used by the Unix arms below.
        #[allow(unused_variables)]
        let dir = tempdir().expect("tmp");
        #[cfg(unix)]
        {
            let parent = dir.path().join("ro");
            fs::create_dir_all(&parent).expect("mkdir");
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).expect("chmod");
            let path = parent.join("sub").join("f");
            let err = write_private_file_atomic(&path, b"x");
            assert!(err.is_err());
            // Parent itself should have no tmp files.
            let leftovers: Vec<_> = fs::read_dir(&parent)
                .expect("read dir")
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
                .collect();
            assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
            // restore so tempdir cleanup works
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("restore");
        }
        #[cfg(not(unix))]
        {
            // On non-Unix, just assert a clearly invalid write fails without panicking.
            let err = write_private_file_atomic(Path::new("/"), b"x");
            assert!(err.is_err());
        }
    }

    #[test]
    fn ensure_private_dir_creates_and_tightens() {
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("a").join("b");
        ensure_private_dir(&target).expect("create");
        assert!(target.is_dir());
        #[cfg(unix)]
        assert_eq!(mode(&target), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_tightens_existing_permissive_dir() {
        let dir = tempdir().expect("tmp");
        let target = dir.path().join("existing");
        fs::create_dir_all(&target).expect("mkdir");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod");
        ensure_private_dir(&target).expect("tighten");
        assert_eq!(mode(&target), 0o700);
    }
}
