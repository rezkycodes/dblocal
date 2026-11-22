//! Real MariaDB integration tests. Gated by DBLOCAL_E2E=1.
//!
//! Run under the machine lock:
//! flock -w 3600 "$HOME/.cache/dblocal-e2e.lock" env DBLOCAL_E2E=1 \
//!   CARGO_TARGET_DIR="$HOME/.cache/dblocal-target" \
//!   cargo test -p dblocal-core --test mariadb_e2e -- --ignored --nocapture
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use dblocal_core::engine::{EngineAdapter, MariaDbAdapter};
use dblocal_core::model::{AuthMode, Port};
use dblocal_core::secret::SecretString;

const MDB118_ARCHIVE: &str = "mariadb-11.8.10-linux-systemd-x86_64.tar.gz";
const MDB114_ARCHIVE: &str = "mariadb-11.4.14-linux-systemd-x86_64.tar.gz";

fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!("set DBLOCAL_E2E=1 to run real-engine integration tests");
    }
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cache/dblocal-dev")
}

fn extract_tar_gz(archive: &Path, dest: &Path) {
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .expect("tar");
    assert!(status.success(), "extract failed: {}", archive.display());
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

fn wait_pid_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        #[cfg(target_os = "linux")]
        {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok();
            let alive = stat
                .as_deref()
                .map(|s| {
                    let close = s.rfind(')').unwrap_or(0);
                    let rest = &s[close + 1..];
                    !matches!(rest.split_whitespace().next(), Some("Z"))
                })
                .unwrap_or(false);
            if !alive {
                return true;
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let status = Command::new("kill").args(["-0", &pid.to_string()]).status();
            if !status.map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .expect("pidfile")
        .trim()
        .parse()
        .expect("pid")
}

fn runtime_dir(extracted: &Path, version: &str) -> PathBuf {
    extracted.join(format!("mariadb-{version}-linux-systemd-x86_64"))
}

fn mk_instance(base: &Path, name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let inst = base.join(name);
    let data = inst.join("data");
    let logs = inst.join("logs");
    let run = inst.join("run");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&run).unwrap();
    fs::create_dir_all(&data).unwrap();
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
        instance_name: "mdb-e2e".into(),
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
        instance_name: "mdb-e2e".into(),
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

struct KillOnDrop(Option<std::process::Child>, &'static str);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("killed {}", self.1);
        }
    }
}

fn start_mariadb(
    runtime: &Path,
    data: &Path,
    logs: &Path,
    run: &Path,
    port: u16,
    auth: AuthMode,
    password: Option<&str>,
) -> KillOnDrop {
    let ctx = run_ctx(runtime, data, logs, run, port, auth, password);
    let cmd = MariaDbAdapter.spawn_command(&ctx);
    if let Some(pw) = password {
        assert_no_password_in_args(&cmd, pw);
    }
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(v) = std::env::var_os("LD_LIBRARY_PATH") {
        c.env("LD_LIBRARY_PATH", v);
    }
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
    let child = c.spawn().expect("spawn mariadbd");
    KillOnDrop(Some(child), "mariadbd")
}

fn mariadb_client(runtime: &Path) -> PathBuf {
    let p = runtime.join("bin").join("mariadb");
    if p.exists() {
        p
    } else {
        runtime.join("bin").join("mysql")
    }
}

