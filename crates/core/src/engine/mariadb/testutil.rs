//! Shared scaffolding for the MariaDB adapter tests.
//!
//! Fake binaries are written to a temp name, closed, chmodded and renamed into
//! place: a concurrently forked child in the same test binary can otherwise
//! inherit the write descriptor and make the following exec fail with
//! `Text file busy` (ETXTBSY). Executions also retry that transient error.

use std::path::Path;

/// Writes `body` as an executable file at `path` atomically.
pub(super) fn write_fake_script(path: &Path, body: &str) {
    let dir = path.parent().expect("fake script has a parent dir");
    std::fs::create_dir_all(dir).expect("fake script dir");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, body).expect("write fake script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake script");
    }
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path).expect("rename fake script into place");
}

/// Runs `f` until it stops failing with ETXTBSY.
///
/// A concurrent fork in another test can still hold a write descriptor to the
/// script's inode; that is a test environment artifact, not product behaviour.
pub(super) fn retry_etxtbsy<T, E>(
    mut f: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    let mut last = None;
    for _ in 0..500 {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !err.to_string().contains("Text file busy") {
                    return Err(err);
                }
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    Err(last.expect("retry budget records the last ETXTBSY error"))
}

/// A spawned child that is killed and reaped when dropped.
///
/// A test that spawns a fake engine and then panics (an assertion, or the whole
/// test binary being killed by a timeout) used to leave the child running: the
/// cleanup `child.kill()` at the end of the happy path never runs. A detached fake
/// that ignores `TERM` then lives forever, and if the test run inherited the file
/// descriptor of the `flock` guarding the heavy gate, it keeps that machine-wide
/// lock held with nothing alive to release it. This guard makes the kill happen on
/// every exit path, and `reap` lets the happy path wait explicitly.
pub(super) struct ChildGuard {
    child: std::process::Child,
}

impl ChildGuard {
    /// Wraps a freshly spawned child.
    pub(super) fn new(child: std::process::Child) -> Self {
        Self { child }
    }

    /// The child's process id.
    pub(super) fn id(&self) -> u32 {
        self.child.id()
    }

    /// Kills and reaps the child now; the guard's `Drop` then finds it gone.
    pub(super) fn reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
