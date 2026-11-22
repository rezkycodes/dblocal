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

/// Extra slack added to the engine start timeout when the pass waits for a
/// start to finish (D1).
///
/// The wait bound is the engine's own start timeout (`START_TIMEOUT_SECS` /
/// `START_TIMEOUT_VALKEY_SECS`) plus this margin, so the observer never gives
/// up before the engine itself would. It is a margin, not a new timeout.
pub const AUTOSTART_START_MARGIN_SECS: u64 = 15;

/// Engine start timeout, re-exported so the autostart module derives its wait
/// bound from the one source of truth rather than a literal.
pub const START_TIMEOUT_SECS: u64 = crate::consts::START_TIMEOUT_SECS;
/// Valkey start timeout, re-exported for the same reason.
pub const START_TIMEOUT_SECS_VALKEY: u64 = crate::consts::START_TIMEOUT_VALKEY_SECS;

/// How often the login pass re-reads the store while waiting for a start (D1).
pub const AUTOSTART_WAIT_POLL_MS: u64 = crate::consts::HEALTH_CHECK_POLL_MS;

// Compile-time invariants so a bad tweak cannot ship even without a test.
const _: () = assert!(AUTOSTART_CONCURRENCY >= 2);
const _: () = assert!(AUTOSTART_START_MARGIN_SECS > 0);
const _: () = assert!(START_TIMEOUT_SECS > 0);
const _: () = assert!(START_TIMEOUT_SECS_VALKEY > 0);
const _: () = assert!(AUTOSTART_WAIT_POLL_MS > 0);