fn sql_query(runtime: &Path, port: u16, user: &str, password: Option<&str>, sql: &str) -> String {
    let client = mariadb_client(runtime);
    let mut cmd = Command::new(client);
    cmd.args([
        "-h",
        "127.0.0.1",
        "-P",
        &port.to_string(),
        "-u",
        user,
        "--protocol=tcp",
        "-N",
        "-B",
        "-e",
        sql,
    ]);
    if let Some(pw) = password {
        cmd.env("MYSQL_PWD", pw);
    } else {
        cmd.env_remove("MYSQL_PWD");
    }
    let out = cmd.output().expect("mariadb client");
    assert!(
        out.status.success(),
        "sql failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sql_try(
    runtime: &Path,
    port: u16,
    user: &str,
    password: Option<&str>,
    sql: &str,
) -> std::process::Output {
    let client = mariadb_client(runtime);
    let mut cmd = Command::new(client);
    cmd.args([
        "-h",
        "127.0.0.1",
        "-P",
        &port.to_string(),
        "-u",
        user,
        "--protocol=tcp",
        "-N",
        "-B",
        "-e",
        sql,
    ]);
    if let Some(pw) = password {
        cmd.env("MYSQL_PWD", pw);
    } else {
        cmd.env_remove("MYSQL_PWD");
    }
    cmd.output().expect("mariadb client")
}

fn assert_no_ss_outside(port: u16) {
    let out = Command::new("ss").args(["-ltnp"]).output().expect("ss");
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if !line.contains(&format!(":{port}")) {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        let local = cols.get(3).copied().unwrap_or("");
        assert!(
            local.starts_with("127.0.0.1:") || local.starts_with("[::1]:"),
            "listener not on loopback: {line}"
        );
        assert!(
            !local.starts_with("0.0.0.0:")
                && !local.starts_with("*:")
                && !local.starts_with("[::]:"),
            "listener on wildcard: {line}"
        );
    }
}

fn ensure_libcrypt_shim(base: &Path) -> Option<PathBuf> {
    let probe = Command::new("ldconfig").arg("-p").output().ok();
    let has_so1 = probe
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("libcrypt.so.1"))
        .unwrap_or(false);
    if has_so1 {
        return None;
    }
    let shim = base.join("libshim");
    fs::create_dir_all(&shim).unwrap();
    // Try to provide libcrypt.so.1 from libxcrypt compat locations, else fall back
    // to libcrypt.so.2 symlink (works on Fedora where symbols are compatible).
    let candidates = [
        "/usr/lib64/libcrypt.so.1",
        "/lib64/libcrypt.so.1",
        "/usr/lib/x86_64-linux-gnu/libcrypt.so.1",
        "/usr/lib64/libcrypt.so.2",
        "/lib64/libcrypt.so.2",
    ];
    for target in candidates {
        let t = Path::new(target);
        if t.exists() {
            let link = shim.join("libcrypt.so.1");
            let _ = fs::remove_file(&link);
            std::os::unix::fs::symlink(t, &link).unwrap();
            return Some(shim);
        }
    }
    None
}

fn shared_runtime_118() -> PathBuf {
    static RUNTIME: OnceLock<PathBuf> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let base = PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                .join(".cache/dblocal-e2e-shared");
            fs::create_dir_all(&base).expect("shared base");
            maybe_install_libcrypt_shim(&base);
            let cache = cache_dir();
            let a118 = cache.join(MDB118_ARCHIVE);
            assert!(a118.is_file(), "missing {}", a118.display());
            let rt = runtime_dir(&base, "11.8.10");
            if !rt.join("bin").join("mariadbd").is_file() {
                extract_tar_gz(&a118, &base);
            }
            rt
        })
        .clone()
}

fn shared_runtime_114() -> Option<PathBuf> {
    static RUNTIME: OnceLock<Option<PathBuf>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let base = PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                .join(".cache/dblocal-e2e-shared");
            fs::create_dir_all(&base).expect("shared base");
            maybe_install_libcrypt_shim(&base);
            let cache = cache_dir();
            let a114 = cache.join(MDB114_ARCHIVE);
            if !a114.is_file() {
                return None;
            }
            let rt = runtime_dir(&base, "11.4.14");
            if !rt.join("bin").join("mariadbd").is_file() {
                extract_tar_gz(&a114, &base);
            }
            Some(rt)
        })
        .clone()
}

fn maybe_install_libcrypt_shim(base: &Path) {
    if let Some(shim) = ensure_libcrypt_shim(base) {
        let prev = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let val = if prev.is_empty() {
            shim.display().to_string()
        } else {
            format!("{}:{prev}", shim.display())
        };
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("LD_LIBRARY_PATH", val)
        };
    }
}

fn setup_runtime(_tmp: &Path) -> PathBuf {
    shared_runtime_118()
}

