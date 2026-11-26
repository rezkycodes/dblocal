//! Shared RESP (Redis Serialization Protocol) engine implementation.
//!
//! Both [`crate::engine::ValkeyAdapter`] and [`crate::engine::RedisAdapter`]
//! delegate to this module, parameterised by [`RespBrand`].

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::consts;
use crate::engine::uri::{self, exe_name};
use crate::engine::{Credential, Health, InitContext, RunContext};
use crate::error::{DblError, Result};
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

/// RESP engines listen on IPv4 and IPv6 loopback only (never `0.0.0.0`).
pub const BIND: &str = "127.0.0.1 -::1";
/// Dependency policy: AOF keeps data across a forced kill (C-11).
const PERSISTENCE: &str = "appendonly yes\nappendfilename \"appendonly.aof\"\nappenddirname \"appendonlydir\"\nsave 3600 1 300 100 60 10000";
/// Mode passed to `SHUTDOWN`; AOF already carries committed data.
pub const SHUTDOWN_MODE: &str = "NOSAVE";

/// Engine-specific naming and identity parameters for a RESP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RespBrand {
    /// Domain engine key (`Valkey` or `Redis`).
    pub engine: EngineKey,
    /// Lowercase engine label for logs and error messages.
    pub label: &'static str,
    /// Primary server executable base name (`valkey-server` or `redis-server`).
    pub server_bin: &'static str,
    /// Optional multi-call fallback binary (e.g. `redis-check-rdb` in the
    /// official `redis-tools` `.deb` package, invoked with `argv[0] = server_bin`).
    pub multicall_bin: Option<&'static str>,
    /// Managed config file name inside `run_dir`.
    pub config_file: &'static str,
    /// Server pidfile name inside `run_dir`.
    pub pid_file: &'static str,
    /// Server logfile name inside `logs_dir`.
    pub log_file: &'static str,
    /// Default superuser name (`default`).
    pub superuser: &'static str,
}

/// Inputs of a rendered RESP config file (`valkey.conf` / `redis.conf`).
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

/// Parsed RESP pidfile: the server writes a single PID line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerIdentity {
    /// Server process id.
    pub pid: u32,
}

/// Initializes instance directories and writes the managed config file.
pub fn initialize(brand: &RespBrand, ctx: &InitContext) -> Result<()> {
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
    let rendered = render_config(brand, &params);
    write_private_config_atomic(&config_path(brand, &run_dir), rendered.expose())?;
    tracing::debug!(instance = %ctx.instance_name, file = brand.config_file, "config written");
    Ok(())
}

/// Builds the detached server spawn command.
///
/// When `brand.multicall_bin` is set and `bin/<server_bin>` is absent (as in
/// the official `redis-tools` `.deb` package), launches `bin/<multicall_bin>`
/// with `argv[0] = server_bin` on Unix so the multi-call binary enters server
/// mode.
pub fn spawn_command(brand: &RespBrand, ctx: &RunContext) -> Command {
    let primary = ctx.runtime_dir.join("bin").join(exe_name(brand.server_bin));
    let (program, use_arg0) = match brand.multicall_bin {
        Some(multicall) if !primary.exists() => (
            ctx.runtime_dir.join("bin").join(exe_name(multicall)),
            true,
        ),
        _ => (primary, false),
    };
    let mut cmd = Command::new(&program);
    #[cfg(unix)]
    if use_arg0 {
        use std::os::unix::process::CommandExt;
        cmd.arg0(brand.server_bin);
    }
    #[cfg(not(unix))]
    let _ = use_arg0;
    cmd.arg(config_path(brand, &ctx.run_dir));
    cmd.stdin(Stdio::null());
    cmd
}

/// Probes the server with RESP `PING`.
pub fn health_check(brand: &RespBrand, ctx: &RunContext) -> Result<Health> {
    let client = client_for(brand, ctx)?;
    let mut con = client
        .get_connection_with_timeout(Duration::from_secs(5))
        .map_err(|e| classify_redis_connect(brand, &e))?;
    match redis::cmd("PING").query::<String>(&mut con) {
        Ok(reply) if reply == "PONG" => Ok(Health::Ready),
        Ok(reply) => Ok(Health::NotReady(format!("unexpected PING reply: {reply}"))),
        Err(err) => {
            if is_auth_challenge(&err) {
                Ok(Health::Ready)
            } else {
                Err(DblError::Engine {
                    reason: format!("{} PING failed: {err}", brand.label),
                })
            }
        }
    }
}

/// Requests a clean `SHUTDOWN NOSAVE`.
pub fn shutdown(brand: &RespBrand, ctx: &RunContext) -> Result<()> {
    let client = client_for(brand, ctx)?;
    let mut con = client
        .get_connection_with_timeout(Duration::from_secs(consts::STOP_GRACE_SECS))
        .map_err(|e| classify_redis_connect(brand, &e))?;
    let reply: redis::RedisResult<()> = redis::cmd("SHUTDOWN").arg(SHUTDOWN_MODE).query(&mut con);
    match reply {
        Ok(()) => Ok(()),
        Err(err) if is_connection_closed(&err) => Ok(()),
        Err(err) => Err(DblError::Engine {
            reason: format!("{} SHUTDOWN failed: {err}", brand.label),
        }),
    }
}

