//! MariaDB engine adapter.
//!
//! Renders `my.cnf`, bootstraps with `mariadb-install-db`, starts `mariadbd`,
//! probes readiness with a MariaDB/MySQL protocol greeting, shuts down via
//! SIGTERM (graceful, bounded), and diagnoses start failures from real log text.

mod identity;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_hardening;
#[cfg(test)]
mod tests_windows;
#[cfg(test)]
mod testutil;

use std::fmt::Write as _;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::consts;
use crate::engine::sockdir;
use crate::engine::uri::{self, exe_name};
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::{DblError, Result};
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

#[cfg(test)]
pub(super) use identity::{
    ExpectedIdentity, PidIdentity, cmdline_targets_instance, contains_arg_token, is_zombie_stat,
    parse_cmdline, path_arg_matches, pid_alive, read_pidfile, same_executable,
    shutdown_verified_pid, strip_deleted, verify_pid_identity, verify_ps_command_output,
};
/// MariaDB protocol version byte for the server greeting.
const PROTOCOL_VERSION_10: u8 = 10;
/// Max greeting bytes we read before considering the packet hostile.
const MAX_GREETING_BYTES: usize = 16384;

/// MariaDB adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct MariaDbAdapter;

impl EngineAdapter for MariaDbAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Mariadb
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        prepare_dirs(ctx)?;
        let sock = resolve_socket_path(&ctx.data_dir, ctx.port.get())?;
        sockdir::validate_socket_path_len(&sock)?;
        let conf = config_path(&ctx.data_dir);
        let rendered = render_config(&config_params(ctx, &sock)?);
        if cfg!(windows) {
            // The Windows tool refuses a data directory that is not empty, so
            // the option file goes in after it ran.
            run_install_db(ctx)?;
            write_private_config(&conf, rendered.expose())?;
        } else {
            write_private_config(&conf, rendered.expose())?;
            run_install_db(ctx)?;
        }
        if ctx.auth == AuthMode::Password {
            bootstrap_password(ctx)?;
        }
        tracing::debug!(
            instance = %ctx.instance_name,
            socket = %sock.display(),
            "mariadb initialized"
        );
        Ok(())
    }

    fn spawn_command(&self, ctx: &RunContext) -> Command {
        let server = ctx.runtime_dir.join("bin").join(exe_name("mariadbd"));
        let mut cmd = Command::new(server);
        cmd.arg(format!(
            "--defaults-file={}",
            config_path(&ctx.data_dir).display()
        ));
        cmd.stdin(Stdio::null());
        // Never forward a password in the environment to the server process.
        cmd.env_remove("MYSQL_PWD");
        cmd.env_remove("MYSQL_TCP_PORT");
        cmd.env("TMPDIR", tmpdir_path(&ctx.data_dir));
        cmd
    }

    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        probe_greeting(ctx.port.get())
    }

    fn shutdown(&self, ctx: &RunContext) -> Result<()> {
        identity::shutdown_by_sigterm(ctx)
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString {
        uri::render(EngineKey::Mariadb, ctx.port.get(), credential, format)
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        diagnose_failure_text(output)
    }

    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        None
    }
}

#[derive(Debug, Clone)]
struct ConfigParams {
    basedir: PathBuf,
    datadir: PathBuf,
    port: u16,
    socket: PathBuf,
    pid_file: PathBuf,
    log_error: PathBuf,
    tmpdir: PathBuf,
}

fn config_params(ctx: &InitContext, socket: &Path) -> Result<ConfigParams> {
    Ok(ConfigParams {
        basedir: ctx.runtime_dir.clone(),
        datadir: ctx.data_dir.clone(),
        port: ctx.port.get(),
        socket: socket.to_path_buf(),
        pid_file: pidfile_path(&ctx.data_dir),
        log_error: log_error_path(&ctx.data_dir),
        tmpdir: tmpdir_path(&ctx.data_dir),
    })
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("my.cnf")
}

fn pidfile_path(data_dir: &Path) -> PathBuf {
    data_dir.join("mariadbd.pid")
}