fn scan_dir_for_secrets(root: &Path, needles: &[&str]) -> Vec<(PathBuf, String)> {
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            for needle in needles {
                if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    hits.push((path.clone(), (*needle).to_owned()));
                }
            }
        }
    }
    hits
}

#[test]
#[ignore = "e2e: requires cached MariaDB 11.8 tarball; run with DBLOCAL_E2E=1 under flock $HOME/.cache/dblocal-e2e.lock"]
fn mariadb_118_password_cycle() {
    require_e2e();
    let tmp = tempfile::tempdir().expect("tmp");
    let rt = setup_runtime(tmp.path());
    let port = 43100u16;
    require_port_free(port);
    let (_i, data, logs, run) = mk_instance(tmp.path(), "pw");
    let pw = "e2e-mdb-secret-123";

    MariaDbAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("init");

    let mut g = start_mariadb(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "listen failed; log:\n{}",
        fs::read_to_string(logs.join("mariadb.err")).unwrap_or_default()
    );
    let pid = read_pid(&data.join("mariadbd.pid"));

    let h = MariaDbAdapter
        .health_check(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("health");
    assert!(matches!(h, dblocal_core::engine::Health::Ready));

    let ver = sql_query(&rt, port, "root", Some(pw), "SELECT VERSION();");
    assert!(ver.contains("11.8.10-MariaDB"), "version: {ver}");

    let bad = sql_try(&rt, port, "root", Some("wrong"), "SELECT 1;");
    assert!(!bad.status.success(), "wrong password should fail");

    sql_query(
        &rt,
        port,
        "root",
        Some(pw),
        "CREATE DATABASE e2e; CREATE TABLE e2e.t (id INT); INSERT INTO e2e.t VALUES (42);",
    );
    assert_no_ss_outside(port);

    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("shutdown");
    let _ = g.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));
    assert!(wait_pid_gone(pid, Duration::from_secs(30)));
    assert!(!data.join("mariadbd.pid").exists() || wait_pid_gone(pid, Duration::from_secs(1)));

    // restart, data survives
    let mut g2 = start_mariadb(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    assert!(wait_tcp(port, Duration::from_secs(30)), "restart listen");
    let got = sql_query(&rt, port, "root", Some(pw), "SELECT id FROM e2e.t;");
    assert_eq!(got, "42");
    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("shutdown2");
    let _ = g2.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));
    eprintln!("mariadb password cycle OK");
}

#[test]
#[ignore = "e2e: requires cached MariaDB 11.8 tarball; run with DBLOCAL_E2E=1 under flock $HOME/.cache/dblocal-e2e.lock"]
fn mariadb_118_no_password_cycle() {
    require_e2e();
    let tmp = tempfile::tempdir().expect("tmp");
    let rt = setup_runtime(tmp.path());
    let port = 43103u16;
    require_port_free(port);
    let (inst, data, logs, run) = mk_instance(tmp.path(), "nopw");

    MariaDbAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::None, None))
        .expect("init");

    let mut g = start_mariadb(&rt, &data, &logs, &run, port, AuthMode::None, None);
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "listen failed; log:\n{}",
        fs::read_to_string(logs.join("mariadb.err")).unwrap_or_default()
    );

    let h = MariaDbAdapter
        .health_check(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::None,
            None,
        ))
        .expect("health");
    assert!(matches!(h, dblocal_core::engine::Health::Ready));

    let who = sql_query(&rt, port, "root", None, "SELECT CURRENT_USER();");
    assert!(
        who == "root@127.0.0.1" || who == "root@localhost" || who.starts_with("root@"),
        "unexpected current_user: {who}"
    );
    // Verify 127.0.0.1 TCP account exists and is usable
    let accounts = sql_query(
        &rt,
        port,
        "root",
        None,
        "SELECT CONCAT(User,'@',Host) FROM mysql.user WHERE User='root' ORDER BY Host;",
    );
    eprintln!("root accounts: {accounts}");
    assert!(
        accounts.contains("root@127.0.0.1"),
        "missing root@127.0.0.1: {accounts}"
    );

    let uri = MariaDbAdapter.connection_uri_no_password(
        &run_ctx(&rt, &data, &logs, &run, port, AuthMode::None, None),
        dblocal_core::service::UriFormat::Uri,
    );
    assert_eq!(uri.expose(), format!("mysql://root@127.0.0.1:{port}/"));
    let env = MariaDbAdapter.connection_uri_no_password(
        &run_ctx(&rt, &data, &logs, &run, port, AuthMode::None, None),
        dblocal_core::service::UriFormat::DotEnv,
    );
    assert!(
        !env.expose().contains("DB_PASSWORD"),
        "no-password env must not have DB_PASSWORD"
    );

    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::None,
            None,
        ))
        .expect("shutdown");
    let _ = g.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));
    let sock = run.join(format!("mariadb-{port}.sock"));
    assert!(
        !sock.exists(),
        "socket should be gone after shutdown: {}",
        sock.display()
    );
    let _ = inst;
    eprintln!("mariadb no-password cycle OK");
}

