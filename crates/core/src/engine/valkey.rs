//! Valkey engine adapter.
//!
//! Renders `valkey.conf`, builds the `valkey-server` start command, probes with
//! `PING` over RESP, stops with `SHUTDOWN`, parses the pidfile and diagnoses
//! start failures — including causes that only appear in the logfile (C-10).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::consts;
use crate::engine::uri::{self, exe_name};
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::{DblError, Result};
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

/// Valkey listens on IPv4 and IPv6 loopback only (never `0.0.0.0`).
pub const BIND: &str = "127.0.0.1 -::1";
/// Dependency policy: AOF keeps data across a forced kill (C-11).
const PERSISTENCE: &str = "appendonly yes\nappendfilename \"appendonly.aof\"\nappenddirname \"appendonlydir\"\nsave 3600 1 300 100 60 10000";
/// Mode passed to `SHUTDOWN`; AOF already carries committed data.
pub const SHUTDOWN_MODE: &str = "NOSAVE";

/// Valkey adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValkeyAdapter;

impl EngineAdapter for ValkeyAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Valkey
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        let run_dir = sibling_dir(&ctx.data_dir, consts::DIR_RUN);
        let logs_dir = sibling_dir(&ctx.data_dir, consts::DIR_LOGS);
        for (dir, label) in [
            (&ctx.data_dir, "data"),
            (&run_dir, "run"),
            (&logs_dir, "logs"),
        ] {
            ensure_private_dir(dir, label)?;
        }
        let params = ConfigParams {
            port: ctx.port.get(),
            data_dir: &ctx.data_dir,
            run_dir: &run_dir,
            logs_dir: &logs_dir,
            password: ctx.password.as_ref(),
            auth: ctx.auth,
        };
        let rendered = render_config(&params);
        write_config(&config_path(&run_dir), rendered.expose())?;
        tracing::debug!(instance = %ctx.instance_name, "valkey.conf written");
        Ok(())
    }

    fn spawn_command(&self, ctx: &RunContext) -> Command {
        let server = ctx.runtime_dir.join("bin").join(exe_name("valkey-server"));
        let mut cmd = Command::new(server);
        cmd.arg(config_path(&ctx.run_dir));
        cmd.stdin(Stdio::null());
        cmd
    }

    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        // Parse the RESP reply. `valkey-cli` exits 0 on NOAUTH (C-12), so a CLI
        // exit code proves nothing.
        let client = client_for(ctx)?;
        let mut con = client
            .get_connection_with_timeout(Duration::from_secs(5))
            .map_err(|e| classify_redis_connect(&e))?;
        match redis::cmd("PING").query::<String>(&mut con) {
            Ok(reply) if reply == "PONG" => Ok(Health::Ready),
            Ok(reply) => Ok(Health::NotReady(format!("unexpected PING reply: {reply}"))),
            Err(err) => {
                if is_auth_challenge(&err) {
                    // The server answered the protocol with an auth requirement:
                    // it is up (trait contract).
                    Ok(Health::Ready)
                } else {
                    Err(DblError::Engine {
                        reason: format!("valkey PING failed: {err}"),
                    })
                }
            }
        }
    }

    fn shutdown(&self, ctx: &RunContext) -> Result<()> {
        let client = client_for(ctx)?;
        let mut con = client
            .get_connection_with_timeout(Duration::from_secs(consts::STOP_GRACE_SECS))
            .map_err(|e| classify_redis_connect(&e))?;
        let reply: redis::RedisResult<()> =
            redis::cmd("SHUTDOWN").arg(SHUTDOWN_MODE).query(&mut con);
        match reply {
            Ok(()) => Ok(()),
            // `SHUTDOWN` closes the socket before acknowledging; a dropped
            // connection after we sent the command is the success path.
            Err(err) if is_connection_closed(&err) => Ok(()),
            Err(err) => Err(DblError::Engine {
                reason: format!("valkey SHUTDOWN failed: {err}"),
            }),
        }
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString {
        uri::render(EngineKey::Valkey, ctx.port.get(), credential, format)
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        diagnose_failure_text(output)
    }

    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        // Phase 2.
        None
    }
}

/// Inputs of a rendered `valkey.conf`.
#[derive(Debug, Clone, Copy)]
pub struct ConfigParams<'a> {
    /// Listening port.
    pub port: u16,
    /// Server working directory (`dir`); must exist.
    pub data_dir: &'a Path,
    /// Directory holding the pidfile and config.
    pub run_dir: &'a Path,
    /// Directory holding the logfile.
    pub logs_dir: &'a Path,
    /// Superuser password, present only in password auth mode.
    pub password: Option<&'a SecretString>,
    /// Authentication mode; drives `requirepass`.
    pub auth: AuthMode,
}

