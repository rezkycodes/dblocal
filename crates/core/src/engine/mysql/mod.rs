//! MySQL 8.4 LTS engine adapter.
//!
//! Implements `mysqld --initialize-insecure` bootstrap, `my.cnf` rendering,
//! `mysqld` start command construction, protocol handshake health checks,
//! graceful shutdown and start-failure diagnosis from real 8.4.11 behavior.

use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::consts;
use crate::engine::uri::{self, exe_name};
use crate::engine::{Credential, EngineAdapter, Health, InitContext, RunContext};
use crate::error::{DblError, Result};
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

/// MySQL listens on loopback only.
pub const BIND: &str = "127.0.0.1";
/// Name of the one-shot init SQL file used to bootstrap credentials.
const INIT_FILE_NAME: &str = "dblocal-init.sql";

/// MySQL adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct MysqlAdapter;

impl EngineAdapter for MysqlAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Mysql
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        let run_dir = sibling_dir(&ctx.data_dir, consts::DIR_RUN);
        let logs_dir = sibling_dir(&ctx.data_dir, consts::DIR_LOGS);
        let tmp_dir = tmpdir_path(&run_dir);
        for (dir, label) in [
            (&ctx.data_dir, "data"),
            (&run_dir, "run"),
            (&logs_dir, "logs"),
            (&tmp_dir, "tmp"),
        ] {
            ensure_private_dir(dir, label)?;
        }
        let instance_dir = ctx
            .data_dir
            .parent()
            .unwrap_or(ctx.data_dir.as_path())
            .to_path_buf();
        let socket = resolve_socket_path(&run_dir, ctx.port.get(), &instance_dir)?;

        let outcome = (|| -> Result<()> {
            run_initialize_insecure(ctx, &logs_dir)?;
            write_my_cnf(ctx, &run_dir, &logs_dir, &socket)?;
            bootstrap_credentials(ctx, &run_dir, &logs_dir)?;
            Ok(())
        })();

        if let Err(err) = outcome {
            let _ = fs::remove_dir_all(&ctx.data_dir);
            let _ = fs::remove_file(init_file_path(&run_dir));
            cleanup_fallback_socket(&socket, &run_dir, ctx.port.get());
            return Err(err);
        }
        let _ = fs::remove_file(init_file_path(&run_dir));
        tracing::debug!(instance = %ctx.instance_name, socket = %socket.display(), "mysql initialized");
        Ok(())
    }

    fn spawn_command(&self, ctx: &RunContext) -> Command {
        let mysqld = ctx.runtime_dir.join("bin").join(exe_name("mysqld"));
        let mut cmd = Command::new(mysqld);
        cmd.arg(format!(
            "--defaults-file={}",
            my_cnf_path(&ctx.run_dir).display()
        ));
        cmd.stdin(Stdio::null());
        strip_password_env(&mut cmd);
        cmd.env("TMPDIR", tmpdir_path(&ctx.run_dir));
        cmd
    }

    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        match read_handshake(ctx.port.get(), Duration::from_secs(5)) {
            Ok(Handshake::Ready) => Ok(Health::Ready),
            Ok(Handshake::NotReady(reason)) => Ok(Health::NotReady(reason)),
            Err(err) => Err(err),
        }
    }

    fn shutdown(&self, ctx: &RunContext) -> Result<()> {
        let out = shutdown_command(ctx)
            .output()
            .map_err(|e| DblError::Engine {
                reason: format!("failed to execute mysqladmin shutdown: {e}"),
            })?;
        let result = if out.status.success() {
            Ok(())
        } else {
            let tail = combined_output(out.stdout, out.stderr);
            Err(DblError::Engine {
                reason: format!(
                    "mysqladmin shutdown failed (code {:?}): {}",
                    out.status.code(),
                    tail.trim_end()
                ),
            })
        };
        cleanup_fallback_socket_from_my_cnf(
            &my_cnf_path(&ctx.run_dir),
            &ctx.run_dir,
            ctx.port.get(),
        );
        result
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString {
        uri::render(EngineKey::Mysql, ctx.port.get(), credential, format)
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        diagnose_failure_text(output)
    }
}

#[derive(Debug, Clone, Copy)]
struct MyCnfParams<'a> {
    port: u16,
    data_dir: &'a Path,
    run_dir: &'a Path,
    logs_dir: &'a Path,
    runtime_dir: &'a Path,
    socket: &'a Path,
}

