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
    let mut stderr = std::io::stderr().lock();
    let service = build_login_service().map_err(|e| e.to_string());
    run_login_mode_with(
        &mut stderr,
        dblocal_gui::gui_unavailable_reason().is_none(),
        &|err| {
            let service = service.as_ref().map_err(|e: &String| e.clone())?;
            report_pass(service, err)
        },
        &|| match dblocal_gui::run_autostart() {
            Ok(()) => EXIT_OK,
            Err(err) => {
                eprintln!("{}", err.diagnostic());
                EXIT_GENERAL
            }
        },
    )
}

/// Builds the production service for a headless login pass.
pub fn build_login_service() -> dblocal_core::Result<Arc<dyn dblocal_core::service::DblService>> {
    build_service_for_paths(dblocal_core::paths::DblPaths::from_env()?)
}

/// Builds a service over explicit paths; testable with a temp root.
pub fn build_service_for_paths(
    paths: dblocal_core::paths::DblPaths,
) -> dblocal_core::Result<Arc<dyn dblocal_core::service::DblService>> {
    let service = dblocal_core::service::LocalService::new(
        paths,
        dblocal_core::service::ServiceOptions::default(),
    )?;
    Ok(Arc::new(service))
}

/// Decides and runs the login mode with injectable seams.
///
/// `gui_available` is true when a display and the main thread are usable;
/// `headless` runs the service-only pass into the writer; `gui` runs the
/// tray-only GUI path. The decision itself is what the tests pin.
pub fn run_login_mode_with(
    err: &mut dyn Write,
    gui_available: bool,
    headless: &dyn Fn(&mut dyn Write) -> Result<u8, String>,
    gui: &dyn Fn() -> u8,
) -> u8 {
    if gui_available {
        return gui();
    }
    match headless(err) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(err, "{message}");
            EXIT_GENERAL
        }
    }
}

/// Starts flagged instances and writes the presentation message.
fn report_pass(
    service: &Arc<dyn dblocal_core::service::DblService>,
    err: &mut dyn Write,
) -> Result<u8, String> {
    let pass = autostart::run_pass(service, false).map_err(|e| e.to_string())?;
    let (code, message) = headless_outcome(pass.mode);
    let _ = writeln!(err, "{message}");
    Ok(code)
}

/// The exit code and message for a decided mode, pure so it can be tested.
pub fn headless_outcome(mode: AutostartMode) -> (u8, &'static str) {
    match mode {
        AutostartMode::TrayOnly | AutostartMode::HeadlessKeepAlive => {
            (EXIT_OK, autostart::messages::MODE_HEADLESS_KEEP)
        }
        AutostartMode::HeadlessExit => (EXIT_OK, autostart::messages::MODE_EXIT_NO_INSTANCES),
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

    #[test]
    fn a_gui_available_run_takes_the_gui_path() {
        let mut err = Vec::new();
        let headless_called = std::cell::Cell::new(false);
        let code = run_login_mode_with(
            &mut err,
            true,
            &|_w: &mut dyn Write| {
                headless_called.set(true);
                Ok(EXIT_OK)
            },
            &|| EXIT_OK,
        );
        assert_eq!(code, EXIT_OK);
        assert!(
            !headless_called.get(),
            "the GUI path must not run the headless pass"
        );
    }

    #[test]
    fn a_headless_run_reports_the_pass_and_never_the_gui() {
        let mut err = Vec::new();
        let code = run_login_mode_with(
            &mut err,
            false,
            &|w: &mut dyn Write| {
                let _ = writeln!(w, "{}", autostart::messages::MODE_HEADLESS_KEEP);
                Ok(EXIT_OK)
            },
            &|| panic!("the GUI path must not run headless"),
        );
        assert_eq!(code, EXIT_OK);
        assert!(
            String::from_utf8_lossy(&err).contains("Autostart: tray tidak tersedia")
                || String::from_utf8_lossy(&err).contains("Autostart")
        );
    }

    #[test]
    fn a_headless_error_is_printed_and_exits_one() {
        let mut err = Vec::new();
        let code = run_login_mode_with(
            &mut err,
            false,
            &|_: &mut dyn Write| Err("DBL-8500 tidak didukung".to_owned()),
            &|| EXIT_OK,
        );
        assert_eq!(code, EXIT_GENERAL);
        assert!(String::from_utf8_lossy(&err).contains("DBL-8500"));
    }

    #[test]
    fn headless_outcome_covers_every_mode() {
        assert_eq!(
            headless_outcome(AutostartMode::TrayOnly).1,
            autostart::messages::MODE_HEADLESS_KEEP
        );
        assert_eq!(
            headless_outcome(AutostartMode::HeadlessKeepAlive).1,
            autostart::messages::MODE_HEADLESS_KEEP
        );
        assert_eq!(
            headless_outcome(AutostartMode::HeadlessExit).1,
            autostart::messages::MODE_EXIT_NO_INSTANCES
        );
    }

    #[test]
    fn status_through_the_real_host_backend_is_read_only_and_returns_a_status() {
        // `status` only reads the host login-item state (no enable/disable), so
        // it is safe to run against the real config root and covers the host
        // backend construction path.
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_action(&mut out, &mut err, "status", true);
        // Either a status line or a typed backend error, never a panic.
        assert!(code == EXIT_OK || code == EXIT_GENERAL);
        if code == EXIT_OK {
            let value: serde_json::Value =
                serde_json::from_str(String::from_utf8_lossy(&out).trim()).expect("json status");
            assert!(value.get("enabled").is_some());
        } else {
            assert!(!err.is_empty(), "a failure must explain itself on stderr");
        }
    }

    #[test]
    fn a_pass_over_a_temp_service_writes_a_presentation_message() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = dblocal_core::paths::DblPaths::with_root(home.path());
        let service: Arc<dyn dblocal_core::service::DblService> = Arc::new(
            dblocal_core::service::LocalService::new(
                paths,
                dblocal_core::service::ServiceOptions::default(),
            )
            .expect("service"),
        );
        let mut err: Vec<u8> = Vec::new();
        let code = report_pass(&service, &mut err).expect("pass");
        assert_eq!(code, EXIT_OK);
        assert!(
            String::from_utf8_lossy(&err).contains("Autostart"),
            "a presentation message must be written"
        );
    }

    #[test]
    fn build_service_for_paths_opens_over_a_temp_root() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = dblocal_core::paths::DblPaths::with_root(home.path());
        let service = build_service_for_paths(paths).expect("service over a temp root");
        assert!(service.list_instances().is_ok());
    }
}
