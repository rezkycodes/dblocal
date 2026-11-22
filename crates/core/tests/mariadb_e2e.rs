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
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return true;
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
    let cmd = MariaDbAdapter.spawn_command(&ctx);
    assert_no_password_in_args(&cmd, password);
    let mut c = Command::new(cmd.get_program());
    c.args(cmd.get_args());
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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

fn sql_query(runtime: &Path, port: u16, user: &str, password: &str, sql: &str) -> String {
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
    cmd.env("MYSQL_PWD", password);
    let out = cmd.output().expect("mariadb client");
    assert!(
        out.status.success(),
        "sql failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn assert_no_ss_outside(port: u16) {
    // Verify only this port is listening for our test range is hard globally,
    // but we can assert the process on this port binds 127.0.0.1.
    let out = Command::new("ss").args(["-ltnp"]).output().expect("ss");
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains(&format!(":{port}")) {
            assert!(
                line.contains("127.0.0.1"),
                "listener not on loopback: {line}"
            );
            assert!(!line.contains("0.0.0.0"), "listener on 0.0.0.0: {line}");
        }
    }
}

fn ensure_libcrypt_shim(base: &Path) -> Option<PathBuf> {
    // Fedora 44 ships libcrypt.so.2 but MariaDB 11.8/11.4 tarballs link libcrypt.so.1.
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
    let target = Path::new("/usr/lib64/libcrypt.so.2");
    if target.exists() {
        let link = shim.join("libcrypt.so.1");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(target, &link).unwrap();
        return Some(shim);
    }
    None
}

#[test]
#[ignore = "e2e: requires cached MariaDB tarballs; run with DBLOCAL_E2E=1 under flock $HOME/.cache/dblocal-e2e.lock"]
fn mariadb_11_8_full_cycle_and_two_instances() {
    require_e2e();
    let cache = cache_dir();
    let a118 = cache.join(MDB118_ARCHIVE);
    let a114 = cache.join(MDB114_ARCHIVE);
    assert!(a118.is_file(), "missing {}", a118.display());

    let tmp = tempfile::tempdir().expect("tmp");
    if let Some(shim) = ensure_libcrypt_shim(tmp.path()) {
        let prev = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let val = if prev.is_empty() {
            shim.display().to_string()
        } else {
            format!("{}:{prev}", shim.display())
        };
        // Safety: e2e test process sets env before spawning engine children.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("LD_LIBRARY_PATH", val)
        };
    }

    extract_tar_gz(&a118, tmp.path());
    let rt118 = runtime_dir(tmp.path(), "11.8.10");

    let port1 = 43100u16;
    let port2 = 43101u16;
    require_port_free(port1);
    require_port_free(port2);

    let (_i1, d1, l1, r1) = mk_instance(tmp.path(), "m118a");
    let (_i2, d2, l2, r2) = mk_instance(tmp.path(), "m118b");
    let pw = "e2e-mdb-secret-123";

    // init
    MariaDbAdapter
        .initialize(&init_ctx(&rt118, &d1, port1, AuthMode::Password, Some(pw)))
        .expect("init 1");
    MariaDbAdapter
        .initialize(&init_ctx(&rt118, &d2, port2, AuthMode::Password, Some(pw)))
        .expect("init 2");

    // start 1
    let mut g1 = start_mariadb(&rt118, &d1, &l1, &r1, port1, pw);
    assert!(
        wait_tcp(port1, Duration::from_secs(30)),
        "mdb1 did not listen; log:\n{}",
        fs::read_to_string(l1.join("mariadb.err")).unwrap_or_default()
    );
    let pid1 = read_pid(&d1.join("mariadbd.pid"));
    assert!(Path::new(&format!("/proc/{pid1}")).exists());

    let h = MariaDbAdapter
        .health_check(&run_ctx(
            &rt118,
            &d1,
            &l1,
            &r1,
            port1,
            AuthMode::Password,
            Some(pw),
        ))
        .expect("health");
    assert!(matches!(h, dblocal_core::engine::Health::Ready));

    // login with password
    let ver = sql_query(&rt118, port1, "root", pw, "SELECT VERSION();");
    assert!(ver.contains("11.8.10-MariaDB"), "version: {ver}");
    eprintln!("mariadb version: {ver}");

    // wrong password rejected
    let client = mariadb_client(&rt118);
    let bad = Command::new(&client)
        .args([
            "-h",
            "127.0.0.1",
            "-P",
            "43100",
            "-u",
            "root",
            "--protocol=tcp",
            "-N",
            "-B",
            "-e",
            "SELECT 1;",
        ])
        .env("MYSQL_PWD", "wrong")
        .output()
        .expect("bad pw");
    assert!(!bad.status.success(), "wrong password should fail");

    // create data
    sql_query(
        &rt118,
        port1,
        "root",
        pw,
        "CREATE DATABASE e2e; CREATE TABLE e2e.t (id INT); INSERT INTO e2e.t VALUES (42);",
    );

    // start second instance (same major, different port)
    let mut g2 = start_mariadb(&rt118, &d2, &l2, &r2, port2, pw);
    assert!(
        wait_tcp(port2, Duration::from_secs(30)),
        "mdb2 did not listen; log:\n{}",
        fs::read_to_string(l2.join("mariadb.err")).unwrap_or_default()
    );
    let _pid2 = read_pid(&d2.join("mariadbd.pid"));
    let ver2 = sql_query(&rt118, port2, "root", pw, "SELECT VERSION();");
    assert!(ver2.contains("11.8.10-MariaDB"));

    assert_no_ss_outside(port1);
    assert_no_ss_outside(port2);

    // stop 2
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
    let _ = g2.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port2, Duration::from_secs(30)));

    // stop 1
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
    let _ = g1.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port1, Duration::from_secs(30)));
    assert!(wait_pid_gone(pid1, Duration::from_secs(30)));

    // restart 1, data survives
    let mut g1b = start_mariadb(&rt118, &d1, &l1, &r1, port1, pw);
    assert!(wait_tcp(port1, Duration::from_secs(30)), "restart listen");
    let got = sql_query(&rt118, port1, "root", pw, "SELECT id FROM e2e.t;");
    assert_eq!(got, "42");
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
        .expect("shutdown 1b");
    let _ = g1b.0.take().map(|mut c| c.wait());
    assert!(wait_tcp_gone(port1, Duration::from_secs(30)));

    // side-by-side with 11.4.14 if cached
    if a114.is_file() {
        extract_tar_gz(&a114, tmp.path());
        let rt114 = runtime_dir(tmp.path(), "11.4.14");
        let port3 = 43102u16;
        require_port_free(port3);
        let (_i3, d3, l3, r3) = mk_instance(tmp.path(), "m114");
        MariaDbAdapter
            .initialize(&init_ctx(&rt114, &d3, port3, AuthMode::Password, Some(pw)))
            .expect("init 114");
        let mut g3 = start_mariadb(&rt114, &d3, &l3, &r3, port3, pw);
        assert!(wait_tcp(port3, Duration::from_secs(30)), "114 listen");
        let v3 = sql_query(&rt114, port3, "root", pw, "SELECT VERSION();");
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
        eprintln!("mariadb 11.4 side-by-side OK: {v3}");
    } else {
        eprintln!("skip 11.4 side-by-side: archive not cached");
    }

    eprintln!("mariadb e2e OK");
}
