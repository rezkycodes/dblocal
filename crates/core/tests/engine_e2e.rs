//! Integration tests against real engine binaries cached in `$HOME/.cache/dblocal-dev/`.
//!
//! These are `#[ignore]`d so `cargo test` stays offline and never starts real
//! engines. Run explicitly:
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
//! DBLOCAL_E2E=1 cargo test -p dblocal-core --test engine_e2e -- --ignored --nocapture
//! ```
//!
//! Ports 41200-41219 only. Every process and temp directory is cleaned up.

// This file is an integration test: every unwrap/expect here is test setup.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use dblocal_core::engine::{
    EngineAdapter, PostgresAdapter, PostmasterIdentity, ValkeyAdapter, parse_pidfile,
    read_postmaster_pid,
};
use dblocal_core::model::{AuthMode, Port};
use dblocal_core::secret::SecretString;

const PG18_ARCHIVE: &str = "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz";
const PG17_ARCHIVE: &str = "postgresql-17.12.0-x86_64-unknown-linux-gnu.tar.gz";
const VK_JAMMY_ARCHIVE: &str = "valkey-9.2.0-jammy-x86_64.tar.gz";
const VK_NOBLE_ARCHIVE: &str = "valkey-9.2.0-noble-x86_64.tar.gz";

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
    let home = std::env::var_os("HOME").expect("HOME");
    PathBuf::from(home).join(".cache/dblocal-dev")
}

fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!("set DBLOCAL_E2E=1 to run real-engine integration tests");
    }
}

fn extract_tar_gz(archive: &Path, dest: &Path) {
    let status = Command::new("tar")
        .arg("-xzf")
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

/// Fails loudly when a port is already bound, so a stale engine from an earlier
/// run is never mistaken for a product bug.
fn require_port_free(port: u16) {
    if TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(150),
    )
    .is_ok()
    {
        panic!("port {port} is already in use before the test started");
    }
}

fn wait_pid_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
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

fn pg_runtime(extracted: &Path, version: &str) -> PathBuf {
    extracted.join(format!("postgresql-{version}-x86_64-unknown-linux-gnu"))
}

