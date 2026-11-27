//! Where each engine's binaries live inside an installed runtime.
//!
//! This is the single source of truth for two questions that must always agree:
//!
//! - which paths extraction must verify before a runtime is finalised, and
//! - whether a runtime already on disk is installed and can be reused.
//!
//! They used to disagree: extraction verified the engine's expected binaries
//! while the reuse check looked for a hard-coded `bin/` directory. RustFS ships
//! a single `rustfs` file at the archive root (no `bin/`), so a second create
//! never saw the installed runtime, re-downloaded it and collided with the
//! existing directory (`DBL-1003`).

use std::path::Path;

use crate::model::EngineKey;

/// Binaries an extracted runtime must contain, relative to its root, named as
/// the engine drivers launch them.
pub fn expected_binaries(engine: EngineKey) -> Vec<String> {
    expected_binaries_for(engine, cfg!(windows))
}

/// [`expected_binaries`] with the platform passed in, so both spellings are
/// testable on any host.
pub fn expected_binaries_for(engine: EngineKey, windows: bool) -> Vec<String> {
    let suffix = if windows { ".exe" } else { "" };
    if engine == EngineKey::Rustfs {
        // The RustFS zip holds one `rustfs` entry at the archive root and the
        // catalog sets no `strip_prefix`, so there is no `bin/` to look in.
        return vec![format!("rustfs{suffix}")];
    }
    let bases: &[&str] = match engine {
        EngineKey::Postgresql => &["postgres", "initdb", "pg_ctl"],
        EngineKey::Valkey => &["valkey-server"],
        EngineKey::Redis => &["redis-check-rdb", "redis-cli"],
        EngineKey::Mysql => &["mysqld"],
        EngineKey::Mariadb => &["mariadbd", "mysqld"],
        EngineKey::Rustfs => unreachable!("handled above"),
    };
    bases
        .iter()
        .map(|base| format!("bin/{base}{suffix}"))
        .collect()
}

/// Whether every expected binary exists as a file under `runtime_dir`.
///
/// A bare directory is not enough: `create` uses this to decide whether to
/// download, and treating a partially populated tree as installed would start
/// an instance from a runtime that cannot launch.
pub fn runtime_has_binaries(runtime_dir: &Path, expected: &[String]) -> bool {
    !expected.is_empty() && expected.iter().all(|rel| runtime_dir.join(rel).is_file())
}

