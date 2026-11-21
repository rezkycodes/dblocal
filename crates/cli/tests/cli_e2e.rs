//! W3-D real-binary e2e: the shipped `dblocal` binary driving a real Valkey.
//!
//! Run it with:
//!
//! ```sh
//! export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target-gui"
//! DBLOCAL_E2E=1 cargo xtask e2e
//! ```
//!
//! or alone:
//!
//! ```sh
//! DBLOCAL_E2E=1 cargo test -p dblocal-cli --test cli_e2e -- --ignored --nocapture
//! ```
//!
//! What is real: the binary, `LocalService`, the catalog, the HTTPS downloader,
//! `valkey-server`, and the `redis` client that consumes `dblocal uri`.
//!
//! Substitutions, all documented in `docs/architecture/cli.md`:
//! - `HOME` and `DBLOCAL_HOME` point into a temp directory;
//! - `DBUS_SESSION_BUS_ADDRESS` points at a nonexistent bus, so the keychain is
//!   unavailable and the production `0600` file fallback is used (F-CRD-01 AC2);
//! - the engine runtime may be copied from the process-external cache the core
//!   e2e seeded (`F-RTM-03` AC3), so repeated runs do not re-download 46 MB.
//!
//! Ports come from 20000-29999 only. The guard scans `/proc` on drop (panic
//! included) and kills only processes whose command line names this temp home
//! *and* whose executable resolves inside its `runtimes/` tree.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(unsafe_code)]

use std::io::Write as _;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use redis::Commands;
use tempfile::TempDir;

const PORT_FIRST: u16 = 20_000;
const PORT_LAST: u16 = 29_999;

/// Prefix every e2e temp home uses (`cargo xtask e2e` scans for it).
const HOME_PREFIX: &str = "dblocal-e2e-cli-";

/// Line the follow test appends to the engine log.
const APPENDED_LINE: &str = "cli-e2e: appended while following";

struct Home {
    dir: TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_dblocal: Option<std::ffi::OsString>,
    previous_bus: Option<std::ffi::OsString>,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix(HOME_PREFIX)
            .tempdir()
            .expect("temp home");
        let previous_home = std::env::var_os("HOME");
        let previous_dblocal = std::env::var_os("DBLOCAL_HOME");
        let previous_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        // SAFETY: one ignored test per process (`--test-threads=1`), and the
        // guard restores these on drop.
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("DBLOCAL_HOME", dir.path());
            // No session bus: the keychain reports Unsupported and the
            // production 0600 file fallback is used.
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-bus");
        }
        Self {
            dir,
            previous_home,
            previous_dblocal,
            previous_bus,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        kill_engines_under(self.dir.path());
        wait_none_under(self.dir.path(), Duration::from_secs(10));
        // SAFETY: same single-threaded env reasoning as `new`.
        unsafe {
            match &self.previous_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.previous_dblocal {
                Some(v) => std::env::set_var("DBLOCAL_HOME", v),
                None => std::env::remove_var("DBLOCAL_HOME"),
            }
            match &self.previous_bus {
                Some(v) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v),
                None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
            }
        }
    }
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dblocal"))
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().expect("run dblocal")
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed ({:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn require_e2e() {
    if std::env::var("DBLOCAL_E2E").ok().as_deref() != Some("1") {
        panic!("cli_e2e needs DBLOCAL_E2E=1; run `DBLOCAL_E2E=1 cargo xtask e2e`");
    }
}

fn reserve_port() -> u16 {
    for _ in 0..256 {
        let raw = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u16)
            .unwrap_or(0);
        let span = PORT_LAST - PORT_FIRST + 1;
        let port = PORT_FIRST + (raw % span);
        if TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in {PORT_FIRST}-{PORT_LAST}");
}

fn wait_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn wait_tcp_gone(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(200),
        )
        .is_err()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn cmdline(pid: u32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        bytes
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn kill_engines_under(home: &Path) {
    let runtimes = home.join("runtimes");
    let needle = home.to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(line) = cmdline(pid) else { continue };
        if !line.contains(&needle) {
            continue;
        }
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
        match exe {
            Some(path) if path.starts_with(&runtimes) => {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
            _ => eprintln!(
                "cli_e2e cleanup: not signalling pid {pid}: executable is not under {}",
                runtimes.display()
            ),
        }
    }
}

fn wait_none_under(home: &Path, timeout: Duration) {
    let runtimes = home.join("runtimes");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let mut alive = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<u32>().ok())
                else {
                    continue;
                };
                let Some(line) = cmdline(pid) else { continue };
                if line.contains(&home.to_string_lossy().into_owned())
                    && std::fs::read_link(format!("/proc/{pid}/exe"))
                        .map(|p| p.starts_with(&runtimes))
                        .unwrap_or(false)
                {
                    alive.push(pid);
                }
            }
        }
        if alive.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Copies the Valkey runtime from the shared cache, like the core e2e does, so
/// repeated runs skip a 46 MB download (`F-RTM-03` AC3).
fn seed_cached_runtime(home: &Path, version: &str) -> bool {
    let cache = std::env::var_os("DBLOCAL_E2E_RUNTIME_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("dblocal-e2e-runtime-cache"));
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let source = cache.join("runtimes/valkey").join(version).join(arch);
    if !source.join("bin").is_dir() {
        return false;
    }
    let target = home.join("runtimes/valkey").join(version).join(arch);
    if target.join("bin").is_dir() {
        return true;
    }
    copy_tree(&source, &target).is_ok()
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, meta.permissions())?;
    } else if meta.file_type().is_symlink() {
        let target = std::fs::read_link(from)?;
        let _ = std::fs::remove_file(to);
        std::os::unix::fs::symlink(target, to)?;
    } else {
        std::fs::copy(from, to)?;
        std::fs::set_permissions(to, meta.permissions())?;
    }
    Ok(())
}

