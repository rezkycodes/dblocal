//! Real-engine MySQL 8.4 integration tests.
//!
//! Ignored by default: these extract and run the cached MySQL minimal tarball.
//! Run under the machine-wide e2e lock:
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" \
//!   env DBLOCAL_E2E_MYSQL=1 cargo test -p dblocal-core --test mysql_e2e -- --ignored --nocapture
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use dblocal_core::engine::{EngineAdapter, MysqlAdapter};
use dblocal_core::model::AuthMode;

mod helpers;

use helpers::*;

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_full_cycle() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());

    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());
    assert!(rt.join("bin/mysqld").is_file(), "mysqld missing");

    let port = 42010u16;
    require_port_free(port);
    let (_, data, logs, run) = mk_instance(tmp.path(), "my1");
    let pw = "e2e-mysql-secret-pw";

    // init
    MysqlAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("initialize mysql");
    assert!(data.join("ibdata1").is_file(), "ibdata1 must exist");
    let cnf = fs::read_to_string(run.join("my.cnf")).expect("my.cnf");
    assert!(cnf.contains("bind-address=127.0.0.1"));
    assert!(cnf.contains("mysqlx=OFF"));
    assert!(cnf.contains(&format!("port={port}")));
    assert!(
        !run.join("dblocal-init.sql").exists(),
        "init file must be removed"
    );

    // start
    let decoy_dir = std::env::temp_dir();
    let decoy = decoy_dir.join(format!("dblocal-mysql-decoy-{port}-#sql-decoy"));
    fs::write(&decoy, b"decoy").expect("write decoy");

    let mut guard = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not listen; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    let pid = guard.0.as_ref().unwrap().id();
    assert_only_instance_loopback_port(pid, port);
    let tmpdir = run.join("tmp");
    assert!(
        tmpdir.is_dir(),
        "private tmpdir must exist while server runs"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(&tmpdir).expect("tmpdir meta");
        assert_eq!(meta.mode() & 0o777, 0o700, "tmpdir must be 0700");
        assert_eq!(meta.uid(), nix::unistd::geteuid().as_raw());
    }

    // health
    let ctx = run_ctx(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    let h = MysqlAdapter.health_check(&ctx).expect("health");
    assert!(matches!(h, dblocal_core::engine::Health::Ready));

    // login with password
    let out = mysql_client(&rt, port, pw, "SELECT @@port, @@bind_address, @@version;");
    assert!(
        out.status.success(),
        "login failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&port.to_string()));
    assert!(stdout.contains("127.0.0.1"));
    assert!(stdout.contains("8.4.11"));

    // create data that must survive restart
    let out = mysql_client(
        &rt,
        port,
        pw,
        "CREATE DATABASE dblocal_e2e; CREATE TABLE dblocal_e2e.t(v INT); INSERT INTO dblocal_e2e.t VALUES (42);",
    );
    assert!(
        out.status.success(),
        "seed failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // wrong password rejected
    let bad = Command::new(rt.join("bin/mysql"))
        .args([
            "--no-defaults",
            "-h",
            "127.0.0.1",
            "-P",
            &port.to_string(),
            "-u",
            "root",
            "--protocol=TCP",
            "-Nse",
            "SELECT 1",
        ])
        .env("MYSQL_PWD", "wrong-password")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("mysql bad");
    assert!(!bad.status.success(), "wrong password must fail");
    let bad_err = String::from_utf8_lossy(&bad.stderr);
    assert!(
        bad_err.contains("Access denied") || bad_err.contains("ERROR 1045"),
        "unexpected: {bad_err}"
    );

    // stop
    MysqlAdapter.shutdown(&ctx).expect("shutdown");
    if let Some(mut c) = guard.0.take() {
        let _ = c.wait();
    }
    assert!(
        wait_tcp_gone(port, Duration::from_secs(30)),
        "port still listening"
    );
    let log = read_log_tail(&logs.join("error.log"), 40);
    assert!(
        log.contains("Shutdown complete"),
        "log missing Shutdown complete:\n{log}"
    );

    // restart, data survives
    let mut guard2 = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "restart did not listen: {}",
        read_log_tail(&logs.join("error.log"), 80)
    );
    let h2 = MysqlAdapter.health_check(&ctx).expect("health restart");
    assert!(matches!(h2, dblocal_core::engine::Health::Ready));
    let out = mysql_client(&rt, port, pw, "SELECT v FROM dblocal_e2e.t;");
    assert!(
        out.status.success(),
        "select after restart failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");

    MysqlAdapter.shutdown(&ctx).expect("shutdown restart");
    if let Some(mut c) = guard2.0.take() {
        let _ = c.wait();
    }
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));
    assert!(
        decoy.exists(),
        "shared TMPDIR decoy must not be removed by mysql"
    );
    let _ = fs::remove_file(&decoy);
}

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_two_instances_side_by_side() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());

    let port_a = 42011u16;
    let port_b = 42012u16;
    require_port_free(port_a);
    require_port_free(port_b);
    let (_, da, la, ra) = mk_instance(tmp.path(), "a");
    let (_, db, lb, rb) = mk_instance(tmp.path(), "b");
    let pwa = "e2e-mysql-a-pw";
    let pwb = "e2e-mysql-b-pw";

    MysqlAdapter
        .initialize(&init_ctx(&rt, &da, port_a, AuthMode::Password, Some(pwa)))
        .expect("init a");
    MysqlAdapter
        .initialize(&init_ctx(&rt, &db, port_b, AuthMode::Password, Some(pwb)))
        .expect("init b");

    let mut ga = mysql_start(&rt, &da, &la, &ra, port_a, pwa);
    let mut gb = mysql_start(&rt, &db, &lb, &rb, port_b, pwb);
    assert!(
        wait_tcp(port_a, Duration::from_secs(30)),
        "a not listening: {}",
        read_log_tail(&la.join("error.log"), 80)
    );
    assert!(
        wait_tcp(port_b, Duration::from_secs(30)),
        "b not listening: {}",
        read_log_tail(&lb.join("error.log"), 80)
    );
    assert_only_instance_loopback_port(ga.0.as_ref().unwrap().id(), port_a);
    assert_only_instance_loopback_port(gb.0.as_ref().unwrap().id(), port_b);

    let ca = run_ctx(&rt, &da, &la, &ra, port_a, AuthMode::Password, Some(pwa));
    let cb = run_ctx(&rt, &db, &lb, &rb, port_b, AuthMode::Password, Some(pwb));
    assert!(matches!(
        MysqlAdapter.health_check(&ca).unwrap(),
        dblocal_core::engine::Health::Ready
    ));
    assert!(matches!(
        MysqlAdapter.health_check(&cb).unwrap(),
        dblocal_core::engine::Health::Ready
    ));

    let oa = mysql_client(&rt, port_a, pwa, "SELECT @@port;");
    let ob = mysql_client(&rt, port_b, pwb, "SELECT @@port;");
    assert_eq!(
        String::from_utf8_lossy(&oa.stdout).trim(),
        port_a.to_string()
    );
    assert_eq!(
        String::from_utf8_lossy(&ob.stdout).trim(),
        port_b.to_string()
    );

    MysqlAdapter.shutdown(&ca).expect("stop a");
    MysqlAdapter.shutdown(&cb).expect("stop b");
    if let Some(mut c) = ga.0.take() {
        let _ = c.wait();
    }
    if let Some(mut c) = gb.0.take() {
        let _ = c.wait();
    }
    assert!(wait_tcp_gone(port_a, Duration::from_secs(30)));
    assert!(wait_tcp_gone(port_b, Duration::from_secs(30)));
}

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_spawn_argv_has_no_password_canary() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());
    let port = 42013u16;
    require_port_free(port);
    let (_, data, logs, run) = mk_instance(tmp.path(), "canary");
    let pw = "canary-secret-mysql-pw";
    MysqlAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("init");
    let mut guard = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "listen: {}",
        read_log_tail(&logs.join("error.log"), 80)
    );
    // argv check is done in mysql_start; also check /proc/cmdline of the process.
    let pid = guard.0.as_ref().unwrap().id();
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let cmdline_s = String::from_utf8_lossy(&cmdline);
    assert!(
        !cmdline_s.contains(pw),
        "password leaked in /proc/cmdline: {cmdline_s:?}"
    );
    let ctx = run_ctx(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    MysqlAdapter.shutdown(&ctx).expect("shutdown");
    if let Some(mut c) = guard.0.take() {
        let _ = c.wait();
    }
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));
}