fn render_my_cnf(params: &MyCnfParams<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "[mysqld]");
    let _ = writeln!(out, "bind-address={BIND}");
    let _ = writeln!(out, "port={}", params.port);
    let _ = writeln!(out, "datadir={}", params.data_dir.display());
    let _ = writeln!(out, "socket={}", params.socket.display());
    let _ = writeln!(out, "pid-file={}", pidfile_path(params.run_dir).display());
    let _ = writeln!(out, "log-error={}", log_path(params.logs_dir).display());
    let _ = writeln!(out, "tmpdir={}", tmpdir_path(params.run_dir).display());
    let _ = writeln!(out, "mysqlx=OFF");
    let _ = writeln!(out, "skip-name-resolve");
    let _ = writeln!(out, "basedir={}", params.runtime_dir.display());
    out
}

fn write_my_cnf(ctx: &InitContext, run_dir: &Path, logs_dir: &Path, socket: &Path) -> Result<()> {
    let params = MyCnfParams {
        port: ctx.port.get(),
        data_dir: &ctx.data_dir,
        run_dir,
        logs_dir,
        runtime_dir: &ctx.runtime_dir,
        socket,
    };
    let rendered = render_my_cnf(&params);
    write_private_config_atomic(&my_cnf_path(run_dir), &rendered)
}

fn run_initialize_insecure(ctx: &InitContext, logs_dir: &Path) -> Result<()> {
    let init_err = logs_dir.join("init.err");
    let out = initialize_insecure_command(ctx, &init_err)
        .output()
        .map_err(|e| DblError::Engine {
            reason: format!("failed to execute mysqld --initialize-insecure: {e}"),
        })?;
    if out.status.success() {
        return Ok(());
    }
    let tail = combined_output(out.stdout, out.stderr);
    let log_tail = fs::read_to_string(&init_err).unwrap_or_default();
    let detail = if log_tail.trim().is_empty() {
        tail
    } else {
        format!("{tail}\n{log_tail}")
    };
    Err(DblError::Engine {
        reason: format!(
            "mysqld --initialize-insecure failed (code {:?}): {}",
            out.status.code(),
            detail.trim_end()
        ),
    })
}

pub(super) fn initialize_insecure_command(ctx: &InitContext, init_err: &Path) -> Command {
    let mysqld = ctx.runtime_dir.join("bin").join(exe_name("mysqld"));
    let mut cmd = Command::new(mysqld);
    cmd.arg("--no-defaults")
        .arg("--initialize-insecure")
        .arg(format!("--datadir={}", ctx.data_dir.display()))
        .arg(format!("--basedir={}", ctx.runtime_dir.display()))
        .arg(format!("--log-error={}", init_err.display()));
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    strip_password_env(&mut cmd);
    cmd.env(
        "TMPDIR",
        sibling_dir(&ctx.data_dir, consts::DIR_RUN).join("tmp"),
    );
    cmd
}

fn bootstrap_credentials(ctx: &InitContext, run_dir: &Path, logs_dir: &Path) -> Result<()> {
    let init_sql = render_init_sql(ctx)?;
    let init_path = init_file_path(run_dir);
    crate::fsutil::write_private_file_atomic(init_path.as_path(), init_sql.expose().as_bytes())
        .map_err(|e| DblError::Engine {
            reason: format!("cannot write init SQL {}: {e}", init_path.display()),
        })?;
    let cfg = my_cnf_path(run_dir);
    let mut child = bootstrap_mysqld_command(ctx, &cfg, &init_path)
        .spawn()
        .map_err(|e| DblError::Engine {
            reason: format!("failed to start mysqld for credential bootstrap: {e}"),
        })?;
    let port = ctx.port.get();
    let deadline = std::time::Instant::now() + Duration::from_secs(consts::START_TIMEOUT_SECS);
    let ready = loop {
        if let Some(status) = child.try_wait().map_err(|e| DblError::Engine {
            reason: format!("failed waiting for mysqld bootstrap: {e}"),
        })? {
            let out = child.wait_with_output().map_err(|e| DblError::Engine {
                reason: format!("failed reading mysqld bootstrap output: {e}"),
            })?;
            let tail = combined_output(out.stdout, out.stderr);
            let log_tail = read_log_tail(&log_path(logs_dir), 40);
            return Err(DblError::Engine {
                reason: format!(
                    "mysqld bootstrap exited early (status {status}): {}\n{}",
                    tail.trim_end(),
                    log_tail
                ),
            });
        }
        if tcp_connects(port) {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(consts::HEALTH_CHECK_POLL_MS));
    };
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        let log_tail = read_log_tail(&log_path(logs_dir), 40);
        return Err(DblError::Engine {
            reason: format!("mysqld bootstrap timed out on port {port}\n{log_tail}"),
        });
    }
    let shutdown_res = shutdown_bootstrap(
        &ctx.runtime_dir,
        ctx.port.get(),
        &ctx.superuser,
        ctx.auth,
        ctx.password.as_ref(),
    );
    let wait_deadline = std::time::Instant::now() + Duration::from_secs(consts::STOP_GRACE_SECS);
    loop {
        if child
            .try_wait()
            .map_err(|e| DblError::Engine {
                reason: format!("failed waiting for bootstrap mysqld exit: {e}"),
            })?
            .is_some()
        {
            break;
        }
        if std::time::Instant::now() >= wait_deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&init_path);
            return Err(DblError::Engine {
                reason: "mysqld bootstrap did not exit after shutdown".to_owned(),
            });
        }
        std::thread::sleep(Duration::from_millis(consts::HEALTH_CHECK_POLL_MS));
    }
    cleanup_init_file(&init_path);
    shutdown_res
}

