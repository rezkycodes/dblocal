//! User-facing messages for autostart (F-AUT-01, F-AUT-02).
//!
//! Lives inside the module so no shared file (`strings.rs`) has to change.
//! No message here contains a path that could hold a secret; the executable
//! path is shown only in the diagnostics the user asked for.

/// The platform has no user-level login mechanism DBLocal can use.
pub const ERR_UNSUPPORTED: &str = "Running automatically at login is not supported on this platform yet.";
/// Writing or removing the login item file failed.
pub const ERR_WRITE_FAILED: &str = "Could not write the autostart file for this user.";
/// A helper command (`systemctl --user`, `launchctl`, `reg`) failed.
pub const ERR_COMMAND_FAILED: &str = "The system command for autostart failed.";
/// The running executable path is unusable as a login command.
pub const ERR_INVALID_EXE: &str = "The application location cannot be used for autostart.";
/// The login item state could not be read.
pub const ERR_STATUS_FAILED: &str = "Could not read the autostart-at-login status.";
/// Starting flagged instances could not be scheduled.
pub const ERR_PLAN_FAILED: &str = "Some instances could not be scheduled for autostart.";

/// `--autostart` ran headless with nothing left to do: said before exiting.
pub const MODE_EXIT_NO_INSTANCES: &str =
    "Autostart: no instances are running, DBLocal exits without a window.";
/// `--autostart` ran headless with at least one instance started.
pub const MODE_HEADLESS_KEEP: &str =
    "Autostart: the tray is unavailable; the instances keep running without a window.";
/// One flagged instance started successfully in the login pass (D1).
pub const OUTCOME_STARTED: &str = "autostart: instance started";
/// One flagged instance failed to start; the `DBL-xxxx` code is appended (D1).
pub const OUTCOME_FAILED: &str = "autostart: instance failed to start";

/// Formats the per-instance success line: `autostart: <name> started`.
pub fn outcome_started(name: &str) -> String {
    format!("{OUTCOME_STARTED}: {name}")
}

/// Formats the per-instance failure line: `autostart: <name> failed — <code>`.
pub fn outcome_failed(name: &str, detail: &str) -> String {
    format!("{OUTCOME_FAILED}: {name} — {detail}")
}

/// GUI login mode: a tray host accepted the icon, so there is no window (AC3).
pub const GUI_TRAY_ONLY: &str = "Autostart: tray active, no window.";
/// GUI login mode: no tray host, so the main window was opened (D4 fallback).
pub const GUI_WINDOW_OPENED: &str = "Autostart: the tray is unavailable, the window was opened.";
/// GUI login mode: the window could not be opened.
pub const GUI_WINDOW_FAILED: &str = "Autostart: could not open the window";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_started_names_the_instance() {
        let line = outcome_started("vk-auto");
        assert!(line.contains(OUTCOME_STARTED));
        assert!(line.contains("vk-auto"));
    }

    #[test]
    fn outcome_failed_names_the_instance_and_carries_the_detail() {
        let line = outcome_failed("vk-blocked", "[DBL-1005] port in use");
        assert!(line.contains(OUTCOME_FAILED));
        assert!(line.contains("vk-blocked"));
        assert!(line.contains("[DBL-1005]"));
    }

    #[test]
    fn mode_and_gui_messages_are_non_empty() {
        for text in [
            MODE_EXIT_NO_INSTANCES,
            MODE_HEADLESS_KEEP,
            OUTCOME_STARTED,
            OUTCOME_FAILED,
            GUI_TRAY_ONLY,
            GUI_WINDOW_OPENED,
            GUI_WINDOW_FAILED,
        ] {
            assert!(!text.is_empty());
        }
    }
}