fn vk_runtime(extracted: &Path, version: &str, variant: &str) -> PathBuf {
    extracted.join(format!("valkey-{version}-{variant}-x86_64"))
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

fn pg_init_ctx(
    runtime: &Path,
    data: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> dblocal_core::engine::InitContext {
    dblocal_core::engine::InitContext {
        instance_name: "pg-e2e".into(),
        runtime_dir: runtime.to_path_buf(),
        data_dir: data.to_path_buf(),
        port: Port::new(port).unwrap(),
        superuser: "postgres".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

fn pg_run_ctx(
    runtime: &Path,
    data: &Path,
    logs: &Path,
    run: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> dblocal_core::engine::RunContext {
    dblocal_core::engine::RunContext {
        instance_name: "pg-e2e".into(),
        runtime_dir: runtime.to_path_buf(),
        data_dir: data.to_path_buf(),
        logs_dir: logs.to_path_buf(),
        run_dir: run.to_path_buf(),
        port: Port::new(port).unwrap(),
        superuser: "postgres".into(),
        auth,
        credential: password
            .map(|p| dblocal_core::engine::Credential::new("postgres", SecretString::from(p))),
    }
}

fn vk_init_ctx(
    runtime: &Path,
    data: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> dblocal_core::engine::InitContext {
    dblocal_core::engine::InitContext {
        instance_name: "vk-e2e".into(),
        runtime_dir: runtime.to_path_buf(),
        data_dir: data.to_path_buf(),
        port: Port::new(port).unwrap(),
        superuser: "default".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

fn vk_run_ctx(
    runtime: &Path,
    data: &Path,
    logs: &Path,
    run: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> dblocal_core::engine::RunContext {
    dblocal_core::engine::RunContext {
        instance_name: "vk-e2e".into(),
        runtime_dir: runtime.to_path_buf(),
        data_dir: data.to_path_buf(),
        logs_dir: logs.to_path_buf(),
        run_dir: run.to_path_buf(),
        port: Port::new(port).unwrap(),
        superuser: "default".into(),
        auth,
        credential: password
            .map(|p| dblocal_core::engine::Credential::new("default", SecretString::from(p))),
    }
}

fn assert_no_password_in_args(cmd: &Command, secret: &str) {
    for arg in cmd.get_args() {
        assert!(
            !arg.to_string_lossy().contains(secret),
            "password leaked in argv: {:?}",
            arg
        );
    }
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            assert!(
                !v.to_string_lossy().contains(secret),
                "password leaked in env {k:?}"
            );
        }
    }
}

fn pg_start(runtime: &Path, data: &Path, logs: &Path, run: &Path, port: u16) {
    let ctx = pg_run_ctx(
        runtime,
        data,
        logs,
        run,
        port,
        AuthMode::Password,
        Some("ignored-for-start"),
    );
    let cmd = PostgresAdapter.spawn_command(&ctx);
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => {
                c.env(k, v);
            }
            None => {
                c.env_remove(k);
            }
        }
    }
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = c.output().expect("pg_ctl start");
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let log = read_log_tail(&logs.join("postgres.log"), 80);
        panic!(
            "pg_ctl start failed: {:?}\nstdout:\n{}\nstderr:\n{}\nlog:\n{}",
            out.status.code(),
            stdout,
            stderr,
            log
        );
    }
}

fn pg_stop(runtime: &Path, data: &Path, logs: &Path, run: &Path, port: u16, password: &str) {
    let ctx = pg_run_ctx(
        runtime,
        data,
        logs,
        run,
        port,
        AuthMode::Password,
        Some(password),
    );
    PostgresAdapter.shutdown(&ctx).expect("pg_ctl stop");
}

fn pg_version(runtime: &Path, port: u16, password: &str) -> String {
    let psql = runtime.join("bin/psql");
    let out = Command::new(psql)
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-U",
            "postgres",
            "-Atc",
            "select version();",
        ])
        .env("PGPASSWORD", password)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("psql");
    assert!(
        out.status.success(),
        "psql failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
#[ignore = "e2e: requires cached engine binaries; run with DBLOCAL_E2E=1 cargo test -p dblocal-core --test engine_e2e -- --ignored --nocapture"]
fn postgres_18_17_side_by_side_full_cycle() {
    require_e2e();
    let cache = cache_dir();
    let pg18a = cache.join(PG18_ARCHIVE);
    let pg17a = cache.join(PG17_ARCHIVE);
    assert!(pg18a.is_file(), "missing {}", pg18a.display());
    assert!(pg17a.is_file(), "missing {}", pg17a.display());

    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_gz(&pg18a, tmp.path());
    extract_tar_gz(&pg17a, tmp.path());
    let rt18 = pg_runtime(tmp.path(), "18.7.0");
    let rt17 = pg_runtime(tmp.path(), "17.12.0");

    let (_, d18, l18, r18) = mk_instance(tmp.path(), "pg18");
    let (_, d17, l17, r17) = mk_instance(tmp.path(), "pg17");
    let pw18 = "e2e-pw-18-secret";
    let pw17 = "e2e-pw-17-secret";
    let port18 = 41200u16;
    let port17 = 41201u16;
    require_port_free(port18);
    require_port_free(port17);

    PostgresAdapter
        .initialize(&pg_init_ctx(
            &rt18,
            &d18,
            port18,
            AuthMode::Password,
            Some(pw18),
        ))
        .expect("init pg18");
    PostgresAdapter
        .initialize(&pg_init_ctx(
            &rt17,
            &d17,
            port17,
            AuthMode::Password,
            Some(pw17),
        ))
        .expect("init pg17");

    pg_start(&rt18, &d18, &l18, &r18, port18);
    assert!(
        wait_tcp(port18, Duration::from_secs(15)),
        "pg18 did not listen; log:\n{}",
        read_log_tail(&l18.join("postgres.log"), 80)
    );
    pg_start(&rt17, &d17, &l17, &r17, port17);
    assert!(
        wait_tcp(port17, Duration::from_secs(15)),
        "pg17 did not listen; log:\n{}",
        read_log_tail(&l17.join("postgres.log"), 80)
    );

    let id18: PostmasterIdentity = read_postmaster_pid(&d18).expect("pid 18");
    let id17: PostmasterIdentity = read_postmaster_pid(&d17).expect("pid 17");
    assert_eq!(id18.port, port18);
    assert_eq!(id17.port, port17);
    assert!(
        Path::new(&format!("/proc/{}", id18.pid)).exists(),
        "pg18 pid alive"
    );
    assert!(
        Path::new(&format!("/proc/{}", id17.pid)).exists(),
        "pg17 pid alive"
    );
    eprintln!("pg18 identity: pid={} epoch={}", id18.pid, id18.start_epoch);
    eprintln!("pg17 identity: pid={} epoch={}", id17.pid, id17.start_epoch);

    let h18 = PostgresAdapter
        .health_check(&pg_run_ctx(
            &rt18,
            &d18,
            &l18,
            &r18,
            port18,
            AuthMode::Password,
            Some(pw18),
        ))
        .expect("health 18");
    let h17 = PostgresAdapter
        .health_check(&pg_run_ctx(
            &rt17,
            &d17,
            &l17,
            &r17,
            port17,
            AuthMode::Password,
            Some(pw17),
        ))
        .expect("health 17");
    assert!(matches!(h18, dblocal_core::engine::Health::Ready));
    assert!(matches!(h17, dblocal_core::engine::Health::Ready));

    let v18 = pg_version(&rt18, port18, pw18);
    let v17 = pg_version(&rt17, port17, pw17);
    assert!(v18.contains("18.7"), "unexpected pg18 version: {v18}");
    assert!(v17.contains("17.12"), "unexpected pg17 version: {v17}");
    eprintln!("pg18 version: {v18}");
    eprintln!("pg17 version: {v17}");

    pg_stop(&rt18, &d18, &l18, &r18, port18, pw18);
    pg_stop(&rt17, &d17, &l17, &r17, port17, pw17);
    assert!(
        wait_tcp_gone(port18, Duration::from_secs(20)),
        "pg18 still listening"
    );
    assert!(
        wait_tcp_gone(port17, Duration::from_secs(20)),
        "pg17 still listening"
    );
    assert!(
        wait_pid_gone(id18.pid, Duration::from_secs(20)),
        "pg18 pid alive"
    );
    assert!(
        wait_pid_gone(id17.pid, Duration::from_secs(20)),
        "pg17 pid alive"
    );

    pg_start(&rt18, &d18, &l18, &r18, port18);
    pg_start(&rt17, &d17, &l17, &r17, port17);
    assert!(
        wait_tcp(port18, Duration::from_secs(15)),
        "pg18 did not restart"
    );
    assert!(
        wait_tcp(port17, Duration::from_secs(15)),
        "pg17 did not restart"
    );
    let h18b = PostgresAdapter
        .health_check(&pg_run_ctx(
            &rt18,
            &d18,
            &l18,
            &r18,
            port18,
            AuthMode::Password,
            Some(pw18),
        ))
        .expect("health 18 restart");
    let h17b = PostgresAdapter
        .health_check(&pg_run_ctx(
            &rt17,
            &d17,
            &l17,
            &r17,
            port17,
            AuthMode::Password,
            Some(pw17),
        ))
        .expect("health 17 restart");
    assert!(matches!(h18b, dblocal_core::engine::Health::Ready));
    assert!(matches!(h17b, dblocal_core::engine::Health::Ready));

    pg_stop(&rt18, &d18, &l18, &r18, port18, pw18);
    pg_stop(&rt17, &d17, &l17, &r17, port17, pw17);
    assert!(wait_tcp_gone(port18, Duration::from_secs(20)));
    assert!(wait_tcp_gone(port17, Duration::from_secs(20)));
    eprintln!("postgres side-by-side e2e OK");
}

fn vk_start(
    runtime: &Path,
    data: &Path,
    logs: &Path,
    run: &Path,
    port: u16,
    password: &str,
) -> KillOnDrop {
    let ctx = vk_run_ctx(
        runtime,
        data,
        logs,
        run,
        port,
        AuthMode::Password,
        Some(password),
    );
    let cmd = ValkeyAdapter.spawn_command(&ctx);
    assert_no_password_in_args(&cmd, password);
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = c.spawn().expect("spawn valkey-server");
    KillOnDrop(Some(child), "valkey-server")
}

#[test]
#[ignore = "e2e: requires cached engine binaries; run with DBLOCAL_E2E=1 cargo test -p dblocal-core --test engine_e2e -- --ignored --nocapture"]
fn valkey_jammy_and_noble_full_cycle() {
    require_e2e();
    let cache = cache_dir();
    let ja = cache.join(VK_JAMMY_ARCHIVE);
    let na = cache.join(VK_NOBLE_ARCHIVE);
    assert!(ja.is_file(), "missing {}", ja.display());
    assert!(na.is_file(), "missing {}", na.display());

    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_gz(&ja, tmp.path());
    extract_tar_gz(&na, tmp.path());
    require_port_free(41210);
    require_port_free(41211);

    for (variant, port) in [("jammy", 41210u16), ("noble", 41211u16)] {
        let rt = vk_runtime(tmp.path(), "9.2.0", variant);
        let (_, data, logs, run) = mk_instance(tmp.path(), &format!("vk-{variant}"));
        let pw = format!("vk-{variant}-secret-pw");
        ValkeyAdapter
            .initialize(&vk_init_ctx(
                &rt,
                &data,
                port,
                AuthMode::Password,
                Some(&pw),
            ))
            .expect("init valkey");

        let mut guard = vk_start(&rt, &data, &logs, &run, port, &pw);
        assert!(
            wait_tcp(port, Duration::from_secs(10)),
            "valkey {variant} did not listen; log:\n{}",
            read_log_tail(&logs.join("valkey.log"), 80)
        );
        let pid = parse_pidfile(&fs::read_to_string(run.join("valkey.pid")).unwrap())
            .expect("pidfile")
            .pid;
        assert!(
            Path::new(&format!("/proc/{pid}")).exists(),
            "valkey pid alive"
        );
        eprintln!("valkey {variant} pid={pid}");

        let ctx = vk_run_ctx(&rt, &data, &logs, &run, port, AuthMode::Password, Some(&pw));
        let h = ValkeyAdapter.health_check(&ctx).expect("health valkey");
        assert!(matches!(h, dblocal_core::engine::Health::Ready));

        ValkeyAdapter.shutdown(&ctx).expect("shutdown valkey");
        if let Some(mut c) = guard.0.take() {
            let _ = c.wait();
        }
        assert!(
            wait_tcp_gone(port, Duration::from_secs(10)),
            "valkey still listening"
        );
        assert!(
            wait_pid_gone(pid, Duration::from_secs(10)),
            "valkey pid alive"
        );

        let mut guard2 = vk_start(&rt, &data, &logs, &run, port, &pw);
        assert!(
            wait_tcp(port, Duration::from_secs(10)),
            "valkey {variant} restart listen; log:\n{}",
            read_log_tail(&logs.join("valkey.log"), 80)
        );
        let h2 = ValkeyAdapter.health_check(&ctx).expect("health restart");
        assert!(matches!(h2, dblocal_core::engine::Health::Ready));
        ValkeyAdapter.shutdown(&ctx).expect("shutdown restart");
        if let Some(mut c) = guard2.0.take() {
            let _ = c.wait();
        }
        assert!(wait_tcp_gone(port, Duration::from_secs(10)));
        eprintln!("valkey {variant} e2e OK");
    }
}

#[test]
#[ignore = "e2e: requires cached engine binaries; run with DBLOCAL_E2E=1 cargo test -p dblocal-core --test engine_e2e -- --ignored --nocapture"]
fn postgres_long_path_disables_unix_socket_and_stays_tcp_only() {
    require_e2e();
    let cache = cache_dir();
    let archive = cache.join(PG18_ARCHIVE);
    assert!(archive.is_file(), "missing {}", archive.display());

    let tmp = tempfile::tempdir().expect("tempdir");
    extract_tar_gz(&archive, tmp.path());
    let runtime = pg_runtime(tmp.path(), "18.7.0");

    // Build a deliberately deep instance path so that the full
    // <run>/sock/.s.PGSQL.<port> path exceeds the kernel's 107-byte limit.
    let deep = tmp
        .path()
        .join("d".repeat(30))
        .join("e".repeat(30))
        .join("f".repeat(40));
    fs::create_dir_all(&deep).expect("deep base");
    let inst = deep.join("pglong");
    let data = inst.join("data");
    let logs = inst.join("logs");
    let run = inst.join("run");
    fs::create_dir_all(&logs).expect("logs");
    fs::create_dir_all(&run).expect("run");

    let socket_path = run.join("sock").join(".s.PGSQL.41203");
    assert!(
        socket_path.as_os_str().len() > 107,
        "fixture must exceed 107 bytes, got {} ({})",
        socket_path.as_os_str().len(),
        socket_path.display()
    );

    let port = 41203u16;
    require_port_free(port);
    let password = "e2e-long-path-secret";
    PostgresAdapter
        .initialize(&pg_init_ctx(
            &runtime,
            &data,
            port,
            AuthMode::Password,
            Some(password),
        ))
        .expect("init under a long path must not fail");

    let conf = fs::read_to_string(data.join("postgresql.conf")).expect("conf");
    assert!(
        conf.contains("unix_socket_directories = ''"),
        "expected unix sockets disabled in postgresql.conf:\n{conf}"
    );
    assert!(
        conf.contains("listen_addresses = '127.0.0.1'"),
        "TCP loopback must still be configured"
    );

    pg_start(&runtime, &data, &logs, &run, port);
    assert!(
        wait_tcp(port, Duration::from_secs(15)),
        "postgres (long path) did not listen; log:\n{}",
        read_log_tail(&logs.join("postgres.log"), 80)
    );

    // select 1 over TCP proves the server is healthy without any unix socket.
    let health = PostgresAdapter
        .health_check(&pg_run_ctx(
            &runtime,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(password),
        ))
        .expect("health over TCP");
    assert!(matches!(health, dblocal_core::engine::Health::Ready));
    let version = pg_version(&runtime, port, password);
    assert!(version.contains("18.7"), "unexpected version: {version}");
    eprintln!("long-path postgres version over TCP: {version}");

    assert!(
        !socket_path.exists(),
        "a unix socket file must not exist when sockets are disabled: {}",
        socket_path.display()
    );
    assert!(
        !run.join("sock").exists(),
        "the socket directory should not even be created"
    );

    let pid = read_postmaster_pid(&data).expect("pidfile").pid;
    pg_stop(&runtime, &data, &logs, &run, port, password);
    assert!(
        wait_tcp_gone(port, Duration::from_secs(20)),
        "long-path postgres still listening"
    );
    assert!(wait_pid_gone(pid, Duration::from_secs(20)), "pid alive");

    // Restart works TCP-only too.
    pg_start(&runtime, &data, &logs, &run, port);
    assert!(wait_tcp(port, Duration::from_secs(15)), "did not restart");
    let health = PostgresAdapter
        .health_check(&pg_run_ctx(
            &runtime,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(password),
        ))
        .expect("health after restart");
    assert!(matches!(health, dblocal_core::engine::Health::Ready));
    assert!(!socket_path.exists(), "no unix socket after restart");
    pg_stop(&runtime, &data, &logs, &run, port, password);
    assert!(wait_tcp_gone(port, Duration::from_secs(20)));
    eprintln!("postgres long-path TCP-only e2e OK");
}

#[test]
#[ignore = "e2e: requires cached engine binaries; run with DBLOCAL_E2E=1 cargo test -p dblocal-core --test engine_e2e -- --ignored --nocapture"]
fn postgres_spawn_command_argv_has_no_password() {
    require_e2e();
    let tmp = tempfile::tempdir().expect("tempdir");
    let ctx = pg_run_ctx(
        tmp.path(),
        &tmp.path().join("data"),
        &tmp.path().join("logs"),
        &tmp.path().join("run"),
        41205,
        AuthMode::Password,
        Some("very-secret-argv"),
    );
    let cmd = PostgresAdapter.spawn_command(&ctx);
    assert_no_password_in_args(&cmd, "very-secret-argv");
}