pub(super) fn bootstrap_mysqld_command(ctx: &InitContext, cfg: &Path, init_path: &Path) -> Command {
    let mut cmd = Command::new(ctx.runtime_dir.join("bin").join(exe_name("mysqld")));
    cmd.arg(format!("--defaults-file={}", cfg.display()))
        .arg(format!("--init-file={}", init_path.display()))
        .arg("--daemonize=OFF");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    strip_password_env(&mut cmd);
    cmd.env(
        "TMPDIR",
        sibling_dir(&ctx.data_dir, consts::DIR_RUN).join("tmp"),
    );
    cmd
}

fn shutdown_bootstrap(
    runtime_dir: &Path,
    port: u16,
    superuser: &str,
    auth: AuthMode,
    password: Option<&SecretString>,
) -> Result<()> {
    let out = bootstrap_shutdown_command(runtime_dir, port, superuser, auth, password)
        .output()
        .map_err(|e| DblError::Engine {
            reason: format!("failed to shutdown bootstrap mysqld: {e}"),
        })?;
    if out.status.success() {
        return Ok(());
    }
    let tail = combined_output(out.stdout, out.stderr);
    Err(DblError::Engine {
        reason: format!(
            "bootstrap shutdown failed (code {:?}): {}",
            out.status.code(),
            tail.trim_end()
        ),
    })
}

pub(super) fn shutdown_command(ctx: &RunContext) -> Command {
    mysqladmin_command(
        &ctx.runtime_dir,
        ctx.port.get(),
        &ctx.superuser,
        ctx.auth,
        ctx.credential.as_ref().map(|c| &c.password),
        "shutdown",
    )
}

pub(super) fn bootstrap_shutdown_command(
    runtime_dir: &Path,
    port: u16,
    superuser: &str,
    auth: AuthMode,
    password: Option<&SecretString>,
) -> Command {
    mysqladmin_command(runtime_dir, port, superuser, auth, password, "shutdown")
}

fn mysqladmin_command(
    runtime_dir: &Path,
    port: u16,
    superuser: &str,
    auth: AuthMode,
    password: Option<&SecretString>,
    subcommand: &str,
) -> Command {
    let mysqladmin = runtime_dir.join("bin").join(exe_name("mysqladmin"));
    let mut cmd = Command::new(mysqladmin);
    cmd.arg("--no-defaults")
        .arg("-h")
        .arg(BIND)
        .arg("-P")
        .arg(port.to_string())
        .arg("-u")
        .arg(superuser)
        .arg("--protocol=TCP")
        .arg(subcommand);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if auth == AuthMode::Password
        && let Some(pw) = password
    {
        cmd.env("MYSQL_PWD", pw.expose());
    }
    strip_password_env_keep_mysql_pwd(&mut cmd);
    cmd
}