fn mysql_client_no_password(runtime: &Path, port: u16, sql: &str) -> std::process::Output {
    let mysql = runtime.join("bin/mysql");
    Command::new(mysql)
        .args([
            "--no-defaults",
            "-h",
            "127.0.0.1",
            "-P",
            &port.to_string(),
            "-u",
            "root",
            "--protocol=TCP",
            "-Nse",
            sql,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("mysql client no password")
}

fn socket_path_from_my_cnf(cnf_path: &Path) -> PathBuf {
    let s = fs::read_to_string(cnf_path).expect("read my.cnf");
    for line in s.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("socket=") {
            return PathBuf::from(v.trim());
        }
    }
    panic!("socket not found in {}", cnf_path.display());
}

/// Replaces the `socket=` directive of an instance `my.cnf` with `sock`.
///
/// Used to point an instance at a private directory a test creates and owns, so
/// the test never has to delete the shared per-user fallback directory.
fn rewrite_socket_in_my_cnf(cnf: &Path, sock: &Path) {
    let text = fs::read_to_string(cnf).expect("read my.cnf");
    let mut out = String::with_capacity(text.len() + 64);
    let mut replaced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("socket=") {
            use std::fmt::Write as _;
            let _ = writeln!(out, "socket={}", sock.display());
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(replaced, "my.cnf must have a socket directive to rewrite");
    fs::write(cnf, out.as_bytes()).expect("rewrite my.cnf");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(cnf, fs::Permissions::from_mode(0o600)).expect("chmod my.cnf");
    }
}

fn assert_file_has_no_password_bytes(path: &Path, needle: &[u8], label: &str) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if bytes.windows(needle.len()).any(|w| w == needle) {
        panic!(
            "password bytes {:?} found in {} ({})",
            label,
            path.display(),
            label
        );
    }
}

