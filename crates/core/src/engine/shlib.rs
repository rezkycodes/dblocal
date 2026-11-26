//! Shared-library preflight check without executing any external binary.
//!
//! The official Redis 7.4 APT package (`redis-tools`) dynamically links
//! `libssl.so.3`, `libcrypto.so.3`, and `libsystemd.so.0`. Before starting a
//! Redis instance, DBLocal inspects the dynamic loader's search directories
//! (`LD_LIBRARY_PATH`, `/etc/ld.so.conf` + included `.conf` files, and the
//! standard system library directories) without spawning any subprocess, and
//! reports any missing libraries with Debian/Ubuntu and Fedora package hints.

use std::collections::BTreeSet;
use std::fs;
#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::path::PathBuf;

use crate::error::{DblError, Result};

/// One shared library required by an engine binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredLib {
    /// ELF `NEEDED` soname, for example `libssl.so.3`.
    pub soname: &'static str,
    /// Package name on Debian / Ubuntu.
    pub debian_package: &'static str,
    /// Package name on Fedora / RHEL.
    pub fedora_package: &'static str,
}

/// Shared libraries required by the official Redis 7.4 `redis-tools` binary.
pub const REDIS_REQUIRED_LIBS: &[RequiredLib] = &[
    RequiredLib {
        soname: "libssl.so.3",
        debian_package: "libssl3",
        fedora_package: "openssl-libs",
    },
    RequiredLib {
        soname: "libcrypto.so.3",
        debian_package: "libssl3",
        fedora_package: "openssl-libs",
    },
    RequiredLib {
        soname: "libsystemd.so.0",
        debian_package: "libsystemd0",
        fedora_package: "systemd-libs",
    },
];

/// Checks that every shared library required by Redis is reachable on Linux.
///
/// On non-Linux platforms this is a no-op (Redis is Linux-only).
pub fn check_redis_libs() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let dirs = linux_library_search_dirs();
        check_libraries_in_dirs("Redis", REDIS_REQUIRED_LIBS, &dirs)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// Verifies every library in `required` exists as a readable file (following
/// symlinks) in at least one directory in `search_dirs`.
pub fn check_libraries_in_dirs(
    engine_name: &str,
    required: &[RequiredLib],
    search_dirs: &[PathBuf],
) -> Result<()> {
    let missing: Vec<&RequiredLib> = required
        .iter()
        .filter(|lib| !library_exists_in(lib.soname, search_dirs))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    let names: Vec<&str> = missing.iter().map(|lib| lib.soname).collect();
    let debian_pkgs: BTreeSet<&str> = missing.iter().map(|lib| lib.debian_package).collect();
    let fedora_pkgs: BTreeSet<&str> = missing.iter().map(|lib| lib.fedora_package).collect();

    let deb_list = debian_pkgs.into_iter().collect::<Vec<_>>().join(" ");
    let fed_list = fedora_pkgs.into_iter().collect::<Vec<_>>().join(" ");

    Err(DblError::Engine {
        reason: format!(
            "{engine_name} cannot start because required system libraries are missing: {}. \
             Install them first — Debian/Ubuntu: `sudo apt install {deb_list}`; \
             Fedora: `sudo dnf install {fed_list}`.",
            names.join(", ")
        ),
    })
}

fn library_exists_in(soname: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        let candidate = dir.join(soname);
        // `fs::metadata` follows symlinks, so a dangling symlink is treated as
        // missing rather than present.
        fs::metadata(&candidate).is_ok_and(|meta| meta.is_file())
    })
}

/// Collects the Linux dynamic loader's search directories without spawning any
/// process.
#[cfg(target_os = "linux")]
pub(crate) fn linux_library_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |p: PathBuf| {
        if !p.as_os_str().is_empty() && seen.insert(p.clone()) {
            dirs.push(p);
        }
    };

    if let Ok(env_val) = std::env::var("LD_LIBRARY_PATH") {
        for entry in std::env::split_paths(&env_val) {
            push(entry);
        }
    }

    for dir in parse_ld_so_conf(Path::new("/etc/ld.so.conf"), 0) {
        push(dir);
    }

    for default_dir in [
        "/lib64",
        "/usr/lib64",
        "/lib/x86_64-linux-gnu",
        "/usr/lib/x86_64-linux-gnu",
        "/lib/aarch64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/lib",
        "/usr/lib",
        "/usr/local/lib64",
        "/usr/local/lib",
    ] {
        push(PathBuf::from(default_dir));
    }

    dirs
}