pub(super) fn render_init_sql(ctx: &InitContext) -> Result<SecretString> {
    let superuser = effective_superuser(&ctx.superuser)?;
    let mut out = String::new();
    match ctx.auth {
        AuthMode::None => {
            // Keep the empty-password, localhost-only account from --initialize-insecure.
            // Add a TCP-loopback account with no password for F-CRD-03.
            let _ = writeln!(
                out,
                "CREATE USER IF NOT EXISTS '{superuser}'@'{BIND}' IDENTIFIED BY '';"
            );
            let _ = writeln!(
                out,
                "GRANT ALL PRIVILEGES ON *.* TO '{superuser}'@'{BIND}' WITH GRANT OPTION;"
            );
        }
        AuthMode::Password => {
            let pw = ctx
                .password
                .as_ref()
                .ok_or_else(|| DblError::InvalidInput {
                    field: "password",
                    reason: "password auth requires a generated password".to_owned(),
                })?;
            let escaped = escape_sql_single_quoted(pw.expose());
            let _ = writeln!(
                out,
                "ALTER USER '{superuser}'@'localhost' IDENTIFIED BY '{escaped}';"
            );
            let _ = writeln!(
                out,
                "CREATE USER IF NOT EXISTS '{superuser}'@'{BIND}' IDENTIFIED BY '{escaped}';"
            );
            let _ = writeln!(
                out,
                "GRANT ALL PRIVILEGES ON *.* TO '{superuser}'@'{BIND}' WITH GRANT OPTION;"
            );
        }
    }
    let _ = writeln!(out, "FLUSH PRIVILEGES;");
    Ok(SecretString::new(out))
}

pub(super) fn effective_superuser(superuser: &str) -> Result<&str> {
    let name = if superuser.is_empty() {
        consts::SUPERUSER_MYSQL
    } else {
        superuser
    };
    if !is_safe_sql_identifier(name) {
        return Err(DblError::InvalidInput {
            field: "superuser",
            reason: format!(
                "superuser must start with ASCII letter or underscore and contain only ASCII letters, digits, or underscores: {name:?}"
            ),
        });
    }
    Ok(name)
}

pub(super) fn is_safe_sql_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn escape_sql_single_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            '\0' => out.push_str("\\0"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

fn init_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(INIT_FILE_NAME)
}

pub(super) fn cleanup_init_file(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn my_cnf_path(run_dir: &Path) -> PathBuf {
    run_dir.join("my.cnf")
}

pub fn log_path(logs_dir: &Path) -> PathBuf {
    logs_dir.join("error.log")
}

pub fn pidfile_path(run_dir: &Path) -> PathBuf {
    run_dir.join("mysqld.pid")
}

pub fn tmpdir_path(run_dir: &Path) -> PathBuf {
    run_dir.join("tmp")
}

mod socket;

pub(in crate::engine::mysql) use socket::{
    cleanup_fallback_socket, cleanup_fallback_socket_from_my_cnf,
};
#[cfg(test)]
pub(in crate::engine::mysql) use socket::{
    cleanup_fallback_socket_with_candidates, ensure_fallback_socket_dir_for,
    ensure_verified_private_dir_for_test, fallback_socket_dir_candidates,
    is_fallback_socket_name_for_port, is_safe_fallback_socket_for_port_with_candidates,
    resolve_socket_path_for, resolve_socket_path_with_fallback_dir, socket_path_from_my_cnf,
    verify_secure_dir_for_test,
};
pub use socket::{resolve_socket_path, socket_path, socket_path_limit, validate_socket_path_len};

pub fn sibling_dir(data_dir: &Path, leaf: &str) -> PathBuf {
    match data_dir.parent() {
        Some(parent) => parent.join(leaf),
        None => data_dir.join(leaf),
    }
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

fn strip_password_env(cmd: &mut Command) {
    cmd.env_remove("MYSQL_PWD");
    cmd.env_remove("MYSQL_TCP_PORT");
}

fn strip_password_env_keep_mysql_pwd(cmd: &mut Command) {
    cmd.env_remove("MYSQL_TCP_PORT");
}

fn combined_output(stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    let mut out = String::from_utf8_lossy(&stdout).into_owned();
    let err = String::from_utf8_lossy(&stderr);
    if !err.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&err);
    }
    out
}

