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

pub(super) const MYSQL_ARCHIVE: &str = "mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz";
pub(super) const MYSQL_DIR: &str = "mysql-8.4.11-linux-glibc2.28-x86_64-minimal";

pub(super) struct KillOnDrop(pub(super) Option<Child>, pub(super) &'static str);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("killed {}", self.1);
        }
    }
}

pub(super) fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cache/dblocal-dev")
}

pub(super) fn require_e2e() {
    if std::env::var("DBLOCAL_E2E_MYSQL").ok().as_deref() != Some("1") {
        panic!("set DBLOCAL_E2E_MYSQL=1 to run real MySQL e2e tests");
    }
}

pub(super) fn extract_tar_xz(archive: &Path, dest: &Path) {
    let status = Command::new("tar")
        .args(["-xJf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .expect("tar must be available");
    assert!(status.success(), "failed to extract {}", archive.display());
}

pub(super) fn wait_tcp(port: u16, timeout: Duration) -> bool {
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

pub(super) fn wait_tcp_gone(port: u16, timeout: Duration) -> bool {
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

pub(super) fn require_port_free(port: u16) {
    if TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(150),
    )
    .is_ok()
    {
        panic!("port {port} already in use before test");
    }
}

pub(super) fn read_log_tail(path: &Path, max_lines: usize) -> String {
    let mut s = String::new();
    if let Ok(mut f) = fs::File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub(super) fn mysql_runtime(extracted: &Path) -> PathBuf {
    extracted.join(MYSQL_DIR)
}

pub(super) fn mk_instance(base: &Path, name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let inst = base.join(name);
    let data = inst.join("data");
    let logs = inst.join("logs");
    let run = inst.join("run");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&run).unwrap();
    fs::create_dir_all(data.parent().unwrap()).unwrap();
    (inst, data, logs, run)
}

pub(super) fn init_ctx(
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

pub(super) fn run_ctx(
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

pub(super) fn assert_no_password_in_args(cmd: &Command, secret: &str) {
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

pub(super) fn mysql_start(
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

pub(super) fn mysql_client(
    runtime: &Path,
    port: u16,
    password: &str,
    sql: &str,
) -> std::process::Output {
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

pub(super) fn assert_only_instance_loopback_port(pid: u32, expected_port: u16) {
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
