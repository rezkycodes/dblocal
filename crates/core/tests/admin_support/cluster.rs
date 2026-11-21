//! A real PostgreSQL cluster for the admin e2e: extract the runtime, `initdb`,
//! start and stop it, and identify/kill its processes from `/proc`.
//!
//! The engine is the real binary from the catalog archive cached in
//! `~/.cache/dblocal-dev`; `initdb`/`pg_ctl` are the exact commands the production
//! adapter builds, reproduced here so the test exercises the same engine layout
//! (`runtimes/postgresql/<version>/<arch>/bin`) without touching `tests/support`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dblocal_core::platform::Arch;
use dblocal_core::secret::SecretString;

use super::temp_home::TempHome;

/// PostgreSQL version this task proves against: the newest in the catalog.
pub const PG_VERSION: &str = "18.7.0";
/// Catalog archive name for [`PG_VERSION`] on Linux x86_64.
pub const PG_ARCHIVE: &str = "postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz";
/// Superuser the cluster is initialised with.
pub const SUPERUSER: &str = "postgres";

/// A running PostgreSQL cluster under a temp home.
pub struct AdminCluster {
    data_dir: PathBuf,
    logs_dir: PathBuf,
    runtime_bin: PathBuf,
    port: u16,
    started: bool,
}

impl AdminCluster {
    /// Extracts the runtime from the cached archive, `initdb`s a fresh data dir
    /// and starts the server on `port`. The caller owns the [`TempHome`].
    pub fn start(home: &TempHome, port: u16, password: &SecretString) -> Self {
        let runtime_bin = extract_runtime(home, PG_VERSION, PG_ARCHIVE);
        let base = home.path().join("admin-cluster");
        let data_dir = base.join("data");
        let logs_dir = base.join("logs");
        fs::create_dir_all(&logs_dir).expect("logs dir");
        initdb(&runtime_bin, &data_dir, port, password);

        let cluster = Self {
            data_dir,
            logs_dir,
            runtime_bin,
            port,
            started: true,
        };
        cluster.start_server();
        cluster
    }

    /// TCP port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    fn start_server(&self) {
        let status = Command::new(self.runtime_bin.join("pg_ctl"))
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-l")
            .arg(self.logs_dir.join("postgres.log"))
            .arg("-w")
            .arg("-t")
            .arg("60")
            .arg("start")
            .stdin(Stdio::null())
            .status()
            .expect("run pg_ctl start");
        assert!(status.success(), "pg_ctl start failed");
        assert!(
            wait_tcp(self.port, Duration::from_secs(30)),
            "port {} never accepted a connection",
            self.port
        );
    }

    /// Stops the server with `pg_ctl -m fast`.
    pub fn stop(&mut self) {
        if !self.started {
            return;
        }
        let status = Command::new(self.runtime_bin.join("pg_ctl"))
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-m")
            .arg("fast")
            .arg("-w")
            .arg("-t")
            .arg("30")
            .arg("stop")
            .stdin(Stdio::null())
            .status()
            .expect("run pg_ctl stop");
        assert!(status.success(), "pg_ctl stop failed");
        self.started = false;
    }
}