fn tcp_connects(port: u16) -> bool {
    let Ok(addr) = format!("{BIND}:{port}").parse() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handshake {
    Ready,
    NotReady(String),
}

/// Reads the first MySQL server packet (greeting or ERR) over TCP loopback.
pub fn read_handshake(port: u16, timeout: Duration) -> Result<Handshake> {
    let addr = format!("{BIND}:{port}");
    let socket_addr = addr.parse().map_err(|e| DblError::Engine {
        reason: format!("invalid mysql address {addr}: {e}"),
    })?;
    let mut stream =
        TcpStream::connect_timeout(&socket_addr, timeout).map_err(|e| DblError::Engine {
            reason: format!("cannot connect to mysql on {addr}: {e}"),
        })?;
    let _ = stream.set_read_timeout(Some(timeout));
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).map_err(|e| DblError::Engine {
        reason: format!("mysql handshake header read failed: {e}"),
    })?;
    let payload_len = (hdr[0] as usize) | ((hdr[1] as usize) << 8) | ((hdr[2] as usize) << 16);
    if payload_len == 0 || payload_len > 16 * 1024 * 1024 {
        return Ok(Handshake::NotReady(format!(
            "invalid mysql packet length {payload_len}"
        )));
    }
    let mut payload = vec![0u8; payload_len];
    stream
        .read_exact(&mut payload)
        .map_err(|e| DblError::Engine {
            reason: format!("mysql handshake payload read failed: {e}"),
        })?;
    let _ = stream.shutdown(Shutdown::Both);
    parse_handshake_payload(&payload)
}

/// Parses a raw MySQL handshake payload (without the 4-byte packet header).
pub fn parse_handshake_payload(payload: &[u8]) -> Result<Handshake> {
    if payload.is_empty() {
        return Ok(Handshake::NotReady("empty handshake payload".to_owned()));
    }
    match payload[0] {
        10 => Ok(Handshake::Ready), // protocol v10 greeting
        0xFF => {
            // ERR packet: 0xff, 2-byte error code, optional sqlstate marker+state, message
            let code = if payload.len() >= 3 {
                u16::from_le_bytes([payload[1], payload[2]])
            } else {
                0
            };
            let msg_start = if payload.len() > 9 && payload[3] == b'#' {
                9
            } else {
                3
            };
            let msg = String::from_utf8_lossy(&payload[msg_start.min(payload.len())..]).to_string();
            if is_auth_challenge_err(code, &msg) {
                Ok(Handshake::Ready)
            } else {
                Ok(Handshake::NotReady(format!("mysql ERR {code}: {msg}")))
            }
        }
        other => Ok(Handshake::NotReady(format!(
            "unexpected mysql handshake byte 0x{other:02x}"
        ))),
    }
}

fn is_auth_challenge_err(code: u16, msg: &str) -> bool {
    // 1045 ER_ACCESS_DENIED_ERROR, 1130 ER_HOST_NOT_PRIVILEGED,
    // 1251 ER_NOT_SUPPORTED_AUTH_MODE, 1524 ER_PLUGIN_IS_NOT_LOADED
    matches!(code, 1045 | 1130 | 1251 | 1524)
        || msg.contains("Access denied")
        || msg.contains("not allowed to connect")
}

pub fn diagnose_failure_text(output: &str) -> StartFailure {
    let lower = output.to_ascii_lowercase();

    if output.contains("Bind on TCP/IP port: Address already in use")
        || output.contains("Do you already have another mysqld server running on port")
        || output.contains("Address already in use")
            && (output.contains("Can't start server") || lower.contains("bind"))
    {
        return StartFailure::PortInUse;
    }

    if lower.contains("permission denied") || output.contains("OS errno 13") {
        return StartFailure::PermissionDenied;
    }

    if output.contains("The socket file path is too long")
        || (output.contains("socket file path is too long"))
    {
        return StartFailure::Unknown;
    }

    if output.contains("Unknown suffix")
        || output.contains("Error while setting value")
        || output.contains("unknown variable")
        || output.contains("Unknown system variable")
        || output.contains("Bad directive")
        || output.contains("FATAL CONFIG FILE ERROR")
    {
        return StartFailure::Unknown;
    }

    // Wrong/missing/garbled data dir: keep raw tail (no honest StartFailure).
    if output.contains("ibdata1")
        || output.contains("InnoDB")
            && (output.contains("Cannot continue operation")
                || output.contains("Plugin initialization aborted")
                || output.contains("different size"))
        || output.contains("Data Dictionary initialization failed")
        || output.contains("Failed to initialize DD Storage Engine")
    {
        return StartFailure::Unknown;
    }

    if is_binary_missing(output) {
        return StartFailure::BinaryMissing;
    }

    StartFailure::Unknown
}

fn is_binary_missing(output: &str) -> bool {
    if output.contains("Exec format error") {
        return true;
    }
    if output.contains("No such file or directory") {
        if output.contains(".sock") || output.contains("Can't create test file") {
            return false;
        }
        return output.contains("mysqld")
            || output.contains("mysqladmin")
            || output.contains("failed to execute");
    }
    false
}

#[cfg(test)]
mod tests;
