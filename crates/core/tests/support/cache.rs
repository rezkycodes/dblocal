//! A process-external runtime cache so tests that do not prove the download
//! path do not re-download a 40 MB archive.
//!
//! Tests 1 and 2 seed this cache after a genuine HTTPS download through the
//! service. The remaining tests copy a verified runtime tree into their own
//! temp home, which is exactly the "runtime already installed" path of
//! `F-RTM-03` AC3 — no service code is faked, only the starting state differs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dblocal_core::model::EngineKey;
use dblocal_core::paths::DblPaths;
use dblocal_core::platform::Arch;

/// The developer's real `HOME`, captured before any test redirects it.
static REAL_HOME: OnceLock<PathBuf> = OnceLock::new();

/// Records the current `HOME`. The temp-home guard calls this *before* it points
/// `HOME` into the temp tree, so the artifact cache stays reachable.
pub fn capture_real_home() {
    if let Some(home) = std::env::var_os("HOME") {
        let _ = REAL_HOME.set(PathBuf::from(home));
    }
}

/// The developer's real `HOME`, or the current one when nothing captured it.
pub fn real_home() -> PathBuf {
    REAL_HOME
        .get()
        .cloned()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default()
}

/// Cache root, outside every temp home and outside the repository.
pub fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("DBLOCAL_E2E_RUNTIME_CACHE") {
        return PathBuf::from(dir);
    }
    std::env::temp_dir().join("dblocal-e2e-runtime-cache")
}

fn cached_runtime(engine: EngineKey, version: &str) -> PathBuf {
    cache_root()
        .join("runtimes")
        .join(engine.as_str())
        .join(version)
        .join(Arch::current().as_str())
}

/// Copies an installed runtime from `paths` into the shared cache.
pub fn seed_cache_from(paths: &DblPaths, engine: EngineKey, version: &str) {
    let Ok(source) = paths.runtime_dir(engine, version, Arch::current()) else {
        return;
    };
    if !source.join("bin").is_dir() {
        return;
    }
    let target = cached_runtime(engine, version);
    if target.join("bin").is_dir() {
        return;
    }
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    copy_tree(&source, &target).expect("populate the runtime cache");
}

/// Copies a cached runtime into `paths`, if one is cached.
///
/// Returns `true` when the runtime is now installed, so the caller can skip the
/// download stage.
pub fn seed_runtime(paths: &DblPaths, engine: EngineKey, version: &str) -> bool {
    let Ok(target) = paths.runtime_dir(engine, version, Arch::current()) else {
        return false;
    };
    if target.join("bin").is_dir() {
        return true;
    }
    let source = cached_runtime(engine, version);
    if !source.join("bin").is_dir() {
        return false;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("runtime parent");
    }
    copy_tree(&source, &target).expect("copy the cached runtime");
    true
}

/// Materialises the runtime tree for `engine` from the cached archive, extracted
/// with the host's own `tar`.
///
/// This exists because of a defect this suite found: the production extractor
/// rejects the in-tree hard links that every shipped PostgreSQL archive carries,
/// so a PostgreSQL runtime can never be produced through `create_instance`
/// (`repro_hardlink` documents it). The other PostgreSQL e2e cases must still be
/// provable, so this helper builds the same tree the extractor would have built —
/// `strip_prefix` applied, executable bits kept — and drops it where
/// `runtime_is_installed` looks. Only the starting state differs: the service
/// then takes its documented "runtime already installed" path, exactly as it
/// would on a second launch after a successful install.
///
/// Returns `false` when the archive is not cached.
pub fn extract_archive_to_runtime(
    paths: &DblPaths,
    engine: EngineKey,
    version: &str,
    archive_name: &str,
) -> bool {
    let Ok(target) = paths.runtime_dir(engine, version, Arch::current()) else {
        return false;
    };
    if target.join("bin").is_dir() {
        return true;
    }
    let cache = real_home().join(".cache/dblocal-dev");
    let archive = cache.join(archive_name);
    if !archive.is_file() {
        return false;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("runtime parent");
    }
    let staging = target.with_file_name(format!(".{version}.staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).expect("staging directory");
    // The archives carry exactly one top-level directory (`strip_prefix` in the
    // catalog), so dropping the first path component reproduces the extractor's
    // result: `bin/` one level inside the runtime root.
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .arg("--strip-components=1")
        .status()
        .expect("run tar");
    assert!(
        status.success(),
        "tar could not extract {}",
        archive.display()
    );
    fs::rename(&staging, &target).expect("rename the extracted runtime into place");
    target.join("bin").is_dir()
}

/// Recursively copies `from` to `to`, preserving permissions (the executable
/// bit is what makes the engine runnable).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())?;
    } else if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        let _ = fs::remove_file(to);
        std::os::unix::fs::symlink(target, to)?;
    } else {
        fs::copy(from, to)?;
        fs::set_permissions(to, meta.permissions())?;
    }
    Ok(())
}