pub fn log_error_path(data_dir: &Path) -> PathBuf {
    // Must equal `instance::helpers::log_file_for`: the service's log viewer and
    // `failure_tail` read `logs/mariadb.log`, and a server started with
    // `log-error` writes there, not to stderr (finding 7).
    sibling_dir(data_dir, consts::DIR_LOGS).join("mariadb.log")
}

fn tmpdir_path(data_dir: &Path) -> PathBuf {
    sibling_dir(data_dir, consts::DIR_RUN).join("tmp")
}

#[cfg(test)]
fn socket_path(data_dir: &Path, port: u16) -> PathBuf {
    natural_socket_path(data_dir, port)
}

/// Natural socket path under the instance run directory.
fn natural_socket_path(data_dir: &Path, port: u16) -> PathBuf {
    sibling_run_dir(data_dir).join(format!("mariadb-{port}.sock"))
}

/// Chooses the socket path for an instance.
///
/// Returns the natural run-dir path when it fits, otherwise the first verified
/// short fallback directory whose `m<port>-<12 hex>.sock` fits.
fn resolve_socket_path(data_dir: &Path, port: u16) -> Result<PathBuf> {
    let natural = natural_socket_path(data_dir, port);
    sockdir::resolve_socket_path(
        &natural,
        port,
        data_dir,
        std::env::var_os("XDG_RUNTIME_DIR"),
        sockdir::current_uid(),
    )
}

fn sibling_run_dir(data_dir: &Path) -> PathBuf {
    sibling_dir(data_dir, consts::DIR_RUN)
}

fn sibling_dir(data_dir: &Path, leaf: &str) -> PathBuf {
    match data_dir.parent() {
        Some(parent) => parent.join(leaf),
        None => data_dir.join(leaf),
    }
}

fn render_config(p: &ConfigParams) -> SecretString {
    render_config_for(p, cfg!(windows))
}

/// [`render_config`] with the platform passed in, so both spellings are tested.
fn render_config_for(p: &ConfigParams, windows: bool) -> SecretString {
    let path = |value: &Path| crate::fsutil::option_file_path(value, windows);
    let mut out = String::new();
    let _ = writeln!(out, "[mysqld]");
    let _ = writeln!(out, "basedir={}", path(&p.basedir));
    let _ = writeln!(out, "datadir={}", path(&p.datadir));
    let _ = writeln!(out, "bind-address=127.0.0.1");
    let _ = writeln!(out, "port={}", p.port);
    let _ = writeln!(out, "socket={}", path(&p.socket));
    let _ = writeln!(out, "pid-file={}", path(&p.pid_file));
    let _ = writeln!(out, "log-error={}", path(&p.log_error));
    let _ = writeln!(out, "tmpdir={}", path(&p.tmpdir));
    let _ = writeln!(out, "skip-name-resolve");
    // Never 0.0.0.0; never a password in config.
    SecretString::new(out)
}

fn prepare_dirs(ctx: &InitContext) -> Result<()> {
    let logs = sibling_dir(&ctx.data_dir, consts::DIR_LOGS);
    let run = sibling_dir(&ctx.data_dir, consts::DIR_RUN);
    let tmp = tmpdir_path(&ctx.data_dir);
    for (dir, label) in [(&ctx.data_dir, "data"), (&logs, "logs"), (&run, "run")] {
        crate::fsutil::ensure_private_dir(dir).map_err(|e| DblError::Engine {
            reason: format!("cannot create {label} directory {}: {e}", dir.display()),
        })?;
    }
    sockdir::prepare_tmpdir(&tmp, sockdir::current_uid())?;
    Ok(())
}

fn prepare_run_spec(data_dir: &Path, port: u16) -> Result<()> {
    let spec = sockdir::PrepareSpec {
        config: &config_path(data_dir),
        natural_socket: &natural_socket_path(data_dir, port),
        port,
        tmpdir: &tmpdir_path(data_dir),
        xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
        uid: sockdir::current_uid(),
    };
    sockdir::prepare_run(&spec)
}

