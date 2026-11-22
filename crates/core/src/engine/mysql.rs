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
/// Maximum length of a Unix socket path on Linux (`sun_path`, 107 bytes).
const UNIX_SOCKET_PATH_LIMIT: usize = 107;
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
        for (dir, label) in [
            (&ctx.data_dir, "data"),
            (&run_dir, "run"),
            (&logs_dir, "logs"),
        ] {
            ensure_private_dir(dir, label)?;
        }
        validate_socket_path_len(&socket_path(&run_dir, ctx.port.get()))?;

        let outcome = (|| -> Result<()> {
            run_initialize_insecure(ctx, &logs_dir)?;
            write_my_cnf(ctx, &run_dir, &logs_dir)?;
            bootstrap_credentials(ctx, &run_dir, &logs_dir)?;
            Ok(())
        })();

        if let Err(err) = outcome {
            let _ = fs::remove_dir_all(&ctx.data_dir);
            let _ = fs::remove_file(init_file_path(&run_dir));
            return Err(err);
        }
        let _ = fs::remove_file(init_file_path(&run_dir));
        tracing::debug!(instance = %ctx.instance_name, "mysql initialized");
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
        let mysqladmin = ctx.runtime_dir.join("bin").join(exe_name("mysqladmin"));
        let mut cmd = Command::new(mysqladmin);
        cmd.arg("--no-defaults")
            .arg("-h")
            .arg(BIND)
            .arg("-P")
            .arg(ctx.port.get().to_string())
            .arg("-u")
            .arg(&ctx.superuser)
            .arg("--protocol=TCP")
            .arg("shutdown");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        if ctx.auth == AuthMode::Password
            && let Some(cred) = ctx.credential.as_ref()
        {
            cmd.env("MYSQL_PWD", cred.password.expose());
        }
        strip_password_env_keep_mysql_pwd(&mut cmd);
        let out = cmd.output().map_err(|e| DblError::Engine {
            reason: format!("failed to execute mysqladmin shutdown: {e}"),
        })?;
        if out.status.success() {
            return Ok(());
        }
        let tail = combined_output(out.stdout, out.stderr);
        Err(DblError::Engine {
            reason: format!(
                "mysqladmin shutdown failed (code {:?}): {}",
                out.status.code(),
                tail.trim_end()
            ),
        })
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
}

fn render_my_cnf(params: &MyCnfParams<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "[mysqld]");
    let _ = writeln!(out, "bind-address={BIND}");
    let _ = writeln!(out, "port={}", params.port);
    let _ = writeln!(out, "datadir={}", params.data_dir.display());
    let _ = writeln!(
        out,
        "socket={}",
        socket_path(params.run_dir, params.port).display()
    );
    let _ = writeln!(out, "pid-file={}", pidfile_path(params.run_dir).display());
    let _ = writeln!(out, "log-error={}", log_path(params.logs_dir).display());
    let _ = writeln!(out, "mysqlx=OFF");
    let _ = writeln!(out, "skip-name-resolve");
    let _ = writeln!(out, "basedir={}", params.runtime_dir.display());
    out
}

fn write_my_cnf(ctx: &InitContext, run_dir: &Path, logs_dir: &Path) -> Result<()> {
    let params = MyCnfParams {
        port: ctx.port.get(),
        data_dir: &ctx.data_dir,
        run_dir,
        logs_dir,
        runtime_dir: &ctx.runtime_dir,
    };
    let rendered = render_my_cnf(&params);
    write_private_config_atomic(&my_cnf_path(run_dir), &rendered)
}

