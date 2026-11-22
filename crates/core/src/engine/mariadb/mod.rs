//! MariaDB engine adapter.
//!
//! Renders `my.cnf`, bootstraps with `mariadb-install-db`, starts `mariadbd`,
//! probes readiness with a MariaDB/MySQL protocol greeting, shuts down via
//! SIGTERM (graceful, bounded), and diagnoses start failures from real log text.

mod identity;
pub mod socket;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_hardening;

use std::fmt::Write as _;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::consts;
use crate::engine::uri::{self, exe_name};
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::{DblError, Result};
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

#[cfg(test)]
pub(super) use identity::{
    ExpectedIdentity, PidIdentity, cmdline_targets_instance, is_zombie_stat, parse_cmdline,
    pid_alive, read_pidfile, shutdown_verified_pid, verify_pid_identity, verify_ps_command_output,
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
        let sock = socket::choose_socket_path(&ctx.data_dir, ctx.port.get())?;
        socket::validate_socket_path_len(&sock)?;
        let conf = config_path(&ctx.data_dir);
        let rendered = render_config(&config_params(ctx, &sock)?);
        write_private_config(&conf, rendered.expose())?;
        run_install_db(ctx)?;
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

fn log_error_path(data_dir: &Path) -> PathBuf {
    sibling_dir(data_dir, consts::DIR_LOGS).join("mariadb.err")
}

fn tmpdir_path(data_dir: &Path) -> PathBuf {
    sibling_dir(data_dir, consts::DIR_RUN).join("tmp")
}

#[cfg(test)]
fn socket_path(data_dir: &Path, port: u16) -> PathBuf {
    socket::natural_socket_path(data_dir, port)
}

fn sibling_dir(data_dir: &Path, leaf: &str) -> PathBuf {
    match data_dir.parent() {
        Some(parent) => parent.join(leaf),
        None => data_dir.join(leaf),
    }
}

fn render_config(p: &ConfigParams) -> SecretString {
    let mut out = String::new();
    let _ = writeln!(out, "[mysqld]");
    let _ = writeln!(out, "basedir={}", p.basedir.display());
    let _ = writeln!(out, "datadir={}", p.datadir.display());
    let _ = writeln!(out, "bind-address=127.0.0.1");
    let _ = writeln!(out, "port={}", p.port);
    let _ = writeln!(out, "socket={}", p.socket.display());
    let _ = writeln!(out, "pid-file={}", p.pid_file.display());
    let _ = writeln!(out, "log-error={}", p.log_error.display());
    let _ = writeln!(out, "tmpdir={}", p.tmpdir.display());
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
    ensure_private_tmpdir(&tmp)?;
    Ok(())
}

fn ensure_private_tmpdir(dir: &Path) -> Result<()> {
    if dir.exists() {
        let meta = fs::symlink_metadata(dir).map_err(|e| DblError::Engine {
            reason: format!("cannot inspect tmpdir {}: {e}", dir.display()),
        })?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(DblError::Engine {
                reason: format!("tmpdir {} is not a real directory", dir.display()),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if meta.uid() != nix::unistd::geteuid().as_raw() {
                return Err(DblError::Engine {
                    reason: format!("tmpdir {} has wrong owner", dir.display()),
                });
            }
            let mode = meta.mode() & 0o777;
            if mode != 0o700 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
                    DblError::Engine {
                        reason: format!("cannot secure tmpdir {}: {e}", dir.display()),
                    }
                })?;
            }
        }
        clean_stale_tmpdir(dir)?;
        return Ok(());
    }
    crate::fsutil::ensure_private_dir(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot create tmpdir {}: {e}", dir.display()),
    })?;
    Ok(())
}