/// The socket path the instance's `my.cnf` names, or the path `initialize`
/// would choose when the config does not exist yet.
///
/// Admin connects over this socket so the password is never sent over a
/// plaintext TCP stream and the bootstrap's `root@localhost` account (which
/// only the socket can reach) is the one used. Expressed over the shared
/// [`sockdir`] (a `socket=` directive read, then the same fallback resolution
/// `initialize` uses), so it never re-implements socket logic.
pub fn configured_socket_path(data_dir: &Path, port: u16) -> Result<PathBuf> {
    if let Some(socket) = sockdir::read_directive(&config_path(data_dir), "socket") {
        return Ok(PathBuf::from(socket));
    }
    resolve_socket_path(data_dir, port)
}

/// Prepares volatile runtime directories before every start.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    prepare_run_spec(&ctx.data_dir, ctx.port.get())
}

fn write_private_config(path: &Path, contents: &str) -> Result<()> {
    crate::fsutil::write_private_file_atomic(path, contents.as_bytes()).map_err(|e| {
        DblError::Engine {
            reason: format!("cannot write {}: {e}", path.display()),
        }
    })
}

/// The bootstrap tool. Unix ships the `scripts/mariadb-install-db` shell
/// script; the Windows zip has no `scripts/` directory and ships
/// `bin/mariadb-install-db.exe` instead.
fn install_db_program(runtime_dir: &Path, windows: bool) -> PathBuf {
    if windows {
        runtime_dir.join("bin").join("mariadb-install-db.exe")
    } else {
        runtime_dir.join("scripts").join("mariadb-install-db")
    }
}

/// Arguments of the bootstrap tool.
///
/// The Windows tool is a different program: it takes neither `--basedir`
/// (it finds `mariadbd.exe` next to itself), `--auth-root-authentication-method`
/// nor `--skip-test-db`, and it leaves `root` with an empty password, which
/// `bootstrap_password` replaces afterwards like on Unix.
fn install_db_args(data_dir: &Path, runtime_dir: &Path, windows: bool) -> Vec<String> {
    if windows {
        return vec![
            format!("--datadir={}", data_dir.display()),
            "--silent".to_owned(),
        ];
    }
    vec![
        format!("--datadir={}", data_dir.display()),
        format!("--basedir={}", runtime_dir.display()),
        "--auth-root-authentication-method=normal".to_owned(),
        "--skip-test-db".to_owned(),
    ]
}

fn build_install_db_command(ctx: &InitContext) -> Command {
    build_install_db_command_for(ctx, cfg!(windows))
}

fn build_install_db_command_for(ctx: &InitContext, windows: bool) -> Command {
    let mut cmd = Command::new(install_db_program(&ctx.runtime_dir, windows));
    cmd.args(install_db_args(&ctx.data_dir, &ctx.runtime_dir, windows));
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env_remove("MYSQL_PWD");
    cmd.env("TMPDIR", tmpdir_path(&ctx.data_dir));
    cmd
}

fn run_install_db(ctx: &InitContext) -> Result<()> {
    let mut cmd = build_install_db_command(ctx);
    let output = cmd.output().map_err(|e| {
        let _ = fs::remove_dir_all(&ctx.data_dir);
        DblError::Engine {
            reason: format!("failed to execute mariadb-install-db: {e}"),
        }
    })?;
    if !output.status.success() {
        let tail = combined_output(&output.stdout, &output.stderr);
        let _ = fs::remove_dir_all(&ctx.data_dir);
        return Err(DblError::Engine {
            reason: format!(
                "mariadb-install-db failed (code {:?}): {}",
                output.status.code(),
                tail.trim_end()
            ),
        });
    }
    Ok(())
}