/// Renders the managed `valkey.conf`. Secret because it may carry `requirepass`.
pub fn render_config(params: &ConfigParams<'_>) -> SecretString {
    let mut out = String::new();
    let _ = writeln!(out, "bind {BIND}");
    let _ = writeln!(out, "protected-mode yes");
    let _ = writeln!(out, "port {}", params.port);
    let _ = writeln!(out, "daemonize no");
    let _ = writeln!(out, "supervised no");
    let _ = writeln!(out, "tcp-backlog 511");
    let _ = writeln!(out, "timeout 0");
    let _ = writeln!(out, "tcp-keepalive 300");
    let _ = writeln!(out, "loglevel notice");
    let _ = writeln!(out, "pidfile {}", pidfile_path(params.run_dir).display());
    let _ = writeln!(out, "logfile {}", logfile_path(params.logs_dir).display());
    let _ = writeln!(out, "databases 16");
    let _ = writeln!(out, "dir {}", params.data_dir.display());
    if params.auth == AuthMode::Password
        && let Some(password) = params.password
    {
        // `requirepass` must be a single token; generated passwords are
        // alphanumeric, so they never need quoting.
        let _ = writeln!(out, "requirepass {}", password.expose());
    }
    let _ = writeln!(out, "{PERSISTENCE}");
    SecretString::new(out)
}

/// Path of the generated config inside the run directory.
pub fn config_path(run_dir: &Path) -> PathBuf {
    run_dir.join("valkey.conf")
}

/// Path of the server pidfile.
pub fn pidfile_path(run_dir: &Path) -> PathBuf {
    run_dir.join("valkey.pid")
}

/// Path of the server logfile.
pub fn logfile_path(logs_dir: &Path) -> PathBuf {
    logs_dir.join("valkey.log")
}

/// Ensures the directories the server needs exist at start time.
///
/// `spawn_command` cannot fail; the service can call this before spawning to get
/// a clear error when the instance layout is damaged.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    ensure_private_dir(&ctx.data_dir, "data")?;
    ensure_private_dir(&ctx.run_dir, "run")?;
    ensure_private_dir(&ctx.logs_dir, "logs")?;
    Ok(())
}

fn ensure_private_dir(dir: &Path, label: &str) -> Result<()> {
    crate::fsutil::ensure_private_dir(dir).map_err(|e| DblError::Engine {
        reason: format!(
            "cannot create/chmod {label} directory {}: {e}",
            dir.display()
        ),
    })
}

/// Rewrites the managed `port` line in `run_dir/valkey.conf`.
///
/// Atomic: temp file in the same directory, fsync, rename. Other settings and
/// comments are preserved. Idempotent. Fails if the config is missing or has no
/// `port` directive (the caller should treat that as a config-drift error).
pub fn sync_port(run_dir: &Path, port: u16) -> Result<()> {
    let path = config_path(run_dir);
    let contents = fs::read_to_string(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot read {}: {e}", path.display()),
    })?;
    let mut replaced = false;
    let mut out = String::with_capacity(contents.len() + 16);
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("port ") {
            let _ = writeln!(out, "port {port}");
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        return Err(DblError::Engine {
            reason: format!("{} has no `port` directive to rewrite", path.display()),
        });
    }
    write_private_config_atomic(&path, &out)
}

fn write_private_config_atomic(path: &Path, contents: &str) -> Result<()> {
    crate::fsutil::write_private_file_atomic(path, contents.as_bytes()).map_err(|e| {
        DblError::Engine {
            reason: format!("cannot write {}: {e}", path.display()),
        }
    })
}

/// Returns `data_dir`'s sibling directory `leaf` (`run`, `logs`).
pub fn sibling_dir(data_dir: &Path, leaf: &str) -> PathBuf {
    match data_dir.parent() {
        Some(parent) => parent.join(leaf),
        None => data_dir.join(leaf),
    }
}

fn write_config(path: &Path, contents: &str) -> Result<()> {
    write_private_config_atomic(path, contents)
}

/// Parsed Valkey pidfile: the server writes a single PID line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerIdentity {
    /// Server process id.
    pub pid: u32,
}

/// Parses a Valkey pidfile. Callers must still verify the executable and the
/// `/proc/<pid>` start time before signalling (C-08 discipline, D-17).
pub fn parse_pidfile(contents: &str) -> Result<ServerIdentity> {
    let first = contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| DblError::Engine {
            reason: "valkey.pid is empty".to_owned(),
        })?;
    let pid = first.parse::<u32>().map_err(|_| DblError::Engine {
        reason: "valkey.pid does not contain a pid".to_owned(),
    })?;
    Ok(ServerIdentity { pid })
}

