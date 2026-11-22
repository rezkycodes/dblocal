//! A `libcrypt.so.1` shim so a MariaDB build that links the old libcrypt can
//! start on hosts that only ship `libcrypt.so.2` (Fedora, this machine).
//!
//! The MariaDB adapter does not set `LD_LIBRARY_PATH`; a real desktop install is
//! expected to have a working `libcrypt.so.1`. This module exists only so the
//! service-level e2e can run the real `LocalService` here: it points
//! `LD_LIBRARY_PATH` at a private directory holding a symlink from
//! `libcrypt.so.1` to the system's `libcrypt.so.2`, whose symbols are compatible
//! for what `mariadbd` calls. Nothing is written outside a temp dir, and the
//! variable is process-local to the test binary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Candidate system libraries a `libcrypt.so.1` shim can point at.
const CANDIDATES: [&str; 5] = [
    "/usr/lib64/libcrypt.so.1",
    "/lib64/libcrypt.so.1",
    "/usr/lib/x86_64-linux-gnu/libcrypt.so.1",
    "/usr/lib64/libcrypt.so.2",
    "/lib64/libcrypt.so.2",
];

/// Whether the system already provides `libcrypt.so.1`.
fn system_has_so1() -> bool {
    let probe = std::process::Command::new("ldconfig").arg("-p").output();
    probe
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("libcrypt.so.1"))
        .unwrap_or(false)
}

/// Builds the shim (once per process), prepends it to `LD_LIBRARY_PATH`, and
/// returns `true` when a usable `libcrypt.so.1` is now reachable.
pub fn ensure_shim_for_runtime() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        if system_has_so1() {
            return true;
        }
        let Some(target) = CANDIDATES.iter().map(Path::new).find(|p| p.exists()) else {
            return false;
        };
        let dir = std::env::temp_dir().join(format!("dblocal-w4e-libshim-{}", std::process::id()));
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        if install_link(&dir, target).is_err() {
            return false;
        }
        prepend_to_ld_library_path(&dir);
        true
    })
}

fn install_link(dir: &Path, target: &Path) -> std::io::Result<PathBuf> {
    let link = dir.join("libcrypt.so.1");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(target, &link)?;
    Ok(link)
}

fn prepend_to_ld_library_path(dir: &Path) {
    let previous = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    let value = if previous.is_empty() {
        dir.display().to_string()
    } else {
        format!("{}:{previous}", dir.display())
    };
    // SAFETY: the e2e binary serialises every env-mutating test behind
    // `TempHome`'s lock, and no other thread in this process reads this variable
    // while it is being set.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("LD_LIBRARY_PATH", value)
    };
}