fn bootstrap_password(ctx: &InitContext) -> Result<()> {
    let Some(password) = ctx.password.as_ref() else {
        return Err(DblError::InvalidInput {
            field: "password",
            reason: "password auth requires a generated password".to_owned(),
        });
    };
    // Write a 0600 init-file with the password-change SQL. The file is deleted
    // immediately after the bootstrap start completes (success or failure).
    let init_file = sibling_dir(&ctx.data_dir, consts::DIR_RUN).join(".mariadb-init.sql");
    let sql = password_change_sql(password);
    crate::fsutil::write_private_file_atomic(&init_file, sql.as_bytes()).map_err(|e| {
        DblError::Engine {
            reason: format!("cannot write init-file {}: {e}", init_file.display()),
        }
    })?;
    let result = run_bootstrap_server(ctx, &init_file);
    let _ = fs::remove_file(&init_file);
    result
}

fn password_change_sql(password: &SecretString) -> String {
    let escaped = escape_sql_single_quoted(password.expose());
    format!(
        "ALTER USER 'root'@'localhost' IDENTIFIED BY '{escaped}';\n\
         ALTER USER 'root'@'127.0.0.1' IDENTIFIED BY '{escaped}';\n\
         ALTER USER 'root'@'::1' IDENTIFIED BY '{escaped}';\n\
         FLUSH PRIVILEGES;\n\
         SHUTDOWN;\n"
    )
}

fn escape_sql_single_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("''"),
            '\0' => out.push_str("\\0"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{1a}' => out.push_str("\\Z"),
            _ => out.push(ch),
        }
    }
    out
}

fn build_bootstrap_command(ctx: &InitContext, init_file: &Path) -> Command {
    let server = ctx.runtime_dir.join("bin").join(exe_name("mariadbd"));
    let mut cmd = Command::new(server);
    cmd.arg(format!(
        "--defaults-file={}",
        config_path(&ctx.data_dir).display()
    ));
    cmd.arg(format!("--init-file={}", init_file.display()));
    // The bootstrap server only needs the local socket for the init-file SQL
    // (including SHUTDOWN). Binding the TCP instance port would make initialize
    // depend on that port being free between the port check and bootstrap.
    if !cfg!(windows) {
        // `mariadbd.exe` refuses to start with TCP and named pipes both off
        // ("Either TCP connections or named pipe connections must be enabled"),
        // so on Windows the bootstrap listens on the loopback port that
        // `my.cnf` binds (`bind-address=127.0.0.1`).
        cmd.arg("--skip-networking");
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env_remove("MYSQL_PWD");
    cmd.env_remove("MYSQL_TCP_PORT");
    cmd.env("TMPDIR", tmpdir_path(&ctx.data_dir));
    cmd
}

/// Whether the bootstrap server's exit counts as success.
///
/// The server runs the init-file and then its `SHUTDOWN` statement. On Unix that
/// ends with status 0. On Windows `mariadbd.exe` exits with status 1 after the
/// same `SHUTDOWN` even though the password change was applied (verified in a
/// Windows 10 VM: the password logs in, the empty one is refused). A statement
/// that fails does not end that way: the error is logged and `SHUTDOWN` never
/// runs, so the server keeps running and the caller's start timeout reports it.
/// A Windows exit therefore means "the init-file ran to its end".
fn bootstrap_exit_ok(success: bool, windows: bool) -> bool {
    success || windows
}

fn run_bootstrap_server(ctx: &InitContext, init_file: &Path) -> Result<()> {
    let mut cmd = build_bootstrap_command(ctx, init_file);
    let mut child = cmd.spawn().map_err(|e| {
        let _ = fs::remove_dir_all(&ctx.data_dir);
        DblError::Engine {
            reason: format!("failed to start mariadbd for password bootstrap: {e}"),
        }
    })?;

    let deadline = Instant::now() + Duration::from_secs(consts::START_TIMEOUT_SECS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&ctx.data_dir);
                return Err(DblError::Engine {
                    reason: "mariadb password bootstrap timed out".to_owned(),
                });
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&ctx.data_dir);
                return Err(DblError::Engine {
                    reason: format!("mariadb password bootstrap wait failed: {e}"),
                });
            }
        }
    };

    if !bootstrap_exit_ok(status.success(), cfg!(windows)) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut out) = child.stdout.take() {
            use std::io::Read as _;
            let _ = out.read_to_end(&mut stdout);
        }
        if let Some(mut err) = child.stderr.take() {
            use std::io::Read as _;
            let _ = err.read_to_end(&mut stderr);
        }
        let tail = combined_output(&stdout, &stderr);
        let log_tail = read_log_tail(&log_error_path(&ctx.data_dir), 40);
        let _ = fs::remove_dir_all(&ctx.data_dir);
        let detail = if log_tail.is_empty() { tail } else { log_tail };
        return Err(DblError::Engine {
            reason: format!(
                "mariadb password bootstrap failed (code {:?}): {}",
                status.code(),
                detail.trim_end()
            ),
        });
    }
    Ok(())
}