#[test]
#[ignore = "e2e: network + a real Valkey engine; run with DBLOCAL_E2E=1 cargo xtask e2e"]
fn cli_drives_a_real_valkey_from_create_to_archive() {
    require_e2e();
    let home = Home::new();
    let started = Instant::now();

    // The URI alone, and the version/help paths, need no engine.
    let version = ok(&run(&["--version"]), "--version");
    eprintln!("cli_e2e: {version}");
    ok(&run(&["--help"]), "--help");
    let bad = run(&["frobnicate"]);
    assert_eq!(bad.status.code(), Some(2));
    let empty = ok(&run(&["list", "--json"]), "list --json on an empty home");
    assert_eq!(empty.trim(), "[]");

    let port = reserve_port();
    let name = "cli-e2e-vk";
    // Seed the cached runtime when the core e2e already downloaded it.
    let seeded = seed_cached_runtime(home.path(), "9.2.0");
    eprintln!("cli_e2e: cached valkey runtime seeded: {seeded}");

    let create_started = Instant::now();
    let uri = ok(
        &run(&[
            "create",
            "--engine",
            "valkey",
            "--version",
            "9.2.0",
            "--name",
            name,
            "--port",
            &port.to_string(),
        ]),
        "create",
    );
    let uri = uri.trim().to_owned();
    eprintln!(
        "cli_e2e: create produced a URI in {:.1}s",
        create_started.elapsed().as_secs_f64()
    );
    assert!(uri.starts_with("redis://"), "unexpected URI: {uri}");

    // stop-then-start, since create leaves the instance stopped.
    let start_started = Instant::now();
    ok(&run(&["start", name, "--timeout", "60"]), "start");
    eprintln!(
        "cli_e2e: start returned in {:.1}s",
        start_started.elapsed().as_secs_f64()
    );
    let status = ok(&run(&["status", name]), "status after start");
    assert_eq!(status.trim(), "running");
    assert!(wait_tcp(port, Duration::from_secs(20)));

    // The URI is real: pipe it into the production redis client.
    let mut client = redis::Client::open(uri.as_str())
        .expect("parse the CLI URI")
        .get_connection_with_timeout(Duration::from_secs(5))
        .expect("connect with the CLI URI");
    assert_eq!(client.ping::<String>().expect("PING"), "PONG");
    client.set::<_, _, ()>("cli-e2e:key", "v1").expect("SET");
    assert_eq!(
        client.get::<_, Option<String>>("cli-e2e:key").expect("GET"),
        Some("v1".to_owned())
    );
    drop(client);

    // K1: `logs -f --lines 2` prints exactly the 2 requested lines and, with
    // nothing appended during the window, nothing else. Before the fix the
    // first tick reprinted the whole 200-line follow window.
    let history = follow_logs(name, 2, || {});
    eprintln!(
        "cli_e2e: logs -f --lines 2 printed {} lines after one follow tick: {history:?}",
        history.len()
    );
    assert_eq!(
        history.len(),
        2,
        "logs -f --lines 2 printed {} lines: {history:?}",
        history.len()
    );

    // K1: a line appended while following is printed exactly once.
    let log = home
        .path()
        .join("instances")
        .join(name)
        .join("logs")
        .join("valkey.log");
    let before = std::fs::read_to_string(&log)
        .expect("read valkey.log")
        .lines()
        .count();
    let followed = follow_logs(name, 5, || {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("open valkey.log for append");
        // The engine always ends its lines, but a missing newline would fuse
        // the append onto the last line and make this test prove nothing.
        if !std::fs::read_to_string(&log)
            .map(|text| text.ends_with('\n'))
            .unwrap_or(false)
        {
            writeln!(file).expect("newline before append");
        }
        writeln!(file, "{}", APPENDED_LINE).expect("append");
        file.flush().expect("flush");
    });
    // History (at most 5) plus the append. An engine that logs on its own could
    // add more, so the count is a lower bound and the append is the assertion.
    eprintln!(
        "cli_e2e: follow with one append printed {} lines (history {before} + 1): {followed:?}",
        followed.len()
    );
    assert!(
        followed.len() >= before.min(5) + 1,
        "the append was not printed: {followed:?}"
    );
    assert_eq!(
        followed
            .iter()
            .filter(|line| line.as_str() == APPENDED_LINE)
            .count(),
        1,
        "the appended line must be printed exactly once: {followed:?}"
    );

    // status is non-zero while stopped.
    ok(&run(&["stop", name]), "stop");
    let status = run(&["status", name]);
    assert!(
        !status.status.success(),
        "status must be non-zero when stopped"
    );
    assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "stopped");
    assert!(wait_tcp_gone(port, Duration::from_secs(20)));

    // The default delete archives and reports where.
    let delete = run(&["delete", name, "--yes"]);
    ok(&delete, "delete --yes");
    let delete_err = String::from_utf8_lossy(&delete.stderr).into_owned();
    assert!(
        delete_err.contains("Data diarsipkan ke"),
        "archive notice missing: {delete_err}"
    );
    let archives = home.path().join("archive");
    assert!(
        std::fs::read_dir(&archives)
            .map(|entries| entries.flatten().count())
            .unwrap_or(0)
            >= 1,
        "an archive directory must exist under {}",
        archives.display()
    );
    assert!(wait_tcp_gone(port, Duration::from_secs(20)));

    eprintln!("cli_e2e: whole run {:.1}s", started.elapsed().as_secs_f64());
}

