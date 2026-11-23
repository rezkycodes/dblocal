//! Real-engine check that the defaults table matches the shipped binaries.
//!
//! Ignored by default: needs the cached engine archives. It re-reads each key's
//! default from the real engine and compares it with `crate::config::schema`, so
//! a catalog bump that changes a default fails here instead of showing a stale
//! number in the Config tab.
//!
//! Run under the e2e lock:
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E=1 cargo test -p dblocal-core --test config_engine_e2e -- --ignored --nocapture
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use dblocal_core::config::{self, settings_for};
use dblocal_core::model::EngineKey;

fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!("set DBLOCAL_E2E=1 to run the real-engine config e2e");
    }
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cache/dblocal-dev")
}

/// Extracts an archive to `dest` and returns its single top-level directory.
fn extract(archive: &Path, dest: &Path) -> PathBuf {
    let status = Command::new("tar")
        .arg(match archive.extension().and_then(|e| e.to_str()) {
            Some("xz") => "-xJf",
            _ => "-xzf",
        })
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .expect("tar");
    assert!(status.success(), "extract {}", archive.display());
    let mut entries = std::fs::read_dir(dest)
        .expect("list")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir());
    entries.next().expect("one top-level dir")
}

/// The engine's own default for `key`, or `None` when it reports no default.
fn mysql_family_default(server: &Path, extra_lib: Option<&Path>, key: &str) -> Option<String> {
    let mut cmd = Command::new(server);
    cmd.arg("--no-defaults").arg("--verbose").arg("--help");
    if let Some(lib) = extra_lib {
        let mut ld = lib.display().to_string();
        if let Some(prev) = std::env::var_os("LD_LIBRARY_PATH") {
            ld = format!("{}:{}", lib.display(), Path::new(&prev).display());
        }
        cmd.env("LD_LIBRARY_PATH", ld);
    }
    let out = cmd.output().expect("server --help");
    let text = String::from_utf8_lossy(&out.stdout);
    // Lines look like: `max-connections  151` or `default-time-zone  (No default value)`.
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() == Some(key) {
            let value = parts.collect::<Vec<_>>().join(" ");
            if value.is_empty() || value.contains("(No default value)") {
                return Some(String::new());
            }
            return Some(value);
        }
    }
    None
}

#[test]
#[ignore = "e2e: needs the cached engine archives; run with DBLOCAL_E2E=1 under flock"]
fn mysql_and_mariadb_defaults_match_the_table() {
    require_e2e();
    let cache = cache_dir();
    let tmp = tempfile::tempdir().expect("tmp");

    // MySQL 8.4.11.
    let mysql_archive = cache.join("mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz");
    assert!(
        mysql_archive.is_file(),
        "missing {}",
        mysql_archive.display()
    );
    let mysql = extract(&mysql_archive, tmp.path());
    let mysqld = mysql.join("bin/mysqld");
    for spec in settings_for(EngineKey::Mysql) {
        // The help output uses dashes for underscores.
        let help_key = spec.key.replace('_', "-");
        let Some(default) = mysql_family_default(&mysqld, None, &help_key) else {
            continue;
        };
        let expected = spec.default;
        match spec.kind {
            config::ValueKind::TimeZone if expected.is_empty() => {
                assert!(
                    default.is_empty(),
                    "mysql {} default {:?}",
                    spec.key,
                    default
                );
            }
            config::ValueKind::Size { .. } => {
                // The engine prints a byte count; the table uses a human size.
                let bytes: u64 = default.parse().expect("bytes");
                let table_bytes = config::parse_size(expected).expect("table size");
                assert_eq!(bytes, table_bytes, "mysql {} size", spec.key);
            }
            _ => assert_eq!(default, expected, "mysql {} default", spec.key),
        }
    }

    // MariaDB 11.8.10, needs the libcrypt shim.
    let mdb_archive = cache.join("mariadb-11.8.10-linux-systemd-x86_64.tar.gz");
    assert!(mdb_archive.is_file(), "missing {}", mdb_archive.display());
    let mdb = extract(&mdb_archive, tmp.path());
    let mariadbd = mdb.join("bin/mariadbd");
    let shim = mdb.join("lib");
    for spec in settings_for(EngineKey::Mariadb) {
        let help_key = spec.key.replace('_', "-");
        let Some(default) = mysql_family_default(&mariadbd, Some(&shim), &help_key) else {
            continue;
        };
        let expected = spec.default;
        match spec.kind {
            config::ValueKind::TimeZone if expected.is_empty() => {
                assert!(
                    default.is_empty(),
                    "mariadb {} default {:?}",
                    spec.key,
                    default
                );
            }
            config::ValueKind::Size { .. } => {
                let bytes: u64 = default.parse().expect("bytes");
                let table_bytes = config::parse_size(expected).expect("table size");
                assert_eq!(bytes, table_bytes, "mariadb {} size", spec.key);
            }
            _ => assert_eq!(default, expected, "mariadb {} default", spec.key),
        }
    }
}

