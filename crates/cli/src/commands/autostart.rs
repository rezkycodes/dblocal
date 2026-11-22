//! `dblocal autostart status|enable|disable` and the hidden login-item mode.
//!
//! `autostart` needs no state database or service, so it is dispatched before
//! the CLI builds one (a login item that runs before the store exists must not
//! fail). Its writers are injected so tests drive it without the real terminal.

use std::ffi::OsString;
use std::io::Write;
use std::sync::Arc;

use dblocal_core::autostart::{
    self, AutostartBackend, AutostartError, AutostartMode, CommandRunner, LoginItem, SystemRunner,
};

use crate::cli::{EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_OK};

/// Runs one `autostart` action (`status`, `enable`, `disable`) against injected
/// writers.
pub fn run_action(out: &mut dyn Write, err: &mut dyn Write, action: &str, json: bool) -> u8 {
    let result = match action {
        "status" => with_host_backend(|backend| backend.status()),
        "enable" => with_host_backend(|backend| {
            let exe = std::env::current_exe().map_err(|_| AutostartError::InvalidExe {
                reason: "current executable could not be resolved",
            })?;
            let item = LoginItem::for_exe(exe)?;
            backend.enable(&item)
        }),
        "disable" => with_host_backend(|backend| backend.disable()),
        _ => {
            let _ = writeln!(
                err,
                "[{}] {}",
                dblocal_core::error::CODE_INVALID_INPUT,
                crate::messages::ERR_AUTOSTART_ACTION
            );
            return EXIT_BAD_ARGS;
        }
    };
    match result {
        Ok(status) => {
            print_status(out, err, &status, json);
            EXIT_OK
        }
        Err(e) => {
            let _ = writeln!(err, "[{}] {}", e.code(), e.user_message());
            EXIT_GENERAL
        }
    }
}

fn print_status(
    out: &mut dyn Write,
    err: &mut dyn Write,
    status: &autostart::LoginItemStatus,
    json: bool,
) {
    if json {
        let value = serde_json::json!({
            "enabled": status.enabled,
            "location": status.location.display().to_string(),
        });
        let _ = writeln!(out, "{value}");
        return;
    }
    let label = if status.enabled {
        crate::messages::AUTOSTART_ENABLED
    } else {
        crate::messages::AUTOSTART_DISABLED
    };
    let _ = writeln!(out, "{label}");
    let _ = writeln!(
        err,
        "{}: {}",
        crate::messages::AUTOSTART_LOCATION,
        status.location.display()
    );
}

fn with_host_backend<F, T>(f: F) -> Result<T, AutostartError>
where
    F: FnOnce(&dyn AutostartBackend) -> Result<T, AutostartError>,
{
    let runner: Arc<dyn CommandRunner> = Arc::new(SystemRunner);
    let backend = autostart::host_backend(runner)?;
    f(backend.as_ref())
}

/// Runs the hidden login-item mode (`autostart-run`, or raw `--autostart`).
///
/// Starts flagged instances with bounded concurrency, independent of each
/// other, and decides the presentation mode. A CLI process owns no tray, so
/// `TrayOnly` cannot be reached here; a pass with nothing to start exits with a
/// clear message instead of opening a window (F-AUT-01 AC3).
pub fn run_login_mode(headless: bool) -> u8 {
    let mut err = std::io::stderr().lock();
    match run_login_mode_with(&mut err, headless) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(err, "{message}");
            EXIT_GENERAL
        }
    }
}

fn run_login_mode_with<W: Write>(err: &mut W, _headless: bool) -> Result<u8, String> {
    let paths = dblocal_core::paths::DblPaths::from_env().map_err(|e| e.to_string())?;
    let service = dblocal_core::service::LocalService::new(
        paths,
        dblocal_core::service::ServiceOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    let service: Arc<dyn dblocal_core::service::DblService> = Arc::new(service);
    let pass = autostart::run_pass(&service, false).map_err(|e| e.to_string())?;
    match pass.mode {
        AutostartMode::TrayOnly | AutostartMode::HeadlessKeepAlive => {
            let _ = writeln!(err, "{}", autostart::messages::MODE_HEADLESS_KEEP);
            Ok(EXIT_OK)
        }
        AutostartMode::HeadlessExit => {
            let _ = writeln!(err, "{}", autostart::messages::MODE_EXIT_NO_INSTANCES);
            Ok(EXIT_OK)
        }
    }
}

/// True when the process was launched as a login item.
///
/// Only the canonical token counts, so the mode is never triggered by accident.
pub fn launched_by_login_item(args: &[OsString]) -> bool {
    args.iter()
        .any(|arg| arg == dblocal_core::autostart::AUTOSTART_FLAG)
}

/// The OS for which we are registering.
pub fn current_os() -> dblocal_core::platform::Os {
    dblocal_core::platform::Os::current()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn launched_by_login_item_detects_only_the_canonical_flag() {
        assert!(launched_by_login_item(&[OsString::from("--autostart")]));
        assert!(!launched_by_login_item(&[OsString::from("autostart-run")]));
        assert!(!launched_by_login_item(&[]));
    }

    #[test]
    fn current_os_is_known() {
        let os = current_os();
        assert!(matches!(
            os,
            dblocal_core::platform::Os::Linux
                | dblocal_core::platform::Os::Macos
                | dblocal_core::platform::Os::Windows
        ));
    }

    #[test]
    fn an_unknown_action_exits_two_and_writes_to_stderr() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_action(&mut out, &mut err, "frobnicate", false);
        assert_eq!(code, EXIT_BAD_ARGS);
        assert!(out.is_empty());
        assert!(String::from_utf8_lossy(&err).contains(crate::messages::ERR_AUTOSTART_ACTION));
    }
}