#[test]
#[ignore = "e2e: requires cached MariaDB 11.8 tarball; run with DBLOCAL_E2E=1 under flock $HOME/.cache/dblocal-e2e.lock"]
fn mariadb_118_hostile_password_and_no_file_leak() {
    require_e2e();
    let tmp = tempfile::tempdir().expect("tmp");
    let rt = setup_runtime(tmp.path());
    let port = 43104u16;
    require_port_free(port);
    let (_inst, data, logs, run) = mk_instance(tmp.path(), "hostile");
    let pw = "Cx9!it's\"q\\z$HOME`id`;&&%*-é";

    MariaDbAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("init");

    let mut g = start_mariadb(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "listen failed; log:\n{}",
        fs::read_to_string(logs.join("mariadb.err")).unwrap_or_default()
    );

    let who = sql_query(&rt, port, "root", Some(pw), "SELECT CURRENT_USER();");
    assert!(
        who == "root@127.0.0.1" || who.starts_with("root@"),
        "unexpected user: {who}"
    );

    // unix-socket login
    let sock = resolve_socket_path(&data, port);
    let client = mariadb_client(&rt);
    let sock_out = Command::new(&client)
        .args([
            "-S",
            sock.to_str().expect("sock utf8"),
            "-u",
            "root",
            "-N",
            "-B",
            "-e",
            "SELECT CURRENT_USER();",
        ])
        .env("MYSQL_PWD", pw)
        .output()
        .expect("socket login");
    assert!(
        sock_out.status.success(),
        "socket login failed: {} {}",
        String::from_utf8_lossy(&sock_out.stdout),
        String::from_utf8_lossy(&sock_out.stderr)
    );
    let sock_user = String::from_utf8_lossy(&sock_out.stdout).trim().to_string();
    assert!(sock_user.starts_with("root@"), "socket user: {sock_user}");

    // empty password must be refused over socket
    let bad_sock = Command::new(&client)
        .args([
            "-S",
            sock.to_str().expect("sock utf8"),
            "-u",
            "root",
            "-N",
            "-B",
            "-e",
            "SELECT 1;",
        ])
        .env_remove("MYSQL_PWD")
        .output()
        .expect("bad socket");
    assert!(
        !bad_sock.status.success(),
        "empty password on socket should fail"
    );

    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("shutdown");
    let _ = g.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));

    let needles = [pw, "it's\"q"];
    let inst_dir = data.parent().expect("inst dir");
    let hits = scan_dir_for_secrets(inst_dir, &needles);
    assert!(
        hits.is_empty(),
        "password leaked in files: {:?}",
        hits.iter().take(10).collect::<Vec<_>>()
    );
    eprintln!("mariadb hostile password OK");
}

fn resolve_socket_path(data_dir: &Path, port: u16) -> PathBuf {
    // Read socket path from my.cnf to be correct with fallback.
    let conf = data_dir.join("my.cnf");
    let text = fs::read_to_string(conf).expect("my.cnf");
    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("socket=") {
            return PathBuf::from(v.trim());
        }
    }
    // fallback to natural
    data_dir
        .parent()
        .unwrap_or(data_dir)
        .join("run")
        .join(format!("mariadb-{port}.sock"))
}