/// Whether `runtime_dir` holds an installed runtime for `engine` on this host.
pub fn runtime_is_installed(runtime_dir: &Path, engine: EngineKey) -> bool {
    runtime_has_binaries(runtime_dir, &expected_binaries(engine))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;

    const ENGINES: [EngineKey; 6] = [
        EngineKey::Postgresql,
        EngineKey::Valkey,
        EngineKey::Redis,
        EngineKey::Mysql,
        EngineKey::Mariadb,
        EngineKey::Rustfs,
    ];

    /// Writes every expected binary for `engine` under `root`.
    fn plant(root: &Path, engine: EngineKey, windows: bool) {
        for rel in expected_binaries_for(engine, windows) {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            std::fs::write(&path, b"binary").expect("binary");
        }
    }

    /// Windows names carry the suffix; every other engine nests under `bin/`.
    #[test]
    fn windows_names_carry_the_exe_suffix() {
        for engine in ENGINES {
            let names = expected_binaries_for(engine, true);
            assert!(!names.is_empty(), "{engine:?} must expect a binary");
            for name in names {
                assert!(
                    name.ends_with(".exe"),
                    "{engine:?}: `{name}` must carry the `.exe` suffix on Windows"
                );
            }
        }
    }

    /// The install check must look where each engine's archive really puts it.
    #[test]
    fn unix_names_match_their_archive_layout() {
        for engine in ENGINES {
            for name in expected_binaries_for(engine, false) {
                assert!(!name.ends_with(".exe"), "{engine:?}: `{name}`");
                if engine == EngineKey::Rustfs {
                    assert_eq!(
                        name, "rustfs",
                        "the RustFS zip has no `bin/`: the binary is at the root"
                    );
                } else {
                    assert!(
                        name.starts_with("bin/"),
                        "{engine:?}: `{name}` must be `bin/<name>` on unix"
                    );
                }
            }
        }
    }

    /// Every engine's real layout is recognised, and only when it is complete
    /// and actually a file. One row per engine so a new engine cannot regress.
    #[test]
    fn every_engine_recognises_only_a_complete_real_layout() {
        for engine in ENGINES {
            let dir = tempdir().expect("tempdir");

            assert!(
                !runtime_is_installed(dir.path(), engine),
                "{engine:?}: an empty directory is not installed"
            );

            // A bare `bin/` directory is what fooled the RustFS check; it must
            // never be enough on its own for any engine.
            std::fs::create_dir_all(dir.path().join("bin")).expect("bare bin");
            assert!(
                !runtime_is_installed(dir.path(), engine),
                "{engine:?}: a bare `bin/` directory must not look installed"
            );

            // A partial layout (all but the last binary) is not installed.
            let expected = expected_binaries_for(engine, cfg!(windows));
            if expected.len() > 1 {
                std::fs::remove_dir_all(dir.path()).expect("reset");
                std::fs::create_dir_all(dir.path()).expect("root");
                for rel in &expected[..expected.len() - 1] {
                    let path = dir.path().join(rel);
                    std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
                    std::fs::write(&path, b"binary").expect("binary");
                }
                assert!(
                    !runtime_is_installed(dir.path(), engine),
                    "{engine:?}: a partial runtime is not installed"
                );
            }

            // The real, complete layout is installed.
            std::fs::remove_dir_all(dir.path()).expect("reset");
            std::fs::create_dir_all(dir.path()).expect("root");
            plant(dir.path(), engine, cfg!(windows));
            assert!(
                runtime_is_installed(dir.path(), engine),
                "{engine:?}: the real archive layout must be recognised"
            );
        }
    }

    /// The regression this module exists for: a runtime unpacked by the current
    /// build for RustFS is a root-level `rustfs` file, and must be recognised so
    /// a second create reuses it instead of re-downloading into the same path.
    #[test]
    fn a_rustfs_runtime_at_the_root_is_recognised() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("rustfs"), b"\x7fELF").expect("rustfs");
        assert!(runtime_is_installed(dir.path(), EngineKey::Rustfs));
    }

    /// The names must be the ones the engine drivers launch (`exe_name`), so the
    /// install check and the start path cannot drift apart.
    #[test]
    fn the_install_check_matches_what_the_drivers_launch() {
        let launched = |engine: EngineKey| -> Vec<String> {
            let bases: &[&str] = match engine {
                EngineKey::Postgresql => &["postgres", "initdb", "pg_ctl"],
                EngineKey::Valkey => &["valkey-server"],
                EngineKey::Redis => &["redis-check-rdb", "redis-cli"],
                EngineKey::Mysql => &["mysqld"],
                EngineKey::Mariadb => &["mariadbd", "mysqld"],
                EngineKey::Rustfs => return vec![crate::engine::uri::exe_name("rustfs")],
            };
            bases
                .iter()
                .map(|base| format!("bin/{}", crate::engine::uri::exe_name(base)))
                .collect()
        };
        for engine in ENGINES {
            assert_eq!(
                expected_binaries_for(engine, cfg!(windows)),
                launched(engine),
                "{engine:?}"
            );
        }
    }

    /// A planted path that is a directory, not a file, is not a usable binary.
    #[test]
    fn a_directory_named_like_a_binary_is_not_installed() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("bin/postgres")).expect("decoy");
        assert!(!runtime_is_installed(dir.path(), EngineKey::Postgresql));
        let _ = PathBuf::new();
    }
}