fn run_initialize_insecure(ctx: &InitContext, logs_dir: &Path) -> Result<()> {
    let mysqld = ctx.runtime_dir.join("bin").join(exe_name("mysqld"));
    let init_err = logs_dir.join("init.err");
    let mut cmd = Command::new(mysqld);
    cmd.arg("--no-defaults")
        .arg("--initialize-insecure")
        .arg(format!("--datadir={}", ctx.data_dir.display()))
        .arg(format!("--basedir={}", ctx.runtime_dir.display()))
        .arg(format!("--log-error={}", init_err.display()));
    strip_password_env(&mut cmd);
    let out = cmd.output().map_err(|e| DblError::Engine {
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

fn bootstrap_credentials(ctx: &InitContext, run_dir: &Path, logs_dir: &Path) -> Result<()> {
    let init_sql = render_init_sql(ctx)?;
    let init_path = init_file_path(run_dir);
    crate::fsutil::write_private_file_atomic(init_path.as_path(), init_sql.expose().as_bytes())
        .map_err(|e| DblError::Engine {
            reason: format!("cannot write init SQL {}: {e}", init_path.display()),
        })?;
    let cfg = my_cnf_path(run_dir);
    let mut cmd = Command::new(ctx.runtime_dir.join("bin").join(exe_name("mysqld")));
    cmd.arg(format!("--defaults-file={}", cfg.display()))
        .arg(format!("--init-file={}", init_path.display()))
        .arg("--daemonize=OFF");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    strip_password_env(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| DblError::Engine {
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
            reason: format!("mysqld bootstrap timed out on port {port}\n{}", log_tail),
        });
    }
    let shutdown_res = shutdown_bootstrap(
        &ctx.runtime_dir,
        ctx.port.get(),
        &ctx.superuser,
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

fn shutdown_bootstrap(
    runtime_dir: &Path,
    port: u16,
    superuser: &str,
    password: Option<&SecretString>,
) -> Result<()> {
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
        .arg("shutdown");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if let Some(pw) = password {
        cmd.env("MYSQL_PWD", pw.expose());
    }
    strip_password_env_keep_mysql_pwd(&mut cmd);
    let out = cmd.output().map_err(|e| DblError::Engine {
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

fn render_init_sql(ctx: &InitContext) -> Result<SecretString> {
    let superuser = effective_superuser(&ctx.superuser);
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

fn effective_superuser(superuser: &str) -> &str {
    if superuser.is_empty() {
        consts::SUPERUSER_MYSQL
    } else {
        superuser
    }
}

fn escape_sql_single_quoted(value: &str) -> String {
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

/// Renders `my.cnf` content for tests and diagnostics.
pub fn my_cnf_path(run_dir: &Path) -> PathBuf {
    run_dir.join("my.cnf")
}

pub fn log_path(logs_dir: &Path) -> PathBuf {
    logs_dir.join("error.log")
}

pub fn pidfile_path(run_dir: &Path) -> PathBuf {
    run_dir.join("mysqld.pid")
}

pub fn socket_path(run_dir: &Path, port: u16) -> PathBuf {
    run_dir.join(format!("mysqld-{port}.sock"))
}

fn init_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(INIT_FILE_NAME)
}

fn cleanup_init_file(path: &Path) {
    let _ = fs::remove_file(path);
}

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

pub fn validate_socket_path_len(sock: &Path) -> Result<()> {
    let len = sock.as_os_str().as_encoded_bytes().len();
    if len > UNIX_SOCKET_PATH_LIMIT {
        return Err(DblError::InvalidInput {
            field: "socket",
            reason: format!(
                "mysql socket path {} is {} bytes, maximum is {}",
                sock.display(),
                len,
                UNIX_SOCKET_PATH_LIMIT
            ),
        });
    }
    Ok(())
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
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::engine::Credential;
    use crate::model::Port;

    const SECRET: &str = "S3cret-Canary-Pw-MySql!9x";

    fn init_ctx(port: u16, auth: AuthMode, password: Option<&str>) -> InitContext {
        InitContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/mysql"),
            data_dir: PathBuf::from("/data"),
            port: Port::new(port).expect("port"),
            superuser: "root".into(),
            auth,
            password: password.map(SecretString::from),
        }
    }

    fn run_ctx(port: u16, auth: AuthMode, password: Option<&str>) -> RunContext {
        RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/mysql"),
            data_dir: PathBuf::from("/data"),
            logs_dir: PathBuf::from("/logs"),
            run_dir: PathBuf::from("/run"),
            port: Port::new(port).expect("port"),
            superuser: "root".into(),
            auth,
            credential: password.map(|p| Credential::new("root", SecretString::from(p))),
        }
    }

    fn assert_no_secret_in_command(cmd: &Command, secret: &str) {
        let prog = cmd.get_program().to_string_lossy();
        assert!(!prog.contains(secret), "secret in program: {prog}");
        for arg in cmd.get_args() {
            let a = arg.to_string_lossy();
            assert!(!a.contains(secret), "secret in arg: {a}");
        }
        for (k, v) in cmd.get_envs() {
            let k = k.to_string_lossy();
            assert!(!k.contains(secret), "secret in env key: {k}");
            if let Some(v) = v {
                let v = v.to_string_lossy();
                assert!(!v.contains(secret), "secret in env value {k}={v}");
            }
        }
    }

    #[test]
    fn render_my_cnf_contains_required_security_settings() {
        let cnf = render_my_cnf(&MyCnfParams {
            port: 42345,
            data_dir: Path::new("/i/data"),
            run_dir: Path::new("/i/run"),
            logs_dir: Path::new("/i/logs"),
            runtime_dir: Path::new("/rt/mysql"),
        });
        assert!(cnf.contains("bind-address=127.0.0.1\n"));
        assert!(cnf.contains("port=42345\n"));
        assert!(cnf.contains("datadir=/i/data\n"));
        assert!(cnf.contains("socket=/i/run/mysqld-42345.sock\n"));
        assert!(cnf.contains("pid-file=/i/run/mysqld.pid\n"));
        assert!(cnf.contains("log-error=/i/logs/error.log\n"));
        assert!(cnf.contains("mysqlx=OFF\n"));
        assert!(cnf.contains("skip-name-resolve\n"));
        assert!(cnf.contains("basedir=/rt/mysql\n"));
        assert!(!cnf.contains("0.0.0.0"));
    }

    #[test]
    fn spawn_command_uses_defaults_file_and_no_password() {
        let ctx = run_ctx(42345, AuthMode::Password, Some(SECRET));
        let cmd = MysqlAdapter.spawn_command(&ctx);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 1);
        assert!(args[0].starts_with("--defaults-file="));
        assert!(args[0].contains("/run/my.cnf"));
        assert!(cmd.get_program().to_string_lossy().ends_with("bin/mysqld"));
        assert_no_secret_in_command(&cmd, SECRET);
    }

    #[test]
    fn connection_uri_password_and_no_password_shapes() {
        let ctx_pw = run_ctx(3306, AuthMode::Password, Some("p@:/?#[]% x"));
        let cred = Credential::new("root", SecretString::from("p@:/?#[]% x"));
        let uri = MysqlAdapter.connection_uri(&ctx_pw, &cred, UriFormat::Uri);
        assert!(uri.expose().starts_with("mysql://root:"));
        assert!(uri.expose().ends_with("@127.0.0.1:3306/"));
        assert!(uri.expose().contains("%40"));
        assert!(uri.expose().contains("%20"));
        let env = MysqlAdapter.connection_uri(&ctx_pw, &cred, UriFormat::DotEnv);
        assert!(env.expose().contains("DB_PASSWORD=p@:/?#[]% x\n"));
        let ctx_none = run_ctx(3306, AuthMode::None, None);
        let no_pw = MysqlAdapter.connection_uri_no_password(&ctx_none, UriFormat::Uri);
        assert_eq!(no_pw.expose(), "mysql://root@127.0.0.1:3306/");
        let no_env = MysqlAdapter.connection_uri_no_password(&ctx_none, UriFormat::DotEnv);
        assert!(!no_env.expose().contains("DB_PASSWORD"));
    }

    #[test]
    fn init_sql_password_mode_sets_both_hosts_and_flushes() {
        let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
        let s = render_init_sql(&ctx).expect("sql").expose().to_owned();
        assert!(
            s.contains("ALTER USER 'root'@'localhost' IDENTIFIED BY 'S3cret-Canary-Pw-MySql!9x';")
        );
        assert!(s.contains("CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY 'S3cret-Canary-Pw-MySql!9x';"));
        assert!(s.contains("GRANT ALL PRIVILEGES ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;"));
        assert!(s.contains("FLUSH PRIVILEGES;"));
    }

    #[test]
    fn init_sql_no_password_mode_adds_loopback_user() {
        let ctx = init_ctx(42345, AuthMode::None, None);
        let s = render_init_sql(&ctx).expect("sql").expose().to_owned();
        assert!(s.contains("CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '';"));
        assert!(!s.contains("ALTER USER 'root'@'localhost' IDENTIFIED BY"));
        assert!(s.contains("FLUSH PRIVILEGES;"));
    }

    #[test]
    fn init_sql_escapes_single_quote_and_backslash() {
        let ctx = init_ctx(42345, AuthMode::Password, Some("p'w\\rd"));
        let s = render_init_sql(&ctx).expect("sql").expose().to_owned();
        assert!(s.contains("IDENTIFIED BY 'p''w\\\\rd';"));
    }

    #[test]
    fn init_sql_debug_redacted() {
        let ctx = init_ctx(42345, AuthMode::Password, Some(SECRET));
        let sql = render_init_sql(&ctx).expect("sql");
        assert!(!format!("{sql:?}").contains(SECRET));
        assert!(!format!("{sql}").contains(SECRET));
    }

    #[test]
    fn validate_socket_path_len_rejects_long_path() {
        let long = PathBuf::from(format!("/tmp/{}/mysqld-3306.sock", "x".repeat(120)));
        let err = validate_socket_path_len(&long).unwrap_err();
        assert!(matches!(
            err,
            DblError::InvalidInput {
                field: "socket",
                ..
            }
        ));
    }

    #[test]
    fn parse_handshake_protocol_10_ready() {
        let mut payload = vec![10u8];
        payload.extend_from_slice(b"8.4.11\0");
        payload.extend_from_slice(&[0u8; 60]);
        assert_eq!(parse_handshake_payload(&payload).unwrap(), Handshake::Ready);
    }

    #[test]
    fn parse_handshake_real_greeting_packet_fixture() {
        // Captured from mysqld 8.4.11 minimal, 127.0.0.1:41108, 2026-11-22.
        // Full packet: 4-byte header + 74-byte payload (protocol 10, 8.4.11, caching_sha2_password).
        let pkt = hex_to_bytes(
            "4a0000000a382e342e3131000a0000002b452a0f663f0b7e00ffffff0200ffdf15000000000000000000002f793f2347727f7a046f1b720063616368696e675f736861325f70617373776f726400",
        );
        let payload = &pkt[4..];
        assert_eq!(payload[0], 10);
        assert_eq!(parse_handshake_payload(payload).unwrap(), Handshake::Ready);
    }

    #[test]
    fn parse_handshake_real_err_1130_host_not_allowed_is_ready() {
        // Captured from mysqld 8.4.11 minimal before root@127.0.0.1 existed.
        let pkt = hex_to_bytes(
            "42000000ff6a04486f737420273132372e302e302e3127206973206e6f7420616c6c6f77656420746f20636f6e6e65637420746f2074686973204d7953514c20736572766572",
        );
        let payload = &pkt[4..];
        assert_eq!(payload[0], 0xFF);
        assert_eq!(parse_handshake_payload(payload).unwrap(), Handshake::Ready);
    }

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect()
    }

    #[test]
    fn parse_handshake_auth_challenges_are_ready() {
        let mut p1045 = vec![0xFF, 0x15, 0x04, b'#'];
        p1045.extend_from_slice(b"28000Access denied for user 'root'@'127.0.0.1'");
        assert_eq!(parse_handshake_payload(&p1045).unwrap(), Handshake::Ready);
        let mut p1130 = vec![0xFF, 0x6A, 0x04, b'#'];
        p1130.extend_from_slice(
            b"HY000Host '127.0.0.1' is not allowed to connect to this MySQL server",
        );
        assert_eq!(parse_handshake_payload(&p1130).unwrap(), Handshake::Ready);
    }

    #[test]
    fn parse_handshake_other_err_empty_unknown_no_panic() {
        let mut other = vec![0xFF, 0x10, 0x04, b'#'];
        other.extend_from_slice(b"HY000Too many connections");
        assert!(matches!(
            parse_handshake_payload(&other).unwrap(),
            Handshake::NotReady(_)
        ));
        assert!(matches!(
            parse_handshake_payload(&[]).unwrap(),
            Handshake::NotReady(_)
        ));
        assert!(matches!(
            parse_handshake_payload(&[0xFE, 0, 0]).unwrap(),
            Handshake::NotReady(_)
        ));
        assert!(matches!(
            parse_handshake_payload(&[0xFF]).unwrap(),
            Handshake::NotReady(_) | Handshake::Ready
        ));
        let big = vec![10u8; 1024 * 1024];
        assert_eq!(parse_handshake_payload(&big).unwrap(), Handshake::Ready);
    }

    fn diag_cases() -> Vec<(&'static str, StartFailure)> {
        vec![
            (
                "2026-11-21T23:56:30.686109Z 0 [ERROR] [MY-010262] [Server] Can't start server: Bind on TCP/IP port: Address already in use\n2026-11-21T23:56:30.686124Z 0 [ERROR] [MY-010257] [Server] Do you already have another mysqld server running on port: 41102 ?\n",
                StartFailure::PortInUse,
            ),
            (
                "Can't create test file /tmp/x/ro/mysqld_tmp_file_case_insensitive_test.lower-test\nmysqld: File './binlog.index' not found (OS errno 13 - Permission denied)\n",
                StartFailure::PermissionDenied,
            ),
            (
                "2026-11-21T23:56:34.517296Z 0 [ERROR] [MY-010267] [Server] The socket file path is too long (> 107): /tmp/very/long/path/mysqld.sock\n",
                StartFailure::Unknown,
            ),
            (
                "2026-11-21T23:56:33.765723Z 0 [ERROR] [MY-000058] [Server] Unknown suffix 'n' used for variable 'port' (value 'not-a-number').\n",
                StartFailure::Unknown,
            ),
            (
                "2026-11-21T23:56:32.516055Z 1 [ERROR] [MY-012646] [InnoDB] File ./ibdata1: 'open' returned OS error 71. Cannot continue operation\n2026-11-21T23:56:32.516068Z 1 [ERROR] [MY-012981] [InnoDB] Cannot continue operation.\n",
                StartFailure::Unknown,
            ),
            (
                "2026-11-21T23:56:32.767385Z 1 [ERROR] [MY-012263] [InnoDB] The Auto-extending innodb_system data file './ibdata1' is of a different size 0 pages\n2026-11-21T23:56:33.268000Z 0 [ERROR] [MY-010020] [Server] Data Dictionary initialization failed.\n",
                StartFailure::Unknown,
            ),
            (
                "mysqld: error while loading shared libraries: libfoo.so.1: cannot open shared object file: No such file or directory\n",
                StartFailure::BinaryMissing,
            ),
            (
                "bash: /opt/mysql/bin/mysqld: No such file or directory\n",
                StartFailure::BinaryMissing,
            ),
        ]
    }

    #[test]
    fn cleanup_init_file_removes_one_shot_sql() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("dblocal-init.sql");
        fs::write(&p, "ALTER USER root@localhost IDENTIFIED BY 'x';").expect("write");
        assert!(p.exists());
        cleanup_init_file(&p);
        assert!(!p.exists(), "init file must be removed");
    }
    #[test]
    fn diagnose_real_spike_texts() {
        for (text, expected) in diag_cases() {
            assert_eq!(diagnose_failure_text(text), expected, "text: {text:?}");
        }
    }

    #[test]
    fn diagnose_unknown_for_unrelated_text() {
        assert_eq!(
            diagnose_failure_text("2026-11-21T00:00:00Z 0 [Note] [MY-000000] [Server] some note\n"),
            StartFailure::Unknown
        );
    }

    #[test]
    fn init_sql_requires_password_when_auth_password() {
        let ctx = init_ctx(3306, AuthMode::Password, None);
        let err = render_init_sql(&ctx).unwrap_err();
        assert!(matches!(
            err,
            DblError::InvalidInput {
                field: "password",
                ..
            }
        ));
    }
}