#[test]
#[ignore = "e2e: requires cached MariaDB 11.8 tarball; run with DBLOCAL_E2E=1 under flock $HOME/.cache/dblocal-e2e.lock"]
fn mariadb_118_long_socket_path_fallback() {
    require_e2e();
    let tmp = tempfile::tempdir().expect("tmp");
    let rt = setup_runtime(tmp.path());
    let port = 43105u16;
    require_port_free(port);

    // Deepen the run dir only so the natural socket path exceeds 120 bytes,
    // while keeping data/logs short enough for mariadb-install-db.
    let inst = tmp.path().join("lsock");
    let data = inst.join("data");
    let logs = inst.join("logs");
    let mut run = inst.join("run");
    for _ in 0..8 {
        run = run.join("very_long_directory_name_for_socket_path_test");
    }
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&run).unwrap();
    fs::create_dir_all(&data).unwrap();

    let natural = run.join(format!("mariadb-{port}.sock"));
    assert!(
        natural.as_os_str().as_encoded_bytes().len() > 120,
        "test setup: natural socket path must be > 120 bytes, got {}: {}",
        natural.as_os_str().as_encoded_bytes().len(),
        natural.display()
    );

    let pw = "e2e-long-sock";
    MariaDbAdapter
        .initialize(&init_ctx(&rt, &data, port, AuthMode::Password, Some(pw)))
        .expect("init with long socket path should succeed via fallback");

    let actual_sock = resolve_socket_path(&data, port);
    assert_ne!(
        actual_sock,
        natural,
        "should have used fallback socket, not natural: {}",
        actual_sock.display()
    );
    assert!(
        actual_sock.as_os_str().as_encoded_bytes().len() <= 107,
        "fallback socket too long: {} ({})",
        actual_sock.display(),
        actual_sock.as_os_str().as_encoded_bytes().len()
    );

    let mut g = start_mariadb(&rt, &data, &logs, &run, port, AuthMode::Password, Some(pw));
    assert!(
        wait_tcp(port, Duration::from_secs(30)),
        "listen failed; log:\n{}",
        fs::read_to_string(logs.join("mariadb.err")).unwrap_or_default()
    );

    let who = sql_query(&rt, port, "root", Some(pw), "SELECT CURRENT_USER();");
    assert!(who.starts_with("root@"), "who: {who}");
    assert_no_ss_outside(port);

    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt,
            &data,
            &logs,
            &run,
            port,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("shutdown");
    let _ = g.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port, Duration::from_secs(30)));

    // fallback socket should be gone after shutdown
    if actual_sock.exists() {
        // give it a moment
        thread::sleep(Duration::from_millis(500));
    }
    assert!(
        !actual_sock.exists(),
        "fallback socket should be removed after shutdown: {}",
        actual_sock.display()
    );
    eprintln!("mariadb long socket fallback OK: {}", actual_sock.display());
}