/// Parses an `/etc/ld.so.conf`-format file, following `include` directives up
/// to a bounded depth.
#[cfg(any(target_os = "linux", test))]
fn parse_ld_so_conf(path: &Path, depth: u8) -> Vec<PathBuf> {
    if depth > 4 {
        return Vec::new();
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let base = path.parent().unwrap_or_else(|| Path::new("/etc"));
    let mut dirs = Vec::new();

    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(pattern) = line.strip_prefix("include").map(str::trim)
            && !pattern.is_empty()
        {
            for included in expand_simple_glob(base, pattern) {
                dirs.extend(parse_ld_so_conf(&included, depth + 1));
            }
            continue;
        }
        // Ignore directives like `hwcap 0 nosegneg`.
        if line.starts_with("hwcap ") {
            continue;
        }
        dirs.push(PathBuf::from(line));
    }

    dirs
}

/// Expands a simple `dir/*.ext` glob (the shape used by `/etc/ld.so.conf` ->
/// `include /etc/ld.so.conf.d/*.conf`) without pulling in a glob crate.
#[cfg(any(target_os = "linux", test))]
fn expand_simple_glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let pat_path = if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        base.join(pattern)
    };
    let Some(file_pat) = pat_path.file_name().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    if !file_pat.contains('*') {
        return vec![pat_path];
    }
    let Some(parent) = pat_path.parent() else {
        return Vec::new();
    };
    let Some((prefix, suffix)) = file_pat.split_once('*') else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matched: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .collect();
    matched.sort();
    matched
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn passes_when_all_required_libraries_exist() {
        let dir = tempdir().expect("tempdir");
        for lib in REDIS_REQUIRED_LIBS {
            fs::write(dir.path().join(lib.soname), b"ELF").expect("write");
        }
        check_libraries_in_dirs("Redis", REDIS_REQUIRED_LIBS, &[dir.path().to_path_buf()])
            .expect("all present");
    }

    #[test]
    fn fails_with_clear_debian_and_fedora_hints_when_libssl_missing() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("libsystemd.so.0"), b"ELF").expect("write");
        let err =
            check_libraries_in_dirs("Redis", REDIS_REQUIRED_LIBS, &[dir.path().to_path_buf()])
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("libssl.so.3"), "{msg}");
        assert!(msg.contains("libcrypto.so.3"), "{msg}");
        assert!(!msg.contains("libsystemd.so.0"), "{msg}");
        assert!(msg.contains("sudo apt install libssl3"), "{msg}");
        assert!(msg.contains("sudo dnf install openssl-libs"), "{msg}");
    }

    #[test]
    fn deduplicates_packages_and_lists_all_missing_libraries() {
        let dir = tempdir().expect("tempdir");
        let err =
            check_libraries_in_dirs("Redis", REDIS_REQUIRED_LIBS, &[dir.path().to_path_buf()])
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("libssl.so.3"), "{msg}");
        assert!(msg.contains("libcrypto.so.3"), "{msg}");
        assert!(msg.contains("libsystemd.so.0"), "{msg}");
        assert!(
            msg.contains("sudo apt install libssl3 libsystemd0"),
            "{msg}"
        );
        assert!(
            msg.contains("sudo dnf install openssl-libs systemd-libs"),
            "{msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_counts_as_missing() {
        let dir = tempdir().expect("tempdir");
        std::os::unix::fs::symlink(
            dir.path().join("does-not-exist"),
            dir.path().join("libssl.so.3"),
        )
        .expect("symlink");
        fs::write(dir.path().join("libcrypto.so.3"), b"ELF").expect("write");
        fs::write(dir.path().join("libsystemd.so.0"), b"ELF").expect("write");
        let err =
            check_libraries_in_dirs("Redis", REDIS_REQUIRED_LIBS, &[dir.path().to_path_buf()])
                .unwrap_err();
        assert!(err.to_string().contains("libssl.so.3"));
    }

    #[test]
    fn parses_ld_so_conf_with_includes() {
        let dir = tempdir().expect("tempdir");
        let conf_d = dir.path().join("ld.so.conf.d");
        fs::create_dir_all(&conf_d).expect("mkdir");
        fs::write(conf_d.join("custom.conf"), "# comment\n/opt/custom/lib\n").expect("write");
        let main_conf = dir.path().join("ld.so.conf");
        fs::write(&main_conf, "include ld.so.conf.d/*.conf\n/usr/local/lib\n").expect("write");
        let dirs = parse_ld_so_conf(&main_conf, 0);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/opt/custom/lib"),
                PathBuf::from("/usr/local/lib"),
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn host_preflight_passes_on_fedora_with_openssl_and_systemd() {
        check_redis_libs().expect("this host has libssl.so.3, libcrypto.so.3, libsystemd.so.0");
    }
}
