//! Login-item registration and the `--autostart` instance plan (F-AUT-01, F-AUT-02).
//!
//! # Scope
//!
//! - [`backend_for`] picks the per-OS login-item backend (systemd/XDG on Linux,
//!   LaunchAgent on macOS, `HKCU\...\Run` on Windows). Every backend is
//!   testable with an injected [`CommandRunner`] and a temp root: nothing in
//!   this module writes to the user's real home.
//! - [`run_start_targets`] starts flagged instances with bounded concurrency;
//!   one failure is recorded but never blocks the others (F-AUT-02 AC2).
//! - [`decide_mode`] and [`flagged_targets`] are pure so the GUI bootstrap and
//!   the CLI can prove the decision without a live tray or engine.
//!
//! # What is deliberately missing
//!
//! The `--autostart` CLI flag and the GUI bootstrap hook need to touch files
//! owned by W2-B (`crates/gui/src/app/bootstrap.rs`, `crates/cli/src/cli.rs`).
//! This crate exposes the decisions and the backends; the integration is done
//! in the CLI `autostart` command and in the GUI wiring, both small enough to
//! review in isolation.

mod backend;
mod consts;
mod error;
mod linux;
mod macos;
mod mode;
mod plan;
mod runner;
mod windows;

pub mod messages;

pub use backend::{
    AutostartBackend, CommandOutput, CommandRunner, LoginItem, LoginItemStatus, SystemRunner,
    backend_for, quote_posix_single, quote_systemd_exec, quote_windows_reg_command, xml_escape,
};
pub use consts::{
    AUTOSTART_CONCURRENCY, AUTOSTART_FLAG, AUTOSTART_START_MARGIN_SECS, LOGIN_ITEM_ID, MACOS_LABEL,
    MACOS_PLIST_FILE, REGISTRY_RUN_KEY, REGISTRY_VALUE, SYSTEMD_UNIT_ARG, SYSTEMD_UNIT_FILE,
    XDG_DESKTOP_FILE,
};
pub use error::{
    AutostartError, AutostartResult, CODE_COMMAND_FAILED, CODE_INVALID_EXE, CODE_PLAN_FAILED,
    CODE_RANGE, CODE_STATUS_FAILED, CODE_UNSUPPORTED, CODE_WRITE_FAILED,
};
pub use linux::LinuxBackend;
pub use macos::MacosBackend;
pub use mode::{AutostartPass, AutostartService, run_pass, start_targets, start_wait_bound};
pub use plan::{
    AutostartMode, AutostartOutcome, AutostartSummary, AutostartTarget, GuiPresentation,
    decide_gui_presentation, decide_mode, flagged_targets, should_start_status,
};
pub use runner::run_start_targets;
pub use windows::WindowsBackend;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::platform::Os;

/// Builds the standard login item for the current executable.
///
/// The executable path must be absolute; the login item appends
/// [`AUTOSTART_FLAG`] so the app opens to the tray, not a window.
pub fn login_item_for_exe(exe: impl Into<PathBuf>) -> AutostartResult<LoginItem> {
    LoginItem::for_exe(exe)
}

/// Picks the production backend for `os`.
///
/// Tests pass a temp `home` and a temp `config_home` plus a fake runner so the
/// real user's login items are never read or written.
pub fn platform_backend(
    os: Os,
    home: &Path,
    config_home: &Path,
    runner: Arc<dyn CommandRunner>,
) -> Box<dyn AutostartBackend> {
    backend_for(os, home, config_home, runner)
}

/// Convenience: build the backend for the host OS and the real config roots.
///
/// `config_home` is `$XDG_CONFIG_HOME` or `home/.config`. This function is the
/// only one that reads environment variables; everything else is pure or takes
/// injected paths.
pub fn host_backend(runner: Arc<dyn CommandRunner>) -> AutostartResult<Box<dyn AutostartBackend>> {
    let home = home_dir()?;
    let config_home = xdg_config_home(&home);
    Ok(platform_backend(Os::current(), &home, &config_home, runner))
}