/// Renders the managed config file. Secret because it may carry `requirepass`.
pub fn render_config(brand: &RespBrand, params: &ConfigParams<'_>) -> SecretString {
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
    let _ = writeln!(
        out,
        "pidfile {}",
        pidfile_path(brand, params.run_dir).display()
    );
    let _ = writeln!(
        out,
        "logfile {}",
        logfile_path(brand, params.logs_dir).display()
    );
    let _ = writeln!(out, "databases 16");
    let _ = writeln!(out, "dir {}", params.data_dir.display());
    if params.auth == AuthMode::Password
        && let Some(password) = params.password
    {
        let _ = writeln!(out, "requirepass {}", password.expose());
    }
    let _ = writeln!(out, "{PERSISTENCE}");
    SecretString::new(out)
}

/// Path of the generated config inside `run_dir`.
pub fn config_path(brand: &RespBrand, run_dir: &Path) -> PathBuf {
    run_dir.join(brand.config_file)
}

/// Path of the server pidfile inside `run_dir`.
pub fn pidfile_path(brand: &RespBrand, run_dir: &Path) -> PathBuf {
    run_dir.join(brand.pid_file)
}

/// Path of the server logfile inside `logs_dir`.
pub fn logfile_path(brand: &RespBrand, logs_dir: &Path) -> PathBuf {
    logs_dir.join(brand.log_file)
}

/// Ensures the directories the server needs exist with 0700 permissions.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    ensure_private_dir(&ctx.data_dir, "data")?;
    ensure_private_dir(&ctx.run_dir, "run")?;
    ensure_private_dir(&ctx.logs_dir, "logs")?;
    Ok(())
}

/// Rewrites the managed `port` line in `run_dir/<config_file>` atomically.
pub fn sync_port(brand: &RespBrand, run_dir: &Path, port: u16) -> Result<()> {
    let path = config_path(brand, run_dir);
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

/// Returns `data_dir`'s sibling directory `leaf` (`run`, `logs`).
pub fn sibling_dir(data_dir: &Path, leaf: &str) -> PathBuf {
    match data_dir.parent() {
        Some(parent) => parent.join(leaf),
        None => data_dir.join(leaf),
    }
}

/// Parses a RESP server pidfile (a single PID line).
pub fn parse_pidfile(brand: &RespBrand, contents: &str) -> Result<ServerIdentity> {
    let first = contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| DblError::Engine {
            reason: format!("{} is empty", brand.pid_file),
        })?;
    let pid = first.parse::<u32>().map_err(|_| DblError::Engine {
        reason: format!("{} does not contain a pid", brand.pid_file),
    })?;
    Ok(ServerIdentity { pid })
}

/// Reads and parses `<run_dir>/<pid_file>`.
pub fn read_pidfile(brand: &RespBrand, run_dir: &Path) -> Result<ServerIdentity> {
    let path = pidfile_path(brand, run_dir);
    let contents = fs::read_to_string(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot read {}: {e}", path.display()),
    })?;
    parse_pidfile(brand, &contents)
}

/// Classifies raw RESP server start output or log tail into a [`StartFailure`].
pub fn diagnose_failure_text(output: &str) -> StartFailure {
    let lower = output.to_ascii_lowercase();

    if output.contains("Address already in use") {
        return StartFailure::PortInUse;
    }

    if output.contains("Permission denied") || lower.contains("permission denied") {
        return StartFailure::PermissionDenied;
    }

    if output.contains("Can't open or create append-only dir") {
        return StartFailure::Unknown;
    }

    StartFailure::Unknown
}

fn ensure_private_dir(dir: &Path, label: &str) -> Result<()> {
    crate::fsutil::ensure_private_dir(dir).map_err(|e| DblError::Engine {
        reason: format!(
            "cannot create/chmod {label} directory {}: {e}",
            dir.display()
        ),
    })
}

fn write_private_config_atomic(path: &Path, contents: &str) -> Result<()> {
    crate::fsutil::write_private_file_atomic(path, contents.as_bytes()).map_err(|e| {
        DblError::Engine {
            reason: format!("cannot write {}: {e}", path.display()),
        }
    })
}

fn client_for(brand: &RespBrand, ctx: &RunContext) -> Result<redis::Client> {
    let username = if ctx.superuser.is_empty() {
        brand.superuser.to_owned()
    } else {
        ctx.superuser.clone()
    };
    let password = ctx
        .credential
        .as_ref()
        .map(|c| c.password.clone())
        .unwrap_or_else(|| SecretString::new(String::new()));
    let credential = Credential::new(username, password);
    let uri = uri::render(brand.engine, ctx.port.get(), &credential, UriFormat::Uri);
    redis::Client::open(uri.expose()).map_err(|e| DblError::Engine {
        reason: format!("cannot build {} client: {e}", brand.label),
    })
}

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

fn classify_redis_connect(brand: &RespBrand, err: &redis::RedisError) -> DblError {
    if is_auth_challenge(err) {
        DblError::Engine {
            reason: format!("{} rejected the credentials", brand.label),
        }
    } else {
        DblError::Engine {
            reason: format!("cannot connect to {}: {err}", brand.label),
        }
    }
}
