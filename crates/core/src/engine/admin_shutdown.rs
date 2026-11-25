//! Shared bounded graceful shutdown for MySQL and MariaDB admin clients.
//!
//! Invokes `mysqladmin` or `mariadb-admin` with a private temporary
//! `--defaults-file` (never passing credentials in argv or env, and ignoring
//! any host `my.cnf`/`my.ini`). Both `--connect-timeout` and a wall-clock
//! process timeout bound the call so a hung server never blocks escalation.
//! Temporary files are removed on drop regardless of success, failure, or
//! timeout.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::error::{DblError, Result};
use crate::model::AuthMode;
use crate::secret::SecretString;

const CONNECT_TIMEOUT_MAX_SECS: u64 = 5;
const POLL_INTERVAL_MS: u64 = 50;
const OUTPUT_TAIL_MAX_BYTES: usize = 2048;

/// Parameters for a bounded `mysqladmin` / `mariadb-admin` shutdown call.
pub(crate) struct AdminShutdownRequest<'a> {
    pub engine_label: &'a str,
    pub tool_label: &'a str,
    pub file_prefix: &'a str,
    pub instance_name: &'a str,
    pub admin_bin: &'a Path,
    pub run_dir: &'a Path,
    pub port: u16,
    pub superuser: &'a str,
    pub auth: AuthMode,
    pub password: Option<&'a SecretString>,
    pub timeout: Duration,
}

/// Renders a minimal `[client]` option file carrying the superuser and
/// (when `AuthMode::Password`) the escaped password.
pub(crate) fn render_admin_defaults(
    superuser: &str,
    auth: AuthMode,
    password: Option<&SecretString>,
) -> SecretString {
    let mut out = String::new();
    let _ = writeln!(out, "[client]");
    let _ = writeln!(out, "user={superuser}");
    if auth == AuthMode::Password
        && let Some(pw) = password
    {
        let escaped = pw.expose().replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(out, "password=\"{escaped}\"");
    }
    SecretString::new(out)
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Runs `<admin_bin> --defaults-file=<tmp> --connect-timeout=... shutdown`
/// bounded by `req.timeout`.
pub(crate) fn run_admin_shutdown(req: &AdminShutdownRequest<'_>) -> Result<()> {
    if req.auth == AuthMode::Password && req.password.is_none() {
        tracing::warn!(
            instance = %req.instance_name,
            engine = req.engine_label,
            "graceful shutdown skipped: missing superuser credential"
        );
        return Err(DblError::Engine {
            reason: format!(
                "{} graceful shutdown requires superuser credential",
                req.engine_label
            ),
        });
    }

    let nonce: u64 = rand::random();
    let defaults_path = req
        .run_dir
        .join(format!(".{}-shutdown-{nonce:016x}.cnf", req.file_prefix));
    let defaults_content = render_admin_defaults(req.superuser, req.auth, req.password);
    crate::fsutil::write_private_file_atomic(&defaults_path, defaults_content.expose().as_bytes())
        .map_err(|e| DblError::Engine {
            reason: format!(
                "cannot write {} shutdown defaults file {}: {e}",
                req.engine_label,
                defaults_path.display()
            ),
        })?;
    let _defaults_guard = TempFileGuard(defaults_path.clone());

    let out_path = req
        .run_dir
        .join(format!(".{}-shutdown-{nonce:016x}.out", req.file_prefix));
    let _out_guard = TempFileGuard(out_path.clone());

    let mut cmd = build_shutdown_command(req, &defaults_path, &out_path);
    let mut child = spawn_retrying_etxtbsy(&mut cmd).map_err(|e| {
        tracing::warn!(
            instance = %req.instance_name,
            tool = req.tool_label,
            error = %e,
            "admin shutdown failed to execute"
        );
        DblError::Engine {
            reason: format!("failed to execute {} shutdown: {e}", req.tool_label),
        }
    })?;

    let status = wait_with_timeout(&mut child, req)?;
    if status.success() {
        return Ok(());
    }

    let code = status.code();
    let tail = read_bounded_tail(&out_path, req.password);
    tracing::warn!(
        instance = %req.instance_name,
        tool = req.tool_label,
        exit_code = ?code,
        "admin shutdown returned non-zero; falling back to termination"
    );
    let reason = if tail.is_empty() {
        format!("{} shutdown failed (code {code:?})", req.tool_label)
    } else {
        format!("{} shutdown failed (code {code:?}): {tail}", req.tool_label)
    };
    Err(DblError::Engine { reason })
}

fn build_shutdown_command(
    req: &AdminShutdownRequest<'_>,
    defaults_path: &Path,
    out_path: &Path,
) -> Command {
    let connect_timeout = req.timeout.as_secs().clamp(1, CONNECT_TIMEOUT_MAX_SECS);
    let mut cmd = Command::new(req.admin_bin);
    cmd.arg(format!("--defaults-file={}", defaults_path.display()))
        .arg(format!("--connect-timeout={connect_timeout}"))
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-P")
        .arg(req.port.to_string())
        .arg("--protocol=TCP")
        .arg("shutdown");
    cmd.stdin(Stdio::null());
    attach_output_file(&mut cmd, out_path);
    cmd.env_remove("MYSQL_PWD");
    cmd.env_remove("MYSQL_TCP_PORT");
    cmd
}

fn attach_output_file(cmd: &mut Command, out_path: &Path) {
    let file = open_private_output_file(out_path);
    match file.and_then(|f| f.try_clone().map(|f2| (f, f2))) {
        Ok((stdout_file, stderr_file)) => {
            cmd.stdout(Stdio::from(stdout_file));
            cmd.stderr(Stdio::from(stderr_file));
        }
        Err(_) => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
    }
}

fn open_private_output_file(path: &Path) -> std::io::Result<fs::File> {
    let mut opts = OpenOptions::new();
    opts.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    req: &AdminShutdownRequest<'_>,
) -> Result<ExitStatus> {
    let deadline = Instant::now() + req.timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!(
                    instance = %req.instance_name,
                    tool = req.tool_label,
                    timeout_secs = req.timeout.as_secs(),
                    "admin shutdown timed out; falling back to termination"
                );
                return Err(DblError::Engine {
                    reason: format!(
                        "{} shutdown timed out after {}s",
                        req.tool_label,
                        req.timeout.as_secs()
                    ),
                });
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DblError::Engine {
                    reason: format!("failed waiting for {} shutdown: {e}", req.tool_label),
                });
            }
        }
    }
}

fn read_bounded_tail(path: &Path, password: Option<&SecretString>) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let Ok(meta) = file.metadata() else {
        return String::new();
    };
    let len = meta.len();
    let start = len.saturating_sub(OUTPUT_TAIL_MAX_BYTES as u64);
    if start > 0 && file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if let Some(pw) = password {
        let raw = pw.expose();
        if !raw.is_empty() {
            text = text.replace(raw, "[REDACTED]");
        }
    }
    text.trim().to_owned()
}

fn spawn_retrying_etxtbsy(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.raw_os_error() == Some(26) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}
