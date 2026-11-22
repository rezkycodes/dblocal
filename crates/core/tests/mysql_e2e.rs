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
use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use dblocal_core::engine::{EngineAdapter, MysqlAdapter};
use dblocal_core::model::{AuthMode, Port};
use dblocal_core::secret::SecretString;

const MYSQL_ARCHIVE: &str = "mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz";
const MYSQL_DIR: &str = "mysql-8.4.11-linux-glibc2.28-x86_64-minimal";

struct KillOnDrop(Option<Child>, &'static str);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("killed {}", self.1);
        }
    }
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cache/dblocal-dev")
}

fn require_e2e() {
    if std::env::var("DBLOCAL_E2E_MYSQL").ok().as_deref() != Some("1") {
        panic!("set DBLOCAL_E2E_MYSQL=1 to run real MySQL e2e tests");
    }
}

fn extract_tar_xz(archive: &Path, dest: &Path) {
    let status = Command::new("tar")
        .args(["-xJf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .expect("tar must be available");
    assert!(status.success(), "failed to extract {}", archive.display());
}

fn wait_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(150),
        )
        .is_ok()
        {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_tcp_gone(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(100),
        )
        .is_err()
        {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn require_port_free(port: u16) {
    if TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(150),
    )
    .is_ok()
    {
        panic!("port {port} already in use before test");
    }
}

fn read_log_tail(path: &Path, max_lines: usize) -> String {
    let mut s = String::new();
    if let Ok(mut f) = fs::File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn mysql_runtime(extracted: &Path) -> PathBuf {
    extracted.join(MYSQL_DIR)
}

fn mk_instance(base: &Path, name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let inst = base.join(name);
    let data = inst.join("data");
    let logs = inst.join("logs");
    let run = inst.join("run");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&run).unwrap();
    fs::create_dir_all(data.parent().unwrap()).unwrap();
    (inst, data, logs, run)
}

fn init_ctx(
    runtime: &Path,
    data: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> dblocal_core::engine::InitContext {
    dblocal_core::engine::InitContext {
        instance_name: "my-e2e".into(),
        runtime_dir: runtime.to_path_buf(),
        data_dir: data.to_path_buf(),
        port: Port::new(port).unwrap(),
        superuser: "root".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

fn run_ctx(
    runtime: &Path,
    data: &Path,
    logs: &Path,
    run: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> dblocal_core::engine::RunContext {
    dblocal_core::engine::RunContext {
        instance_name: "my-e2e".into(),
        runtime_dir: runtime.to_path_buf(),
        data_dir: data.to_path_buf(),
        logs_dir: logs.to_path_buf(),
        run_dir: run.to_path_buf(),
        port: Port::new(port).unwrap(),
        superuser: "root".into(),
        auth,
        credential: password
            .map(|p| dblocal_core::engine::Credential::new("root", SecretString::from(p))),
    }
}

fn assert_no_password_in_args(cmd: &Command, secret: &str) {
    for arg in cmd.get_args() {
        assert!(
            !arg.to_string_lossy().contains(secret),
            "password leaked in arg: {:?}",
            arg
        );
    }
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            assert!(
                !v.to_string_lossy().contains(secret),
                "password leaked in env {:?}",
                k
            );
        }
    }
}

fn mysql_start(
    runtime: &Path,
    data: &Path,
    logs: &Path,
    run: &Path,
    port: u16,
    password: &str,
) -> KillOnDrop {
    let ctx = run_ctx(
        runtime,
        data,
        logs,
        run,
        port,
        AuthMode::Password,
        Some(password),
    );
    let cmd = MysqlAdapter.spawn_command(&ctx);
    assert_no_password_in_args(&cmd, password);
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = c.spawn().expect("spawn mysqld");
    KillOnDrop(Some(child), "mysqld")
}

fn mysql_client(runtime: &Path, port: u16, password: &str, sql: &str) -> std::process::Output {
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
        .env("MYSQL_PWD", password)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("mysql client")
}

fn assert_only_instance_loopback_port(pid: u32, expected_port: u16) {
    let out = Command::new("ss")
        .args(["-ltnp"])
        .stdout(Stdio::piped())
        .output()
        .expect("ss");
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!("pid={pid},");
    let mut found_expected = false;
    for line in text.lines() {
        if !line.contains(&needle) {
            continue;
        }
        // Expect only 127.0.0.1:<expected_port>
        let parts: Vec<&str> = line.split_whitespace().collect();
        let local = parts.get(3).copied().unwrap_or("");
        assert!(
            local == format!("127.0.0.1:{expected_port}"),
            "mysqld pid {pid} has unexpected listener {local:?} in line: {line}"
        );
        found_expected = true;
    }
    assert!(
        found_expected,
        "no ss listener found for mysqld pid {pid}; ss:\n{text}"
    );
}

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
    let mut guard = mysql_start(&rt, &data, &logs, &run, port, pw);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "mysql did not listen; log:\n{}",
        read_log_tail(&logs.join("error.log"), 120)
    );
    let pid = guard.0.as_ref().unwrap().id();
    assert_only_instance_loopback_port(pid, port);

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