#[test]
#[ignore = "e2e: needs the cached PostgreSQL archive; run with DBLOCAL_E2E=1 under flock"]
fn postgresql_defaults_match_the_table() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join("postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz");
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tmp");
    let pg = extract(&archive, tmp.path());
    let initdb = pg.join("bin/initdb");
    let postgres = pg.join("bin/postgres");
    let data = tmp.path().join("data");
    let status = Command::new(&initdb)
        .arg("-D")
        .arg(&data)
        .arg("-A")
        .arg("trust")
        .arg("-U")
        .arg("postgres")
        .status()
        .expect("initdb");
    assert!(status.success(), "initdb");

    for spec in settings_for(EngineKey::Postgresql) {
        // The engine spells TimeZone with a capital; the file key is lowercase.
        let engine_key = match spec.key {
            "timezone" => "TimeZone",
            other => other,
        };
        let out = Command::new(&postgres)
            .arg("-D")
            .arg(&data)
            .arg("-C")
            .arg(engine_key)
            .output()
            .expect("postgres -C");
        let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        match spec.kind {
            config::ValueKind::Size { .. } => {
                // PostgreSQL prints blocks/kB; compare in bytes via parse_size.
                let bytes: u64 = value.parse().expect("number");
                let table_bytes = config::parse_size(spec.default).expect("table size");
                // `shared_buffers` is reported in 8kB blocks, `work_mem` in kB;
                // both are 1024-based multiples of a byte count.
                let as_bytes = match spec.key {
                    "shared_buffers" => bytes * 8192,
                    "work_mem" => bytes * 1024,
                    _ => bytes,
                };
                assert_eq!(as_bytes, table_bytes, "postgres {} size", spec.key);
            }
            config::ValueKind::TimeZone => {
                // A default zone name is environment-specific; only require that
                // it parses as a timezone.
                config::validate_value(spec, &value).expect("pg timezone default parses");
            }
            _ => assert_eq!(value, spec.default, "postgres {} default", spec.key),
        }
    }
}

#[test]
#[ignore = "e2e: needs the cached Valkey archive; run with DBLOCAL_E2E=1 under flock"]
fn valkey_defaults_match_the_table() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join("valkey-9.2.0-jammy-x86_64.tar.gz");
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tmp");
    let vk = extract(&archive, tmp.path());
    let server = vk.join("bin/valkey-server");
    let cli = vk.join("bin/valkey-cli");
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).expect("data");

    // A free loopback port from the reserved test range.
    let before = rc_port();
    let child = Command::new(&server)
        .arg("--port")
        .arg(before.to_string())
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--dir")
        .arg(&dir)
        .arg("--daemonize")
        .arg("no")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("valkey-server");
    // Wait for readiness.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if Command::new(&cli)
            .arg("-p")
            .arg(before.to_string())
            .arg("PING")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    for spec in settings_for(EngineKey::Valkey) {
        let out = Command::new(&cli)
            .arg("-p")
            .arg(before.to_string())
            .arg("CONFIG")
            .arg("GET")
            .arg(spec.key)
            .output()
            .expect("CONFIG GET");
        let text = String::from_utf8_lossy(&out.stdout);
        let value = text.lines().nth(1).unwrap_or("").trim().to_owned();
        match spec.kind {
            config::ValueKind::Size { .. } => {
                let bytes: u64 = value.parse().expect("bytes");
                let table_bytes = config::parse_size(spec.default).expect("table size");
                assert_eq!(bytes, table_bytes, "valkey {} size", spec.key);
            }
            _ => assert_eq!(value, spec.default, "valkey {} default", spec.key),
        }
    }

    let _ = Command::new(&cli)
        .arg("-p")
        .arg(before.to_string())
        .arg("SHUTDOWN")
        .arg("NOSAVE")
        .output();
    let mut child = child;
    let _ = child.wait();
}

/// A free loopback port in 20000-29999 for the Valkey probe.
///
/// Binds `:0` to find a free port, then retries until one lands in the reserved
/// range (`:0` usually returns the ephemeral range, which the e2e rules forbid
/// for a listener we own).
fn rc_port() -> u16 {
    for _ in 0..200 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        if (20000..=29999).contains(&port) {
            return port;
        }
    }
    // Fall back to a fixed reserved port; the e2e lock serialises the runs.
    28497
}