fn clean_stale_tmpdir(dir: &Path) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot read tmpdir {}: {e}", dir.display()),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| DblError::Engine {
            reason: format!("cannot read tmpdir entry in {}: {e}", dir.display()),
        })?;
        let path = entry.path();
        let ft = entry.file_type().map_err(|e| DblError::Engine {
            reason: format!("cannot inspect tmpdir entry {}: {e}", path.display()),
        })?;
        if ft.is_dir() {
            fs::remove_dir_all(&path).map_err(|e| DblError::Engine {
                reason: format!("cannot remove stale tmpdir entry {}: {e}", path.display()),
            })?;
        } else {
            fs::remove_file(&path).map_err(|e| DblError::Engine {
                reason: format!("cannot remove stale tmpdir entry {}: {e}", path.display()),
            })?;
        }
    }
    Ok(())
}

fn read_config_socket_path(data_dir: &Path) -> Result<PathBuf> {
    let path = config_path(data_dir);
    let contents = fs::read_to_string(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot read {}: {e}", path.display()),
    })?;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        let value = trimmed
            .strip_prefix("socket=")
            .or_else(|| trimmed.strip_prefix("socket ="));
        let Some(value) = value else { continue };
        let value = value.trim();
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    Err(DblError::Engine {
        reason: format!("{} has no `socket` directive", path.display()),
    })
}

/// Prepares volatile runtime directories before every start.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    let tmp = tmpdir_path(&ctx.data_dir);
    ensure_private_tmpdir(&tmp)?;

    let socket = read_config_socket_path(&ctx.data_dir)?;
    let socket_dir = socket.parent().ok_or_else(|| DblError::Engine {
        reason: format!("socket path {} has no parent", socket.display()),
    })?;
    let natural_dir = sibling_dir(&ctx.data_dir, consts::DIR_RUN);
    if socket_dir != natural_dir {
        socket::ensure_secure_runtime_dir(socket_dir)?;
    } else {
        crate::fsutil::ensure_private_dir(socket_dir).map_err(|e| DblError::Engine {
            reason: format!("cannot create run directory {}: {e}", socket_dir.display()),
        })?;
    }
    if socket.exists() {
        let meta = fs::symlink_metadata(&socket).map_err(|e| DblError::Engine {
            reason: format!("cannot inspect socket {}: {e}", socket.display()),
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            if meta.file_type().is_socket() {
                fs::remove_file(&socket).map_err(|e| DblError::Engine {
                    reason: format!("cannot remove stale socket {}: {e}", socket.display()),
                })?;
            }
        }
    }
    Ok(())
}

fn write_private_config(path: &Path, contents: &str) -> Result<()> {
    crate::fsutil::write_private_file_atomic(path, contents.as_bytes()).map_err(|e| {
        DblError::Engine {
            reason: format!("cannot write {}: {e}", path.display()),
        }
    })
}

fn install_db_program(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("scripts").join("mariadb-install-db")
}

fn build_install_db_command(ctx: &InitContext) -> Command {
    let program = install_db_program(&ctx.runtime_dir);
    let mut cmd = Command::new(program);
    cmd.arg(format!("--datadir={}", ctx.data_dir.display()))
        .arg(format!("--basedir={}", ctx.runtime_dir.display()))
        .arg("--auth-root-authentication-method=normal")
        .arg("--skip-test-db");
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
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env_remove("MYSQL_PWD");
    cmd.env_remove("MYSQL_TCP_PORT");
    cmd.env("TMPDIR", tmpdir_path(&ctx.data_dir));
    cmd
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

    if !status.success() {
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
        Ok(greeting) => {
            if greeting.protocol_version == PROTOCOL_VERSION_10 {
                Ok(Health::Ready)
            } else {
                Ok(Health::NotReady(format!(
                    "unexpected protocol version {}",
                    greeting.protocol_version
                )))
            }
        }
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
    let sock = socket::choose_socket_path(data_dir, port)?;
    socket::validate_socket_path_len(&sock)?;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("port=") || trimmed.starts_with("port =") {
            let _ = writeln!(out, "port={port}");
            replaced_port = true;
        } else if trimmed.starts_with("socket=") || trimmed.starts_with("socket =") {
            let _ = writeln!(out, "socket={}", sock.display());
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
