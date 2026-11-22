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
    let runner: Arc<dyn CommandRunner> = Arc::new(SystemRunner);
    let backend = match autostart::host_backend(runner) {
        Ok(backend) => backend,
        Err(e) => {
            let _ = writeln!(err, "[{}] {}", e.code(), e.user_message());
            return EXIT_GENERAL;
        }
    };
    run_action_with(out, err, backend.as_ref(), action, json)
}

/// The injectable form: `backend` is a real or fake login-item backend.
pub fn run_action_with(
    out: &mut dyn Write,
    err: &mut dyn Write,
    backend: &dyn AutostartBackend,
    action: &str,
    json: bool,
) -> u8 {
    let result = match action {
        "status" => backend.status(),
        "enable" => {
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(_) => {
                    let e = AutostartError::InvalidExe {
                        reason: "current executable could not be resolved",
                    };
                    let _ = writeln!(err, "[{}] {}", e.code(), e.user_message());
                    return EXIT_GENERAL;
                }
            };
            LoginItem::for_exe(exe).and_then(|item| backend.enable(&item))
        }
        "disable" => backend.disable(),
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

/// Runs the hidden login-item mode (`autostart-run`, or raw `--autostart`).
///
/// On a machine with a display the GUI opens to the tray and starts flagged
/// instances with no window (F-AUT-01 AC3). In a headless environment it falls
/// back to a service-only pass that starts the flagged instances and exits with
/// a clear message when there is nothing to keep alive.
pub fn run_login_mode(_headless: bool) -> u8 {
    if dblocal_gui::gui_unavailable_reason().is_none() {
        return match dblocal_gui::run_autostart() {
            Ok(()) => EXIT_OK,
            Err(err) => {
                eprintln!("{}", err.diagnostic());
                EXIT_GENERAL
            }
        };
    }
    let mut err = std::io::stderr().lock();
    match run_login_mode_headless(&mut err) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(err, "{message}");
            EXIT_GENERAL
        }
    }
}

fn run_login_mode_headless<W: Write>(err: &mut W) -> Result<u8, String> {
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
    use dblocal_core::autostart::{AutostartResult, LoginItemStatus};
    use std::path::PathBuf;

    /// A backend that returns a fixed status or error without touching the host.
    struct FakeBackend {
        enabled: bool,
        location: PathBuf,
        fail: bool,
    }

    impl FakeBackend {
        fn enabled() -> Self {
            Self {
                enabled: true,
                location: PathBuf::from("/tmp/fake/dblocal.service"),
                fail: false,
            }
        }
        fn disabled() -> Self {
            Self {
                enabled: false,
                location: PathBuf::from("/tmp/fake/dblocal.service"),
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                enabled: false,
                location: PathBuf::from("/tmp/fake"),
                fail: true,
            }
        }
    }

    impl AutostartBackend for FakeBackend {
        fn enable(&self, _item: &LoginItem) -> AutostartResult<LoginItemStatus> {
            self.status()
        }
        fn disable(&self) -> AutostartResult<LoginItemStatus> {
            self.status()
        }
        fn status(&self) -> AutostartResult<LoginItemStatus> {
            if self.fail {
                return Err(AutostartError::CommandFailed {
                    program: "systemctl",
                    detail: "no bus".to_owned(),
                });
            }
            Ok(LoginItemStatus {
                enabled: self.enabled,
                location: self.location.clone(),
            })
        }
    }

    fn run_with(backend: &FakeBackend, action: &str, json: bool) -> (u8, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_action_with(&mut out, &mut err, backend, action, json);
        (
            code,
            String::from_utf8_lossy(&out).to_string(),
            String::from_utf8_lossy(&err).to_string(),
        )
    }

    #[test]
    fn status_enabled_prints_the_enabled_label_and_location() {
        let (code, out, err) = run_with(&FakeBackend::enabled(), "status", false);
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, format!("{}\n", crate::messages::AUTOSTART_ENABLED));
        assert!(err.contains(crate::messages::AUTOSTART_LOCATION));
        assert!(err.contains("/tmp/fake/dblocal.service"));
    }

    #[test]
    fn status_disabled_prints_the_disabled_label() {
        let (code, out, _err) = run_with(&FakeBackend::disabled(), "status", false);
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, format!("{}\n", crate::messages::AUTOSTART_DISABLED));
    }

    #[test]
    fn status_json_is_one_line_with_enabled_and_location() {
        let (code, out, err) = run_with(&FakeBackend::enabled(), "status", true);
        assert_eq!(code, EXIT_OK);
        let value: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
        assert_eq!(value["enabled"], serde_json::Value::Bool(true));
        assert!(
            value["location"]
                .as_str()
                .expect("location")
                .ends_with("dblocal.service")
        );
        assert!(
            err.is_empty(),
            "json mode writes nothing to stderr: {err:?}"
        );
    }

    #[test]
    fn enable_and_disable_return_ok_and_a_status() {
        let (code, out, _) = run_with(&FakeBackend::enabled(), "enable", false);
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, format!("{}\n", crate::messages::AUTOSTART_ENABLED));
        let (code, out, _) = run_with(&FakeBackend::disabled(), "disable", false);
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, format!("{}\n", crate::messages::AUTOSTART_DISABLED));
    }

    #[test]
    fn a_failing_backend_exits_one_with_the_code_on_stderr() {
        let (code, out, err) = run_with(&FakeBackend::failing(), "status", false);
        assert_eq!(code, EXIT_GENERAL);
        assert!(out.is_empty());
        assert!(err.contains(dblocal_core::autostart::CODE_COMMAND_FAILED));
    }

    #[test]
    fn an_unknown_action_exits_two_and_names_it() {
        let (code, out, err) = run_with(&FakeBackend::enabled(), "frobnicate", false);
        assert_eq!(code, EXIT_BAD_ARGS);
        assert!(out.is_empty());
        assert!(err.contains(crate::messages::ERR_AUTOSTART_ACTION));
    }

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
}