#[test]
#[ignore = "e2e: requires cached MariaDB 11.8 and 11.4 tarballs; run with DBLOCAL_E2E=1 under flock $HOME/.cache/dblocal-e2e.lock"]
fn mariadb_two_instances_side_by_side() {
    require_e2e();
    let cache = cache_dir();
    let a118 = cache.join(MDB118_ARCHIVE);
    assert!(a118.is_file(), "missing {}", a118.display());

    let rt118 = setup_runtime(Path::new("."));

    let port1 = 43106u16;
    let port2 = 43107u16;
    require_port_free(port1);
    require_port_free(port2);

    let pw = "e2e-side";
    for round in 0..5 {
        let mut handles = Vec::new();
        for idx in 0..2 {
            let rt = rt118.clone();
            let port = if idx == 0 { port1 } else { port2 };
            handles.push(thread::spawn(move || {
                let tmp = tempfile::tempdir().expect("tmp");
                let base = tmp.path().join(format!("r{round}-i{idx}"));
                fs::create_dir_all(&base).expect("base");
                let (_i, d, l, r) = mk_instance(&base, "s");
                MariaDbAdapter
                    .initialize(&init_ctx(&rt, &d, port, AuthMode::Password, Some(pw)))
                    .expect("init");
                let mut g = start_mariadb(&rt, &d, &l, &r, port, AuthMode::Password, Some(pw));
                assert!(
                    wait_tcp(port, Duration::from_secs(30)),
                    "listen r{round} i{idx}"
                );
                MariaDbAdapter
                    .shutdown(&run_ctx(&rt, &d, &l, &r, port, AuthMode::Password, Some(pw)))
                    .expect("shutdown");
                let _ = g.0.take().map(|mut c| c.wait());
                assert!(wait_tcp_gone(port, Duration::from_secs(30)));
            }));
        }
        for h in handles {
            h.join().expect("round thread");
        }
    }

    let tmp = tempfile::tempdir().expect("tmp");
    let (_i1, d1, l1, r1) = mk_instance(tmp.path(), "s1");
    let (_i2, d2, l2, r2) = mk_instance(tmp.path(), "s2");

    MariaDbAdapter
        .initialize(&init_ctx(&rt118, &d1, port1, AuthMode::Password, Some(pw)))
        .expect("init 1");
    MariaDbAdapter
        .initialize(&init_ctx(&rt118, &d2, port2, AuthMode::Password, Some(pw)))
        .expect("init 2");

    let mut g1 = start_mariadb(&rt118, &d1, &l1, &r1, port1, AuthMode::Password, Some(pw));
    let mut g2 = start_mariadb(&rt118, &d2, &l2, &r2, port2, AuthMode::Password, Some(pw));
    assert!(wait_tcp(port1, Duration::from_secs(30)), "p1 listen");
    assert!(wait_tcp(port2, Duration::from_secs(30)), "p2 listen");

    let v1 = sql_query(&rt118, port1, "root", Some(pw), "SELECT VERSION();");
    let v2 = sql_query(&rt118, port2, "root", Some(pw), "SELECT VERSION();");
    assert!(v1.contains("11.8.10-MariaDB"));
    assert!(v2.contains("11.8.10-MariaDB"));
    assert_no_ss_outside(port1);
    assert_no_ss_outside(port2);

    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt118,
            &d1,
            &l1,
            &r1,
            port1,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("shutdown 1");
    MariaDbAdapter
        .shutdown(&run_ctx(
            &rt118,
            &d2,
            &l2,
            &r2,
            port2,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("shutdown 2");
    let _ = g1.0.take().map(|mut c| c.wait());
    let _ = g2.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port1, Duration::from_secs(30)));
    assert!(wait_tcp_gone(port2, Duration::from_secs(30)));

    if let Some(rt114) = shared_runtime_114() {
        let port3 = 43108u16;
        require_port_free(port3);
        let (_i3, d3, l3, r3) = mk_instance(tmp.path(), "s114");
        match MariaDbAdapter.initialize(&init_ctx(&rt114, &d3, port3, AuthMode::Password, Some(pw)))
        {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("{e:?}");
                if msg.contains("libcrypt.so.1")
                    || msg.contains("error while loading shared libraries")
                {
                    eprintln!("skip 11.4 side-by-side: host cannot load 11.4 binary: {msg}");
                    eprintln!("mariadb side-by-side OK (11.8 only)");
                    return;
                }
                panic!("init 114: {e:?}");
            }
        }
        let mut g3 = start_mariadb(&rt114, &d3, &l3, &r3, port3, AuthMode::Password, Some(pw));
        assert!(wait_tcp(port3, Duration::from_secs(30)), "114 listen");
        let v3 = sql_query(&rt114, port3, "root", Some(pw), "SELECT VERSION();");
        assert!(v3.contains("11.4.14-MariaDB"), "v3: {v3}");
        MariaDbAdapter
            .shutdown(&run_ctx(
                &rt114,
                &d3,
                &l3,
                &r3,
                port3,
                AuthMode::Password,
                Some(pw),
            ))
            .expect("shutdown 114");
        let _ = g3.0.take().map(|mut c| c.wait());
        assert!(wait_tcp_gone(port3, Duration::from_secs(30)));
        eprintln!("side-by-side 11.4 OK: {v3}");
    } else {
        eprintln!("skip 11.4 side-by-side: archive not cached");
    }
    eprintln!("mariadb side-by-side OK");
}
