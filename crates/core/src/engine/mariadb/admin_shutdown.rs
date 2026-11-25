//! Graceful MariaDB shutdown via `mariadb-admin` (used on Windows).
//!
//! On Unix, MariaDB shuts down cleanly on `SIGTERM`. Windows has no `SIGTERM`,
//! so `shutdown_by_admin` verifies the PID from `mariadbd.pid` via `sysinfo`
//! and then invokes `mariadb-admin.exe` with a temporary private
//! `--defaults-extra-file` (never passing the password in argv or env). The
//! temporary file is removed on drop regardless of success or failure.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::engine::RunContext;
use crate::error::{DblError, Result};
use crate::model::AuthMode;
use crate::secret::SecretString;

use super::identity;
#[cfg(windows)]
use super::pidfile_path;

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
    shutdown_via_admin_with(ctx, pid, verified, &admin)
}

pub(super) fn shutdown_via_admin_with(
    ctx: &RunContext,
    pid: u32,
    verified: identity::PidIdentity,
    admin_bin: &Path,
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

    let mut cmd = Command::new(admin_bin);
    cmd.arg(format!("--defaults-extra-file={}", defaults_path.display()))
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-P")
        .arg(ctx.port.get().to_string())
        .arg("--protocol=TCP")
        .arg("shutdown");
    cmd.stdin(Stdio::null());
    cmd.env_remove("MYSQL_PWD");
    cmd.env_remove("MYSQL_TCP_PORT");

    let out = crate::fsutil::output_retrying_text_file_busy(cmd).map_err(|e| {
        tracing::warn!(
            instance = %ctx.instance_name,
            error = %e,
            "mariadb-admin shutdown failed to execute"
        );
        DblError::Engine {
            reason: format!("failed to execute mariadb-admin shutdown: {e}"),
        }
    })?;

    if out.status.success() {
        tracing::info!(
            instance = %ctx.instance_name,
            pid,
            "mariadb graceful shutdown requested via mariadb-admin"
        );
        return Ok(());
    }

    let code = out.status.code();
    tracing::warn!(
        instance = %ctx.instance_name,
        exit_code = ?code,
        "mariadb-admin shutdown returned non-zero; falling back to termination"
    );
    Err(DblError::Engine {
        reason: format!("mariadb-admin shutdown failed (code {code:?})"),
    })
}