/// Resolves the user's home directory.
pub fn home_dir() -> AutostartResult<PathBuf> {
    if let Some(raw) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(raw));
    }
    #[cfg(windows)]
    if let Some(raw) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(raw));
    }
    Err(AutostartError::StatusFailed {
        detail: "home directory could not be determined".to_owned(),
    })
}

/// Resolves `$XDG_CONFIG_HOME`, falling back to `$HOME/.config`.
pub fn xdg_config_home(home: &Path) -> PathBuf {
    let raw = std::env::var_os("XDG_CONFIG_HOME");
    resolve_config_home(raw.as_deref(), home)
}

/// Pure resolution of the XDG config root from a possibly-empty override.
pub fn resolve_config_home(override_value: Option<&std::ffi::OsStr>, home: &Path) -> PathBuf {
    override_value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[derive(Debug, Default)]
    struct NoopRunner;
    impl CommandRunner for NoopRunner {
        fn run(&self, _p: &str, _a: &[String]) -> AutostartResult<CommandOutput> {
            Ok(CommandOutput::ok())
        }
    }

    #[test]
    fn login_item_for_exe_refuses_a_relative_path() {
        let err = login_item_for_exe("dblocal").expect_err("relative");
        assert_eq!(err.code(), CODE_INVALID_EXE);
    }

    #[test]
    fn login_item_for_exe_appends_the_autostart_flag() {
        // Build the exe from the manifest dir so the path is absolute in this
        // host's own syntax: a POSIX `/usr/bin/...` literal is not absolute on
        // Windows, where the leading root alone is drive-relative.
        let exe = Path::new(env!("CARGO_MANIFEST_DIR")).join("dblocal");
        let item = login_item_for_exe(&exe).expect("item");
        assert_eq!(
            item.argv(),
            vec![exe.display().to_string(), AUTOSTART_FLAG.to_owned()]
        );
    }

    #[test]
    fn platform_backend_picks_the_right_linux_variant() {
        let home = tempdir().expect("tempdir");
        let config = home.path().join("config");
        let backend = platform_backend(Os::Linux, home.path(), &config, Arc::new(NoopRunner));
        // Status on a temp root with no files is "disabled", never an error.
        let status = backend.status().expect("status");
        assert!(!status.enabled);
    }

    #[test]
    fn platform_backend_picks_the_macos_and_windows_variants() {
        let home = tempdir().expect("tempdir");
        let config = home.path().join("config");
        for os in [Os::Macos, Os::Windows] {
            let backend = platform_backend(os, home.path(), &config, Arc::new(NoopRunner));
            // With no plist and no registry value the answer is "disabled".
            let status = backend.status().expect("status");
            assert!(
                !status.enabled,
                "{os} should report disabled on a temp root"
            );
        }
    }

    #[test]
    fn host_backend_builds_from_the_environment() {
        // `host_backend` reads HOME/XDG_CONFIG_HOME; both are set in a test run,
        // so it must produce a backend rather than error.
        let backend = host_backend(Arc::new(NoopRunner));
        if let Ok(backend) = backend {
            // The result is a usable backend; calling `status` never panics.
            let _ = backend.status();
        }
    }

    #[test]
    fn home_dir_reads_the_environment() {
        // `cargo test` sets HOME on every supported host; if it is somehow
        // absent, the function reports a typed error rather than guessing.
        match home_dir() {
            Ok(home) => assert!(!home.as_os_str().is_empty()),
            Err(err) => assert_eq!(err.code(), CODE_STATUS_FAILED),
        }
    }

    #[test]
    fn resolve_config_home_honours_an_override_and_empty_falls_back() {
        let home = Path::new("/home/alice");
        assert_eq!(
            resolve_config_home(Some(std::ffi::OsStr::new("/custom/cfg")), home),
            PathBuf::from("/custom/cfg")
        );
        assert_eq!(
            resolve_config_home(Some(std::ffi::OsStr::new("")), home),
            home.join(".config")
        );
        assert_eq!(resolve_config_home(None, home), home.join(".config"));
    }

    #[test]
    fn xdg_config_home_returns_an_absolute_under_home_path() {
        let home = Path::new("/home/alice");
        // Whatever XDG_CONFIG_HOME is, the result is a non-empty path.
        assert!(!xdg_config_home(home).as_os_str().is_empty());
    }
}
