//! Pure state for the Settings dialog's login-item section (`F-AUT-01`).
//!
//! The **Updates** section reads the one in-app update read model
//! ([`super::update_state::UpdateState`]) instead of keeping a second copy, so
//! the title-bar icon, the update dialog and Settings can never disagree. What
//! remains here is the login item, whose switch must keep showing the machine's
//! real state through a write and after one fails.

use dblocal_core::service::UserFacingError;

use super::login_item::LoginStatus;

/// The login-item section, as rendered.
///
/// `Busy` and `Failed` carry the last confirmed [`LoginStatus`], because the
/// switch must keep showing the machine's real state while a write is in
/// flight and after one fails. A failed disable leaves the unit enabled, so
/// rendering the switch off would be a claim the machine contradicts (F2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoginState {
    /// The status was not read yet.
    #[default]
    Unknown,
    /// The status is being read or a write is in flight; the value is the last
    /// confirmed state, or `None` before the first read.
    Busy(Option<LoginStatus>),
    /// The item is registered or not, and where it lives.
    Known(LoginStatus),
    /// The read or write failed. The value is the last confirmed state, so the
    /// switch does not move while the typed error is shown.
    Failed(UserFacingError, Option<LoginStatus>),
}

impl LoginState {
    /// The last confirmed status, from whichever variant carries one.
    pub fn confirmed(&self) -> Option<&LoginStatus> {
        match self {
            Self::Known(status) => Some(status),
            Self::Busy(status) | Self::Failed(_, status) => status.as_ref(),
            Self::Unknown => None,
        }
    }
}

/// The whole Settings dialog's data.
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    /// Whether the dialog is open.
    pub open: bool,
    /// The login-item section.
    pub login: LoginState,
}

impl SettingsState {
    /// Opens the dialog with no stale answer: the caller re-reads on open.
    pub fn open(&mut self) {
        self.open = true;
        self.login = LoginState::Unknown;
    }

    /// Closes the dialog. An in-flight update keeps its state; only visibility
    /// flips.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The login state as a known value, when one was read.
    pub fn login_known(&self) -> Option<&LoginStatus> {
        self.login.confirmed()
    }

    /// What the login switch shows: the *last confirmed* state.
    ///
    /// A failed enable reports `false`, never the state the user asked for
    /// (`F-AUT-01` AC2 — "do not show enabled after a failed enable"), and a
    /// failed disable keeps reporting `true` (F2 — the unit is still enabled).
    pub fn login_enabled(&self) -> bool {
        self.login_known().is_some_and(|status| status.enabled)
    }

    /// The typed error of the login section, when the last call failed.
    pub fn login_error(&self) -> Option<&UserFacingError> {
        match &self.login {
            LoginState::Failed(err, _) => Some(err),
            _ => None,
        }
    }
}

/// A human label for the install kind (`F-PKG-05` AC2 tells the user where to
/// update instead).
///
/// Mirrors `InstallKind` without adding a `Display` impl to the core type: the
/// GUI is the only consumer that shows a word for it.
pub fn install_kind_label(kind: &dblocal_core::update::InstallKind) -> &'static str {
    use dblocal_core::update::InstallKind;
    match kind {
        InstallKind::AppImage { .. } => "AppImage",
        InstallKind::PortableTarball { .. } => "Portable tarball",
        InstallKind::WindowsZip { .. } => "Windows zip",
        InstallKind::WindowsMsi { .. } => "Windows MSI",
        InstallKind::MacApp { .. } => "macOS app",
        InstallKind::DistPackage { .. } => "Distribution package",
        InstallKind::Homebrew { .. } => "Homebrew",
        InstallKind::Unknown { .. } => "Unknown",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use dblocal_core::update::InstallKind;

    use super::*;

    #[test]
    fn opening_resets_every_previous_answer() {
        let mut state = SettingsState {
            open: false,
            login: LoginState::Failed(UserFacingError::new("DBL-8504", "no"), None),
        };
        state.open();
        assert!(state.open);
        assert_eq!(state.login, LoginState::Unknown);
    }

    #[test]
    fn a_failed_write_keeps_the_last_confirmed_state() {
        let off = LoginStatus {
            enabled: false,
            location: std::path::PathBuf::from("/tmp/x"),
        };
        let mut state = SettingsState::default();
        assert!(!state.login_enabled());

        state.login = LoginState::Failed(
            UserFacingError::new("DBL-8502", "systemctl"),
            Some(off.clone()),
        );
        assert!(!state.login_enabled(), "a failed enable reported enabled");
        assert!(state.login_error().is_some());

        let on = LoginStatus {
            enabled: true,
            location: off.location.clone(),
        };
        state.login = LoginState::Failed(
            UserFacingError::new("DBL-8503", "systemctl"),
            Some(on.clone()),
        );
        assert!(state.login_enabled(), "a failed disable reported disabled");

        state.login = LoginState::Known(on);
        assert!(state.login_enabled());
    }

    #[test]
    fn busy_keeps_the_last_confirmed_value() {
        let on = LoginStatus {
            enabled: true,
            location: std::path::PathBuf::from("/tmp/x"),
        };
        let mut state = SettingsState {
            login: LoginState::Known(on.clone()),
            ..SettingsState::default()
        };
        state.login = LoginState::Busy(Some(on));
        assert!(state.login_enabled(), "Busy must not flicker the switch");
        assert!(state.login_error().is_none());

        let unread = SettingsState {
            login: LoginState::Busy(None),
            ..SettingsState::default()
        };
        assert!(!unread.login_enabled());
        assert!(unread.login_known().is_none());
    }

    #[test]
    fn every_install_kind_has_a_label() {
        use dblocal_core::update::PackageManagerHint;
        let kinds = [
            InstallKind::AppImage {
                appimage_path: "/a.AppImage".into(),
            },
            InstallKind::PortableTarball {
                exe_path: "/opt/dblocal/dblocal".into(),
            },
            InstallKind::WindowsZip {
                exe_path: "C:\\dblocal.exe".into(),
            },
            InstallKind::WindowsMsi {
                exe_path: "C:\\dblocal.exe".into(),
            },
            InstallKind::MacApp {
                bundle_path: "/Applications/DBLocal.app".into(),
            },
            InstallKind::DistPackage {
                exe_path: "/usr/bin/dblocal".into(),
                hint: PackageManagerHint::Dpkg,
            },
            InstallKind::Homebrew {
                exe_path: "/opt/homebrew/bin/dblocal".into(),
            },
            InstallKind::Unknown {
                exe_path: "/x".into(),
                reason: "?".to_owned(),
            },
        ];
        for kind in kinds {
            assert!(!install_kind_label(&kind).is_empty());
        }
    }
}