/// Runs `dblocal logs <name> -f --lines <lines>`, lets `during` happen, then
/// SIGINTs after the follow has had a chance to tick, and returns stdout lines.
///
/// The child is owned by [`Follower`], which SIGKILLs it on drop, so a panicking
/// assertion cannot leave a `logs -f` process behind.
fn follow_logs(name: &str, lines: usize, during: impl FnOnce()) -> Vec<String> {
    let child = bin()
        .args(["logs", name, "-f", "--lines", &lines.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn logs -f");
    let mut follower = Follower { child: Some(child) };
    // Let the history print and at least one follow tick happen.
    std::thread::sleep(Duration::from_millis(600));
    during();
    // Two follow intervals (250 ms each) plus slack for the append to be read.
    std::thread::sleep(Duration::from_millis(1200));
    follower.interrupt();
    let out = follower.wait();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "logs -f must exit 0 on SIGINT, got {:?}; stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    stdout.lines().map(str::to_owned).collect()
}

/// Owns a `logs -f` child and guarantees it is gone when the test unwinds.
struct Follower {
    child: Option<Child>,
}

impl Follower {
    fn interrupt(&mut self) {
        if let Some(child) = &self.child {
            let _ = Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status();
        }
    }

    fn wait(&mut self) -> Output {
        let Some(child) = self.child.take() else {
            panic!("follower already reaped");
        };
        child.wait_with_output().expect("wait for logs -f")
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
