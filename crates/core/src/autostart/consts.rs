//! Every value the autostart module hardcodes.
//!
//! Kept module-local instead of `crate::consts` so this worker never edits a
//! file other workers are also changing (see `docs/architecture/error-codes.md`
//! for the same rule about `error.rs` and `strings.rs`).

/// Stable identifier of the login item on every platform.
pub const LOGIN_ITEM_ID: &str = "dblocal";
/// Label used by the macOS LaunchAgent; the reverse-DNS form avoids collisions.
pub const MACOS_LABEL: &str = "dev.dblocal.DBLocal";
/// Name of the systemd user unit file.
pub const SYSTEMD_UNIT_FILE: &str = "dblocal.service";
/// Name of the XDG autostart desktop file.
pub const XDG_DESKTOP_FILE: &str = "dblocal.desktop";
/// Name of the LaunchAgent plist file.
pub const MACOS_PLIST_FILE: &str = "dev.dblocal.DBLocal.plist";
/// File name of the desktop entry `Name=` value.
pub const DESKTOP_NAME: &str = "DBLocal";
/// Registry value name under the `Run` key.
pub const REGISTRY_VALUE: &str = "DBLocal";
/// Registry key holding the per-user login items.
pub const REGISTRY_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// Command-line flag the login item passes back to the app.
pub const AUTOSTART_FLAG: &str = "--autostart";
/// `systemctl` program name.
pub const SYSTEMCTL: &str = "systemctl";
/// `launchctl` program name.
pub const LAUNCHCTL: &str = "launchctl";
/// `reg` program name.
pub const REG: &str = "reg";
/// systemd unit name passed to `systemctl --user`.
pub const SYSTEMD_UNIT_ARG: &str = "dblocal.service";

/// How many flagged instances start at the same time.
///
/// Two is the minimum that proves "one hanging start never blocks the rest";
/// four keeps a login-time burst bounded on a small machine.
pub const AUTOSTART_CONCURRENCY: usize = 4;

// Compile-time invariants so a bad tweak cannot ship even without a test.
const _: () = assert!(AUTOSTART_CONCURRENCY >= 2);