/// Reads and parses `<run_dir>/valkey.pid`.
pub fn read_pidfile(run_dir: &Path) -> Result<ServerIdentity> {
    let path = pidfile_path(run_dir);
    let contents = fs::read_to_string(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot read {}: {e}", path.display()),
    })?;
    parse_pidfile(&contents)
}

fn client_for(ctx: &RunContext) -> Result<redis::Client> {
    let username = if ctx.superuser.is_empty() {
        consts::SUPERUSER_VALKEY.to_owned()
    } else {
        ctx.superuser.clone()
    };
    let password = ctx
        .credential
        .as_ref()
        .map(|c| c.password.clone())
        .unwrap_or_else(|| SecretString::new(String::new()));
    let credential = Credential::new(username, password);
    let uri = uri::render(
        EngineKey::Valkey,
        ctx.port.get(),
        &credential,
        UriFormat::Uri,
    );
    redis::Client::open(uri.expose()).map_err(|e| DblError::Engine {
        reason: format!("cannot build valkey client: {e}"),
    })
}

/// A server that answered with an auth requirement is up; this is not a
/// failure (trait contract).
fn is_auth_challenge(err: &redis::RedisError) -> bool {
    if err.kind() == redis::ErrorKind::AuthenticationFailed {
        return true;
    }
    err.detail()
        .is_some_and(|detail| detail.contains("NOAUTH") || detail.contains("AUTH"))
}

fn is_connection_closed(err: &redis::RedisError) -> bool {
    if err.is_io_error() {
        return true;
    }
    err.detail().is_some_and(|detail| {
        detail.contains("broken pipe")
            || detail.contains("Connection reset")
            || detail.contains("connection closed")
    })
}

fn classify_redis_connect(err: &redis::RedisError) -> DblError {
    if is_auth_challenge(err) {
        // Reached the server and it challenged us: it is running. The health
        // probe is the only caller that has a meaningful "Ready" answer here;
        // for stop/URI callers this is a real error.
        DblError::Engine {
            reason: "valkey rejected the credentials".to_owned(),
        }
    } else {
        DblError::Engine {
            reason: format!("cannot connect to valkey: {err}"),
        }
    }
}