impl Drop for AdminCluster {
    fn drop(&mut self) {
        // Best-effort stop; the TempHome guard kills anything left behind.
        let _ = Command::new(self.runtime_bin.join("pg_ctl"))
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-m")
            .arg("immediate")
            .arg("stop")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Extracts `archive` (from the developer's `~/.cache/dblocal-dev`) into the
/// temp home's runtime directory, returning its `bin` folder.
fn extract_runtime(home: &TempHome, version: &str, archive: &str) -> PathBuf {
    let target = home.runtime_dir(version);
    let bin = target.join("bin");
    if bin.is_dir() {
        return bin;
    }
    let real_home = super::temp_home::real_home();
    // HOME is redirected to the temp home; the archive lives in the real one.
    let cache = std::env::var_os("DBLOCAL_E2E_ARCHIVE_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home.join(".cache/dblocal-dev"));
    let path = cache.join(archive);
    assert!(
        path.is_file(),
        "the PostgreSQL archive must be cached at {}",
        path.display()
    );
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("runtime parent");
    }
    let staging = target.with_file_name(format!(".{version}.staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).expect("staging dir");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&path)
        .arg("-C")
        .arg(&staging)
        .arg("--strip-components=1")
        .status()
        .expect("run tar");
    assert!(status.success(), "tar could not extract {}", path.display());
    fs::rename(&staging, &target).expect("install the runtime");
    assert!(bin.is_dir(), "runtime bin missing at {}", bin.display());
    bin
}

/// Runs `initdb` with SCRAM auth and a password file, matching the adapter.
fn initdb(runtime_bin: &Path, data_dir: &Path, port: u16, password: &SecretString) {
    if let Some(parent) = data_dir.parent() {
        fs::create_dir_all(parent).expect("data parent");
    }
    let pwfile = data_dir.with_file_name("pwfile");
    fs::write(&pwfile, password.expose()).expect("write pwfile");
    let locale = pick_locale();
    let status = Command::new(runtime_bin.join("initdb"))
        .arg("-D")
        .arg(data_dir)
        .arg("-U")
        .arg(SUPERUSER)
        .arg("--encoding=UTF8")
        .arg(format!("--locale={locale}"))
        .arg("--auth-local=scram-sha-256")
        .arg("--auth-host=scram-sha-256")
        .arg("--pwfile")
        .arg(&pwfile)
        .stdin(Stdio::null())
        .status()
        .expect("run initdb");
    let _ = fs::remove_file(&pwfile);
    assert!(status.success(), "initdb failed");

    // The managed block the adapter writes after initdb.
    let conf = data_dir.join("postgresql.conf");
    let mut contents = fs::read_to_string(&conf).expect("read conf");
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(&format!(
        "\nlisten_addresses = '127.0.0.1'\nport = {port}\nunix_socket_directories = ''\n"
    ));
    fs::write(&conf, contents).expect("write conf");
    assert_eq!(Arch::current().as_str(), "x86_64", "the archive is x86_64");
}

/// A UTF-8 locale present on this host, preferring `C.UTF-8`.
fn pick_locale() -> &'static str {
    let out = Command::new("locale").arg("-a").output();
    if let Ok(out) = out
        && out.status.success()
    {
        let list = String::from_utf8_lossy(&out.stdout);
        if list.lines().any(|l| l.trim() == "C.UTF-8") {
            return "C.UTF-8";
        }
        if list.lines().any(|l| l.trim() == "C.utf8") {
            return "C.utf8";
        }
    }
    "C"
}

/// Waits until `port` accepts a TCP connection.
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
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

// ---------------------------------------------------------------------------
// /proc scanning for the cleanup guard.
// ---------------------------------------------------------------------------

/// One live process read from `/proc`.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    /// Process id.
    pub pid: u32,
    /// Command line joined by spaces.
    pub cmdline: String,
    /// Resolved executable, when readable.
    pub exe: Option<PathBuf>,
}

fn procs_under(home: &Path) -> Vec<ProcInfo> {
    let needle = home.to_string_lossy().into_owned();
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let cmdline = bytes
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        if !cmdline.contains(&needle) {
            continue;
        }
        found.push(ProcInfo {
            pid,
            cmdline,
            exe: fs::read_link(format!("/proc/{pid}/exe")).ok(),
        });
    }
    found
}

/// Kills every `postgres` process whose executable sits inside `home`'s
/// `runtimes/` tree — positive identification, never a blind signal.
pub fn kill_postgres_under(home: &Path) {
    let runtimes = home.join("runtimes");
    for info in procs_under(home) {
        let is_pg = info
            .exe
            .as_ref()
            .and_then(|exe| exe.file_name())
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "postgres");
        match &info.exe {
            Some(exe) if is_pg && exe.starts_with(&runtimes) => {
                let _ = Command::new("kill")
                    .args(["-KILL", &info.pid.to_string()])
                    .status();
            }
            other => eprintln!(
                "admin cleanup: not signalling pid {} — executable {:?} is not a postgres under {} (argv: {})",
                info.pid,
                other,
                runtimes.display(),
                info.cmdline
            ),
        }
    }
}

/// Waits until no positively-identified `postgres` process mentions `home`.
pub fn wait_none_under(home: &Path, timeout: Duration) -> bool {
    let runtimes = home.join("runtimes");
    let live = || {
        procs_under(home)
            .into_iter()
            .filter(|info| {
                info.exe
                    .as_ref()
                    .is_some_and(|exe| exe.starts_with(&runtimes))
            })
            .collect::<Vec<_>>()
    };
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = live();
        if remaining.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            for info in remaining {
                let _ = Command::new("kill")
                    .args(["-KILL", &info.pid.to_string()])
                    .status();
            }
            return live().is_empty();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
