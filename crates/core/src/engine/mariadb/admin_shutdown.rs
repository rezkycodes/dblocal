//! Graceful MariaDB shutdown via `mariadb-admin` (used on Windows).
//!
//! On Unix, MariaDB shuts down cleanly on `SIGTERM`. Windows has no `SIGTERM`,
//! so `shutdown_by_admin` verifies the PID from `mariadbd.pid` via `sysinfo`
//! and then invokes `mariadb-admin.exe` with a temporary private
//! `--defaults-file` (never passing the password in argv or env, and ignoring
//! any global `my.ini` on the host). Both `--connect-timeout` and a process
//! wall-clock timeout bound the call so a hung server never blocks escalation.
//! The temporary file is removed on drop regardless of success, failure, or
//! timeout.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::consts;
use crate::engine::RunContext;
use crate::error::{DblError, Result};
use crate::model::AuthMode;
use crate::secret::SecretString;

use super::identity;
#[cfg(windows)]
use super::pidfile_path;

const CONNECT_TIMEOUT_MAX_SECS: u64 = 5;
const POLL_INTERVAL_MS: u64 = 50;

pub(super) fn admin_program(runtime_dir: &Path, windows: bool) -> PathBuf {
    let bin = if windows {
        "mariadb-admin.exe"
    } else {
        "mariadb-admin"
    };
    runtime_dir.join("bin").join(bin)
}

pub(super) fn render_admin_defaults(
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

struct TempDefaultsFile(PathBuf);

impl Drop for TempDefaultsFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(windows)]
pub(super) fn shutdown_by_admin(ctx: &RunContext) -> Result<()> {
    let pid_path = pidfile_path(&ctx.data_dir);
    let pid = identity::read_pidfile(&pid_path)?;
    let expected = identity::expected_identity(ctx);
    let verified = identity::verify_pid_identity(pid, &expected);
    let admin = admin_program(&ctx.runtime_dir, true);
    let timeout = Duration::from_secs(consts::STOP_GRACE_SECS);
    shutdown_via_admin_with_timeout(ctx, pid, verified, &admin, timeout)
}

#[cfg(all(test, unix))]
pub(super) fn shutdown_via_admin_with(
    ctx: &RunContext,
    pid: u32,
    verified: identity::PidIdentity,
    admin_bin: &Path,
) -> Result<()> {
    shutdown_via_admin_with_timeout(
        ctx,
        pid,
        verified,
        admin_bin,
        Duration::from_secs(consts::STOP_GRACE_SECS),
    )
}

#[cfg(any(windows, all(test, unix)))]
pub(super) fn shutdown_via_admin_with_timeout(
    ctx: &RunContext,
    pid: u32,
    verified: identity::PidIdentity,
    admin_bin: &Path,
    timeout: Duration,
) -> Result<()> {
    match verified {
        identity::PidIdentity::Match => {}
        identity::PidIdentity::NoSuchProcess => {
            tracing::info!(pid, "mariadb pid already gone");
            return Ok(());
        }
        identity::PidIdentity::NotOurs(reason) => {
            tracing::warn!(
                pid,
                reason = %reason,
                "mariadb pidfile pid is not our mariadbd; refusing to send shutdown"
            );
            return Ok(());
        }
        identity::PidIdentity::Unverifiable(reason) => {
            return Err(DblError::Engine {
                reason: format!(
                    "mariadb pid {pid} identity unverifiable; refusing to send shutdown: {reason}"
                ),
            });
        }
    }

    if ctx.auth == AuthMode::Password && ctx.credential.is_none() {
        tracing::warn!(
            instance = %ctx.instance_name,
            "mariadb graceful shutdown skipped: missing superuser credential"
        );
        return Err(DblError::Engine {
            reason: "mariadb graceful shutdown requires superuser credential".to_owned(),
        });
    }

    let password = ctx.credential.as_ref().map(|c| &c.password);
    let defaults_content = render_admin_defaults(&ctx.superuser, ctx.auth, password);
    let nonce: u64 = rand::random();
    let defaults_path = ctx
        .run_dir
        .join(format!(".mariadb-shutdown-{nonce:016x}.cnf"));
    crate::fsutil::write_private_file_atomic(&defaults_path, defaults_content.expose().as_bytes())
        .map_err(|e| DblError::Engine {
            reason: format!(
                "cannot write mariadb shutdown defaults file {}: {e}",
                defaults_path.display()
            ),
        })?;
    let _guard = TempDefaultsFile(defaults_path.clone());

    let connect_timeout = timeout.as_secs().clamp(1, CONNECT_TIMEOUT_MAX_SECS);
    let mut cmd = Command::new(admin_bin);
    cmd.arg(format!("--defaults-file={}", defaults_path.display()))
        .arg(format!("--connect-timeout={connect_timeout}"))
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-P")
        .arg(ctx.port.get().to_string())
        .arg("--protocol=TCP")
        .arg("shutdown");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.env_remove("MYSQL_PWD");
    cmd.env_remove("MYSQL_TCP_PORT");

    let mut child = spawn_retrying_etxtbsy(&mut cmd).map_err(|e| {
        tracing::warn!(
            instance = %ctx.instance_name,
            error = %e,
            "mariadb-admin shutdown failed to execute"
        );
        DblError::Engine {
            reason: format!("failed to execute mariadb-admin shutdown: {e}"),
        }
    })?;

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!(
                    instance = %ctx.instance_name,
                    timeout_secs = timeout.as_secs(),
                    "mariadb-admin shutdown timed out; falling back to termination"
                );
                return Err(DblError::Engine {
                    reason: format!(
                        "mariadb-admin shutdown timed out after {}s",
                        timeout.as_secs()
                    ),
                });
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DblError::Engine {
                    reason: format!("failed waiting for mariadb-admin shutdown: {e}"),
                });
            }
        }
    };

    if status.success() {
        tracing::info!(
            instance = %ctx.instance_name,
            pid,
            "mariadb graceful shutdown requested via mariadb-admin"
        );
        return Ok(());
    }

    let code = status.code();
    tracing::warn!(
        instance = %ctx.instance_name,
        exit_code = ?code,
        "mariadb-admin shutdown returned non-zero; falling back to termination"
    );
    Err(DblError::Engine {
        reason: format!("mariadb-admin shutdown failed (code {code:?})"),
    })
}

#[cfg(any(windows, all(test, unix)))]
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
