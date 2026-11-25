//! Graceful MariaDB shutdown via `mariadb-admin` (used on Windows).
//!
//! On Unix, MariaDB shuts down cleanly on `SIGTERM`. Windows has no `SIGTERM`,
//! so `shutdown_by_admin` verifies the PID from `mariadbd.pid` via `sysinfo`
//! and then invokes `mariadb-admin.exe` via the shared bounded admin-shutdown
//! helper (`--defaults-file`, `--connect-timeout`, wall-clock timeout, RAII
//! cleanup).

use std::path::{Path, PathBuf};
#[cfg(any(windows, all(test, unix)))]
use std::time::Duration;

#[cfg(any(windows, all(test, unix)))]
use crate::consts;
#[cfg(any(windows, all(test, unix)))]
use crate::engine::RunContext;
#[cfg(any(windows, all(test, unix)))]
use crate::engine::admin_shutdown::{AdminShutdownRequest, run_admin_shutdown};
#[cfg(any(windows, all(test, unix)))]
use crate::error::{DblError, Result};

#[cfg(any(windows, all(test, unix)))]
use super::identity;
#[cfg(windows)]
use super::pidfile_path;
#[cfg(test)]
pub(super) use crate::engine::admin_shutdown::render_admin_defaults;

pub(super) fn admin_program(runtime_dir: &Path, windows: bool) -> PathBuf {
    let bin = if windows {
        "mariadb-admin.exe"
    } else {
        "mariadb-admin"
    };
    runtime_dir.join("bin").join(bin)
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

    let req = AdminShutdownRequest {
        engine_label: "mariadb",
        tool_label: "mariadb-admin",
        file_prefix: "mariadb",
        instance_name: &ctx.instance_name,
        admin_bin,
        run_dir: &ctx.run_dir,
        port: ctx.port.get(),
        superuser: &ctx.superuser,
        auth: ctx.auth,
        password: ctx.credential.as_ref().map(|c| &c.password),
        timeout,
    };
    run_admin_shutdown(&req)?;
    tracing::info!(
        instance = %ctx.instance_name,
        pid,
        "mariadb graceful shutdown requested via mariadb-admin"
    );
    Ok(())
}