/// Classifies raw Valkey start output or log tail into a [`StartFailure`].
///
/// Valkey writes some fatal causes only to the logfile (C-10), so the caller
/// must pass the log tail here, not only stderr.
pub fn diagnose_failure_text(output: &str) -> StartFailure {
    let lower = output.to_ascii_lowercase();

    // Port genuinely occupied (spike 6.1). A generic bind failure such as
    // "Cannot assign requested address" or a bare "Failed listening" has no
    // honest variant, so it falls through to Unknown with the raw tail.
    if output.contains("Address already in use") {
        return StartFailure::PortInUse;
    }

    // Only a real "permission denied" (including the append-only directory
    // case, spike 6.7) is an honest PermissionDenied. A missing directory
    // ("No such file or directory") or an append-only path that is "Not a
    // directory" has no honest variant, so it stays Unknown with the raw tail.
    if output.contains("Permission denied") || lower.contains("permission denied") {
        return StartFailure::PermissionDenied;
    }

    if output.contains("Can't open or create append-only dir") {
        return StartFailure::Unknown;
    }

    StartFailure::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Port;
    use tempfile::tempdir;

    fn params<'a>(password: Option<&'a SecretString>, auth: AuthMode) -> ConfigParams<'a> {
        ConfigParams {
            port: 41300,
            data_dir: Path::new("/home/u/.dblocal/instances/app/data"),
            run_dir: Path::new("/home/u/.dblocal/instances/app/run"),
            logs_dir: Path::new("/home/u/.dblocal/instances/app/logs"),
            password,
            auth,
        }
    }

    #[test]
    fn config_with_password_has_requirepass_and_loopback_bind() {
        let pw = SecretString::from("vk-pass-123");
        let p = params(Some(&pw), AuthMode::Password);
        let conf = render_config(&p).expose().to_owned();
        // The config file spells joined paths with the host separator, so the
        // expectations are built with `Path::join` too (`\` on Windows).
        let dir = p.data_dir.to_string_lossy().into_owned();
        let pidfile = pidfile_path(p.run_dir).to_string_lossy().into_owned();
        let logfile = logfile_path(p.logs_dir).to_string_lossy().into_owned();
        assert!(conf.contains("bind 127.0.0.1 -::1\n"));
        assert!(conf.contains("protected-mode yes\n"));
        assert!(conf.contains("port 41300\n"));
        assert!(conf.contains("daemonize no\n"));
        assert!(conf.contains("supervised no\n"));
        assert!(conf.contains(&format!("dir {dir}\n")));
        assert!(conf.contains(&format!("pidfile {pidfile}\n")));
        assert!(conf.contains(&format!("logfile {logfile}\n")));
        assert!(conf.contains("requirepass vk-pass-123\n"));
        assert!(conf.contains("appendonly yes\n"));
        assert!(!conf.contains("0.0.0.0"));
    }

    #[test]
    fn config_without_password_omits_requirepass() {
        let conf = render_config(&params(None, AuthMode::None))
            .expose()
            .to_owned();
        assert!(!conf.contains("requirepass"));
        // protected-mode stays on as the last line of defence (F-CRD-03 AC3).
        assert!(conf.contains("protected-mode yes"));
    }

    #[test]
    fn config_debug_and_display_are_redacted() {
        let pw = SecretString::from("super-secret-pw");
        let conf = render_config(&params(Some(&pw), AuthMode::Password));
        assert!(!format!("{conf}").contains("super-secret-pw"));
        assert!(!format!("{conf:?}").contains("super-secret-pw"));
    }

    #[test]
    fn pidfile_paths_are_under_run_and_logs() {
        assert_eq!(
            pidfile_path(Path::new("/i/run")),
            PathBuf::from("/i/run/valkey.pid")
        );
        assert_eq!(
            config_path(Path::new("/i/run")),
            PathBuf::from("/i/run/valkey.conf")
        );
        assert_eq!(
            logfile_path(Path::new("/i/logs")),
            PathBuf::from("/i/logs/valkey.log")
        );
    }

    #[test]
    fn sibling_dirs_are_under_the_instance() {
        assert_eq!(
            sibling_dir(Path::new("/i/data"), consts::DIR_RUN),
            PathBuf::from("/i/run")
        );
        assert_eq!(
            sibling_dir(Path::new("/i/data"), consts::DIR_LOGS),
            PathBuf::from("/i/logs")
        );
    }

    #[test]
    fn spawn_command_passes_only_the_config_path() {
        let ctx = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/valkey"),
            data_dir: PathBuf::from("/data"),
            logs_dir: PathBuf::from("/logs"),
            run_dir: PathBuf::from("/run"),
            port: Port::new(41300).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new("default", SecretString::from("top-secret"))),
        };
        let cmd = ValkeyAdapter.spawn_command(&ctx);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![config_path(Path::new("/run")).display().to_string()]
        );
        assert!(
            cmd.get_program()
                .to_string_lossy()
                .contains("valkey-server")
        );
        for arg in &args {
            assert!(!arg.contains("top-secret"));
        }
        for (k, v) in cmd.get_envs() {
            assert!(!k.to_string_lossy().contains("top-secret"));
            if let Some(v) = v {
                assert!(!v.to_string_lossy().contains("top-secret"));
            }
        }
    }

    #[test]
    fn initialize_creates_dirs_and_restricted_config() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path().join("instances/app/data");
        let ctx = InitContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/valkey"),
            data_dir: data.clone(),
            port: Port::new(41300).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::Password,
            password: Some(SecretString::from("pw-123")),
        };
        ValkeyAdapter.initialize(&ctx).expect("init");
        assert!(data.is_dir());
        let logs = sibling_dir(&data, consts::DIR_LOGS);
        let run = sibling_dir(&data, consts::DIR_RUN);
        assert!(logs.is_dir());
        assert!(run.is_dir());
        let conf = fs::read_to_string(config_path(&run)).expect("config");
        assert!(conf.contains("requirepass pw-123"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(config_path(&run))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn pidfile_parsing() {
        assert_eq!(parse_pidfile("4242\n").expect("pid").pid, 4242);
        assert_eq!(parse_pidfile("  7  \n").expect("pid").pid, 7);
        assert!(parse_pidfile("").is_err());
        assert!(parse_pidfile("not-a-pid\n").is_err());
    }

    #[test]
    fn read_pidfile_from_disk() {
        let dir = tempdir().expect("tempdir");
        fs::write(pidfile_path(dir.path()), "999\n").expect("write");
        assert_eq!(read_pidfile(dir.path()).expect("read").pid, 999);
    }

    // ---- Table-driven diagnosis: every verbatim Valkey failure text ----

    fn cases() -> Vec<(&'static str, StartFailure)> {
        vec![
            // 6.1 port already in use
            (
                "<M>:M 20 Nov 2026 20:17:08.351 # Warning: Could not create server TCP listening socket 127.0.0.1:41501: bind: Address already in use\n<M>:M 20 Nov 2026 20:17:08.351 # Failed listening on port 41501 (tcp), aborting.\n",
                StartFailure::PortInUse,
            ),
            // 6.5 bind to unavailable address: not "port in use", so no honest
            // variant; Unknown keeps the raw tail.
            (
                "<M>:M ... # Warning: Could not create server TCP listening socket 10.255.255.1:41502: bind: Cannot assign requested address\n<M>:M ... # Failed listening on port 41502 (tcp), aborting.\n",
                StartFailure::Unknown,
            ),
            // 6.2 bad config: unknown directive
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 10\n>>> 'nonexistent_directive_on_purpose 1'\nBad directive or wrong number of arguments\n",
                StartFailure::Unknown,
            ),
            // 6.3 bad config: wrong value type
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 10\n>>> 'maxmemory not-a-number'\nargument must be a memory value\n",
                StartFailure::Unknown,
            ),
            // 6.4 bad config: port out of range
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 3\n>>> 'port 99999'\nargument must be between 0 and 65535 inclusive\n",
                StartFailure::Unknown,
            ),
            // 6.6 missing data directory: not a permission problem; Unknown.
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 7\n>>> 'dir /tmp/work/does-not-exist'\nNo such file or directory\n",
                StartFailure::Unknown,
            ),
            // 6.7 append-only dir: Permission denied is honest; "Not a
            // directory" is not, so it stays Unknown.
            (
                "<M>:M ... * Server initialized\n<M>:M ... # Can't open or create append-only dir appendonlydir: Permission denied\n",
                StartFailure::PermissionDenied,
            ),
            (
                "<M>:M ... * Server initialized\n<M>:M ... # Can't open or create append-only dir appendonlydir: Not a directory\n",
                StartFailure::Unknown,
            ),
        ]
    }

    #[test]
    fn diagnose_spike_verbatim_texts() {
        for (text, expected) in cases() {
            assert_eq!(diagnose_failure_text(text), expected, "text: {text:?}");
        }
    }

    #[test]
    fn diagnose_unknown_for_unrelated_text() {
        assert_eq!(
            diagnose_failure_text("# WARNING Memory overcommit must be enabled!\n"),
            StartFailure::Unknown
        );
    }

    #[test]
    fn sync_port_rewrites_only_port_line_atomically() {
        let dir = tempdir().expect("tempdir");
        let run = dir.path().join("run");
        fs::create_dir_all(&run).expect("run");
        let mut conf = render_config(&ConfigParams {
            port: 41000,
            data_dir: dir.path().join("data").as_path(),
            run_dir: run.as_path(),
            logs_dir: dir.path().join("logs").as_path(),
            password: None,
            auth: AuthMode::None,
        })
        .expose()
        .to_owned();
        conf.push_str("# keep-me\n");
        fs::write(config_path(&run), conf).expect("write");
        sync_port(&run, 42000).expect("sync");
        let text = fs::read_to_string(config_path(&run)).expect("read");
        assert!(text.contains("port 42000\n"));
        assert!(!text.contains("port 41000\n"));
        assert!(text.contains("# keep-me"));
        assert_eq!(text.matches("\nport ").count(), 1);
        sync_port(&run, 42000).expect("idempotent");
        let again = fs::read_to_string(config_path(&run)).expect("read");
        assert_eq!(text, again);
    }

    #[test]
    fn sync_port_refuses_missing_port_directive() {
        let dir = tempdir().expect("tempdir");
        let run = dir.path().join("run");
        fs::create_dir_all(&run).expect("run");
        fs::write(config_path(&run), "bind 127.0.0.1\n").expect("write");
        let err = sync_port(&run, 42000).unwrap_err();
        assert!(matches!(err, DblError::Engine { .. }));
    }

    #[test]
    fn prepare_run_tightens_existing_permissive_dirs() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let run = dir.path().join("run");
        let logs = dir.path().join("logs");
        fs::create_dir_all(&data).expect("data");
        fs::create_dir_all(&run).expect("run");
        fs::create_dir_all(&logs).expect("logs");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&data, &run, &logs] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o755)).expect("chmod");
            }
        }
        let ctx = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/valkey"),
            data_dir: data.clone(),
            logs_dir: logs.clone(),
            run_dir: run.clone(),
            port: Port::new(41000).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::None,
            credential: None,
        };
        prepare_run(&ctx).expect("prepare");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&data, &run, &logs] {
                let mode = fs::metadata(p).expect("meta").permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "{} must be 0700", p.display());
            }
        }
    }
}