fn assert_no_password_in_instance_files(instance_dir: &Path, needles: &[(&str, &[u8])]) {
    let mut stack = vec![instance_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).expect("meta");
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if meta.is_file() {
                for (label, needle) in needles {
                    assert_file_has_no_password_bytes(&path, needle, label);
                }
            }
        }
    }
}

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_no_password_cycle() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());
    let port = 42014u16;
    require_port_free(port);
    let (_, data, logs, run) = mk_instance(tmp.path(), "nopw");
    MysqlAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::None, None))
        .expect("initialize mysql no-password");
    let ctx = run_ctx(&rt, &data, &logs, &run, port, AuthMode::None, None);
    let mut guard = KillOnDrop(None, "mysqld-nopw");
    let cmd = MysqlAdapter.spawn_command(&ctx);
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    guard.0 = Some(c.spawn().expect("spawn mysqld"));
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not listen; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    let h = MysqlAdapter.health_check(&ctx).expect("health");
    assert!(matches!(h, dblocal_core::engine::Health::Ready));
    let out = mysql_client_no_password(&rt, port, "SELECT CURRENT_USER();");
    assert!(
        out.status.success(),
        "login failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "root@127.0.0.1"
    );

    let uri = MysqlAdapter.connection_uri_no_password(&ctx, dblocal_core::service::UriFormat::Uri);
    let expected_uri = format!("mysql://root@127.0.0.1:{port}/");
    assert_eq!(uri.expose(), expected_uri);
    let parsed = url::Url::parse(uri.expose()).expect("parse uri");
    assert_eq!(parsed.scheme(), "mysql");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(port));
    assert_eq!(parsed.username(), "root");
    assert_eq!(parsed.password(), None);

    MysqlAdapter.shutdown(&ctx).expect("shutdown");
    if let Some(mut c) = guard.0.take() {
        let _ = c.wait();
    }
    assert!(
        wait_tcp_gone(port, Duration::from_secs(30)),
        "port still listening"
    );
    let sock = socket_path_from_my_cnf(&run.join("my.cnf"));
    assert!(
        !sock.exists(),
        "socket should be gone after shutdown: {}",
        sock.display()
    );
}

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_hostile_password_works_and_never_reaches_a_file() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());
    let port = 42015u16;
    require_port_free(port);
    let (inst, data, logs, run) = mk_instance(tmp.path(), "hostile");
    let pw = "Cx9!it's\"q\\z$HOME`id`;&&%*-é漢";
    MysqlAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("initialize mysql hostile password");
    let mut guard = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not listen; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    let ctx = run_ctx(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));

    let out = mysql_client(&rt, port, pw, "SELECT CURRENT_USER();");
    assert!(
        out.status.success(),
        "tcp login failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "root@127.0.0.1"
    );

    let sock = socket_path_from_my_cnf(&run.join("my.cnf"));
    let out_sock = Command::new(rt.join("bin/mysql"))
        .args([
            "--no-defaults",
            "-S",
            sock.to_str().expect("sock utf8"),
            "-u",
            "root",
            "-Nse",
            "SELECT CURRENT_USER();",
        ])
        .env("MYSQL_PWD", pw)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("mysql socket");
    assert!(
        out_sock.status.success(),
        "socket login failed: {}",
        String::from_utf8_lossy(&out_sock.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out_sock.stdout).trim(),
        "root@localhost"
    );

    let out_bad = Command::new(rt.join("bin/mysql"))
        .args([
            "--no-defaults",
            "-S",
            sock.to_str().expect("sock utf8"),
            "-u",
            "root",
            "-Nse",
            "SELECT 1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("mysql socket bad");
    assert!(
        !out_bad.status.success(),
        "empty password on socket must fail"
    );

    MysqlAdapter.shutdown(&ctx).expect("shutdown");
    if let Some(mut c) = guard.0.take() {
        let _ = c.wait();
    }
    assert!(
        wait_tcp_gone(port, Duration::from_secs(30)),
        "port still listening"
    );
    assert!(
        !sock.exists(),
        "socket should be gone after shutdown: {}",
        sock.display()
    );

    let needles: Vec<(&str, &[u8])> = vec![
        ("full-password", pw.as_bytes()),
        ("substring", "it's\"q".as_bytes()),
    ];
    assert_no_password_in_instance_files(&inst, &needles);
}

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_deep_socket_path_falls_back_and_cleans_up() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());
    let port = 42016u16;
    require_port_free(port);

    let deep_base = tmp.path().join("d".repeat(90)).join("e".repeat(90));
    fs::create_dir_all(&deep_base).expect("deep base");
    let inst = deep_base.join("inst");
    let data = inst.join("data");
    let logs = inst.join("logs");
    let run = inst.join("run");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&run).unwrap();
    let preferred = run.join(format!("mysqld-{port}.sock"));
    assert!(
        preferred.as_os_str().as_encoded_bytes().len() > 120,
        "test setup must create a deep preferred socket path, got {}",
        preferred.as_os_str().as_encoded_bytes().len()
    );

    let pw = "deep-socket-pw";
    MysqlAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("initialize mysql deep socket");
    let cnf = run.join("my.cnf");
    let actual_sock = socket_path_from_my_cnf(&cnf);
    assert_ne!(actual_sock, preferred, "deep path must use fallback socket");
    assert!(
        actual_sock.as_os_str().as_encoded_bytes().len()
            <= dblocal_core::engine::mysql_socket_path_limit(),
        "fallback socket too long: {}",
        actual_sock.display()
    );
    let fallback_dir = actual_sock.parent().expect("fallback dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(fallback_dir).expect("meta");
        assert!(meta.is_dir());
        assert_eq!(meta.uid(), nix::unistd::geteuid().as_raw());
        assert_eq!(meta.mode() & 0o777, 0o700);
    }

    let mut guard = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not listen; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    let ctx = run_ctx(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    let h = MysqlAdapter.health_check(&ctx).expect("health");
    assert!(matches!(h, dblocal_core::engine::Health::Ready));
    let out = mysql_client(&rt, port, pw, "SELECT 1;");
    assert!(
        out.status.success(),
        "tcp login failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    MysqlAdapter.shutdown(&ctx).expect("shutdown");
    if let Some(mut c) = guard.0.take() {
        let _ = c.wait();
    }
    assert!(
        wait_tcp_gone(port, Duration::from_secs(30)),
        "port still listening"
    );
    assert!(
        !actual_sock.exists(),
        "fallback socket should be removed after shutdown: {}",
        actual_sock.display()
    );
}

#[test]
#[ignore = "e2e: requires cached MySQL tarball; run with DBLOCAL_E2E_MYSQL=1 under flock"]
fn mysql_84_deep_socket_restart_after_volatile_dirs_disappear() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(MYSQL_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_xz(&archive, tmp.path());
    let rt = mysql_runtime(tmp.path());
    let port = 42017u16;
    require_port_free(port);

    let deep_base = tmp.path().join("d".repeat(90)).join("e".repeat(90));
    fs::create_dir_all(&deep_base).expect("deep base");
    let inst = deep_base.join("inst");
    let data = inst.join("data");
    let logs = inst.join("logs");
    let run = inst.join("run");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&run).unwrap();
    let preferred = run.join(format!("mysqld-{port}.sock"));
    assert!(
        preferred.as_os_str().as_encoded_bytes().len() > 120,
        "test setup must create a deep preferred socket path"
    );

    let pw = "deep-restart-pw";
    MysqlAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("initialize mysql deep socket");
    let cnf = run.join("my.cnf");
    let actual_sock = socket_path_from_my_cnf(&cnf);
    assert_ne!(actual_sock, preferred, "deep path must use fallback socket");
    let shared_fallback_dir = actual_sock.parent().expect("fallback dir").to_path_buf();
    let private_tmpdir = run.join("tmp");

    let mut guard = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not listen; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    let ctx = run_ctx(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    assert!(matches!(
        MysqlAdapter.health_check(&ctx).expect("health"),
        dblocal_core::engine::Health::Ready
    ));
    MysqlAdapter.shutdown(&ctx).expect("shutdown");
    if let Some(mut c) = guard.0.take() {
        let _ = c.wait();
    }
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));

    // A reboot wipes /run/user/<uid> and /tmp, and a restored backup may lose
    // run/tmp. Both are volatile: the start path must rebuild them.
    //
    // The shared fallback directory (`$XDG_RUNTIME_DIR/dblocal` or
    // `/tmp/dblocal-<uid>`) belongs to every DBLocal instance of this user, so
    // this test must never delete it. Point the socket at a private short
    // directory we create under /tmp, delete THAT directory, then run
    // `prepare` and start, which proves the same recreation without touching
    // another instance's live sockets.
    assert!(
        actual_sock.as_os_str().as_encoded_bytes().len() <= 60,
        "fallback socket should live in a short volatile directory: {}",
        actual_sock.display()
    );
    assert!(
        shared_fallback_dir.is_dir(),
        "the shared fallback directory must survive the test"
    );
    let private = tempfile::Builder::new()
        .prefix("mysql-sock-")
        .tempdir_in("/tmp")
        .expect("private socket dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(private.path(), fs::Permissions::from_mode(0o700))
            .expect("chmod private socket dir");
    }
    let private_sock = private.path().join(
        actual_sock
            .file_name()
            .expect("socket file name")
            .to_string_lossy()
            .into_owned(),
    );
    rewrite_socket_in_my_cnf(&cnf, &private_sock);
    let private_dir_path = private.path().to_path_buf();
    private.close().expect("remove private socket dir");
    assert!(!private_dir_path.exists());

    fs::remove_dir_all(&private_tmpdir).expect("remove private tmpdir");
    assert!(!private_tmpdir.exists());

    // The service calls prepare, then builds the spawn command.
    dblocal_core::engine::mysql_prepare_run(&ctx).expect("prepare recreates volatile dirs");
    assert!(private_dir_path.is_dir(), "socket dir must be recreated");
    assert!(private_tmpdir.is_dir(), "private tmpdir must be recreated");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for dir in [&private_dir_path, &private_tmpdir] {
            let meta = fs::symlink_metadata(dir).expect("meta");
            assert_eq!(meta.uid(), nix::unistd::geteuid().as_raw());
            assert_eq!(meta.mode() & 0o777, 0o700, "{} must be 0700", dir.display());
        }
    }

    let cmd = MysqlAdapter.spawn_command(&ctx);
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut guard2 = KillOnDrop(Some(c.spawn().expect("spawn mysqld")), "mysqld-restart");
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not restart after the volatile dirs were rebuilt; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    assert!(matches!(
        MysqlAdapter.health_check(&ctx).expect("health restart"),
        dblocal_core::engine::Health::Ready
    ));
    let out = mysql_client(&rt, port, pw, "SELECT 1;");
    assert!(
        out.status.success(),
        "tcp login after restart failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        socket_path_from_my_cnf(&cnf),
        private_sock,
        "the server uses the rewritten socket path"
    );
    MysqlAdapter.shutdown(&ctx).expect("shutdown restart");
    if let Some(mut c) = guard2.0.take() {
        let _ = c.wait();
    }
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));
}