fn read_log_tail(path: &Path, max_lines: usize) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut out = String::from_utf8_lossy(stdout).into_owned();
    let err = String::from_utf8_lossy(stderr);
    if !err.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&err);
    }
    out
}

fn probe_greeting(port: u16) -> Result<Health> {
    let addr = format!("127.0.0.1:{port}");
    let stream = TcpStream::connect_timeout(
        &addr.parse().map_err(|e| DblError::Engine {
            reason: format!("invalid mariadb address {addr}: {e}"),
        })?,
        Duration::from_secs(5),
    )
    .map_err(|e| DblError::Engine {
        reason: format!("mariadb health connect failed: {e}"),
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    match read_greeting_packet(&stream) {
        // `parse_greeting` only yields `Ok` for protocol version 10, so a
        // parsed greeting is always a ready server.
        Ok(_greeting) => Ok(Health::Ready),
        Err(GreetingError::Io(e)) => Err(DblError::Engine {
            reason: format!("mariadb greeting read failed: {e}"),
        }),
        Err(GreetingError::Invalid(reason)) => {
            if reason.contains("error packet") {
                // Server spoke protocol; an auth/error response still proves liveness.
                Ok(Health::Ready)
            } else {
                Err(DblError::Engine {
                    reason: format!("mariadb greeting invalid: {reason}"),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Greeting {
    protocol_version: u8,
    server_version: String,
}

#[derive(Debug)]
enum GreetingError {
    Io(std::io::Error),
    Invalid(String),
}

impl From<std::io::Error> for GreetingError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn read_greeting_packet(mut stream: &TcpStream) -> std::result::Result<Greeting, GreetingError> {
    use std::io::Read as _;
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr)?;
    let plen = (hdr[0] as usize) | ((hdr[1] as usize) << 8) | ((hdr[2] as usize) << 16);
    if plen == 0 {
        return Err(GreetingError::Invalid("empty packet".to_owned()));
    }
    if plen > MAX_GREETING_BYTES {
        return Err(GreetingError::Invalid(format!(
            "greeting too large: {plen} bytes"
        )));
    }
    let mut payload = vec![0u8; plen];
    stream.read_exact(&mut payload)?;
    parse_greeting(&payload)
}

fn parse_greeting(payload: &[u8]) -> std::result::Result<Greeting, GreetingError> {
    if payload.is_empty() {
        return Err(GreetingError::Invalid("empty payload".to_owned()));
    }
    let proto = payload[0];
    if proto == 0xff {
        // Error packet - server is up but rejected something; treat as invalid
        // greeting for health, caller may map. For connect health, getting an
        // error packet still means server spoke protocol.
        return Err(GreetingError::Invalid(
            "received error packet instead of greeting".to_owned(),
        ));
    }
    if proto != PROTOCOL_VERSION_10 {
        return Err(GreetingError::Invalid(format!(
            "unsupported protocol version {proto}"
        )));
    }
    let rest = &payload[1..];
    let Some(nul) = rest.iter().position(|b| *b == 0) else {
        return Err(GreetingError::Invalid(
            "server version not null-terminated".to_owned(),
        ));
    };
    let server_version = String::from_utf8_lossy(&rest[..nul]).into_owned();
    Ok(Greeting {
        protocol_version: proto,
        server_version,
    })
}

/// Rewrites managed port/socket directives in `my.cnf`.
///
/// Atomic. Idempotent. Fails if the config is missing or has no `port`.
pub fn sync_port(data_dir: &Path, port: u16) -> Result<()> {
    let path = config_path(data_dir);
    let contents = fs::read_to_string(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot read {}: {e}", path.display()),
    })?;
    let mut replaced_port = false;
    let mut out = String::with_capacity(contents.len() + 32);
    let sock = resolve_socket_path(data_dir, port)?;
    sockdir::validate_socket_path_len(&sock)?;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("port=") || trimmed.starts_with("port =") {
            let _ = writeln!(out, "port={port}");
            replaced_port = true;
        } else if trimmed.starts_with("socket=") || trimmed.starts_with("socket =") {
            let _ = writeln!(
                out,
                "socket={}",
                crate::fsutil::option_file_path(&sock, cfg!(windows))
            );
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced_port {
        return Err(DblError::Engine {
            reason: format!("{} has no `port` directive to rewrite", path.display()),
        });
    }
    write_private_config(&path, &out)
}

fn diagnose_failure_text(output: &str) -> StartFailure {
    let lower = output.to_ascii_lowercase();

    // Port in use - verbatim spike: "Can't start server: Bind on TCP/IP port.
    // Got error: 98: Address already in use" / "Do you already have another
    // server running on port: ..."
    if output.contains("Address already in use")
        || output.contains("Do you already have another server running on port")
        || output.contains("Can't start server: Bind on TCP/IP port")
    {
        return StartFailure::PortInUse;
    }

    // Socket path too long - verbatim: "The socket file path is too long (> 107):"
    // No dedicated StartFailure variant; keep Unknown with raw tail.
    if output.contains("socket file path is too long") || output.contains("is too long (> 107)") {
        return StartFailure::Unknown;
    }

    // Data dir version / incompatible data dir. MariaDB opening MySQL 8.4 datadir
    // showed InnoDB errors: "File ./ib_logfile0 was not found", "Plugin 'InnoDB'
    // registration as a STORAGE ENGINE failed", "Unknown/unsupported storage engine: InnoDB".
    // Cross-major MariaDB sometimes upgrades silently, so we only match explicit
    // incompatibility markers.
    if output.contains("database files are incompatible with server")
        || output.contains("initialized by MariaDB version")
        || output.contains("initialized by MySQL version")
        || output.contains("is not a MySQL data directory")
        || (output.contains("Unknown/unsupported storage engine: InnoDB")
            && (output.contains("ib_logfile0 was not found")
                || output.contains("Plugin initialization aborted")))
        || (output.contains("Plugin 'InnoDB' registration as a STORAGE ENGINE failed")
            && output.contains("Unknown/unsupported storage engine: InnoDB"))
    {
        return StartFailure::DataDirVersionMismatch;
    }

    // Permission denied.
    if output.contains("Permission denied") || lower.contains("permission denied") {
        return StartFailure::PermissionDenied;
    }

    // Bad config / unknown variable.
    if output.contains("unknown variable")
        || output.contains("unknown option")
        || output.contains("Bad directive")
        || lower.contains("you have an error in your option")
    {
        // No dedicated StartFailure variant; raw tail preserved by caller.
        return StartFailure::Unknown;
    }

    // Missing/bad data dir - "Can't change dir to" / "Errcode: 2 "No such file..."
    // Not a binary-missing case; keep Unknown.
    if output.contains("Can't change dir to") {
        return StartFailure::Unknown;
    }

    // Disk full.
    if output.contains("No space left on device") || lower.contains("no space left on device") {
        return StartFailure::Unknown;
    }

    // Binary/loader missing.
    if output.contains("Exec format error")
        || output.contains("error while loading shared libraries")
        || (output.contains("not found (required by") && output.contains("mariadbd"))
        || (output.contains("No such file or directory")
            && (output.contains("mariadbd") || output.contains("mariadb-install-db")))
    {
        return StartFailure::BinaryMissing;
    }

    StartFailure::Unknown
}
