//! Pure state for the Settings dialog (`F-PKG-05`, `F-AUT-01`).
//!
//! Two sections, each with its own read model:
//!
//! - **Pembaruan**: the result of the last manual check. There is no timer and
//!   no startup check (`F-PKG-05` AC1 says "when the user opens Settings →
//!   Updates"); the dialog only ever shows what a click produced.
//! - **Jalankan saat login**: the login-item state, read on open and updated
//!   only by the switch. A failed enable never shows "enabled" (`F-AUT-01`
//!   AC1/AC2).
//!
//! Everything here is data; the service and the login-item seam are called from
//! the action handlers, off the GPUI thread.

use dblocal_core::service::{UpdateStatus, UserFacingError};

use super::login_item::LoginStatus;

/// The last update check, as the Pembaruan section renders it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateCheckState {
    /// No check was made yet.
    #[default]
    Idle,
    /// A check is in flight.
    Checking,
    /// The check completed; the status carries the answer and the install kind.
    Checked(UpdateStatus),
    /// The check failed with a typed error.
    Failed(UserFacingError),
}

/// The install action, once a check found a newer release.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InstallState {
    /// No install was requested.
    #[default]
    Idle,
    /// A download/apply is in flight.
    Installing,
    /// The update was applied; a restart is required.
    Installed,
    /// The install failed with a typed error.
    Failed(UserFacingError),
}

/// The login-item section, as rendered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoginState {
    /// The status was not read yet.
    #[default]
    Unknown,
    /// The status is being read or a write is in flight.
    Busy,
    /// The item is registered or not, and where it lives.
    Known(LoginStatus),
    /// The read or write failed; the switch never claims a state after this.
    Failed(UserFacingError),
}

/// The whole Settings dialog's data.
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    /// Whether the dialog is open.
    pub open: bool,
    /// The last manual update check.
    pub check: UpdateCheckState,
    /// The install action, if requested.
    pub install: InstallState,
    /// The login-item section.
    pub login: LoginState,
}

impl SettingsState {
    /// Opens the dialog with no stale answer: the caller re-reads on open.
    pub fn open(&mut self) {
        self.open = true;
        self.check = UpdateCheckState::Idle;
        self.install = InstallState::Idle;
        self.login = LoginState::Unknown;
    }

    /// Closes the dialog. A close during an install keeps the install state so
    /// the status bar can still report it; only the visibility flips.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Whether a check is in flight.
    pub fn is_checking(&self) -> bool {
        self.check == UpdateCheckState::Checking
    }

    /// Whether an install is in flight.
    pub fn is_installing(&self) -> bool {
        self.install == InstallState::Installing
    }

    /// The login state as a known value, when one was read.
    pub fn login_known(&self) -> Option<&LoginStatus> {
        match &self.login {
            LoginState::Known(status) => Some(status),
            _ => None,
        }
    }

    /// What the login switch shows: the *last confirmed* state.
    ///
    /// A failed enable reports `false`, never the state the user asked for
    /// (`F-AUT-01` AC2 — "do not show enabled after a failed enable").
    pub fn login_enabled(&self) -> bool {
        self.login_known().is_some_and(|status| status.enabled)
    }

    /// The typed error of the login section, when the last call failed.
    pub fn login_error(&self) -> Option<&UserFacingError> {
        match &self.login {
            LoginState::Failed(err) => Some(err),
            _ => None,
        }
    }

    /// The typed error of the check, when it failed.
    pub fn check_error(&self) -> Option<&UserFacingError> {
        match &self.check {
            UpdateCheckState::Failed(err) => Some(err),
            _ => None,
        }
    }

    /// The typed error of the install, when it failed.
    pub fn install_error(&self) -> Option<&UserFacingError> {
        match &self.install {
            InstallState::Failed(err) => Some(err),
            _ => None,
        }
    }

    /// The status of the last successful check.
    pub fn checked(&self) -> Option<&UpdateStatus> {
        match &self.check {
            UpdateCheckState::Checked(status) => Some(status),
            _ => None,
        }
    }
}

/// Whether the "Pasang" button may be offered: only for a found release on an
/// install kind that can self-update (`F-PKG-05` AC2).
pub fn can_install(state: &SettingsState) -> bool {
    match state.checked().and_then(|status| status.available.as_ref()) {
        Some(available) => available.can_self_update,
        None => false,
    }
}

/// The package-manager hint to show instead of "Pasang", when the service has
/// one for this install kind (`F-PKG-05` AC2).
pub fn package_manager_hint(state: &SettingsState) -> Option<&str> {
    state
        .checked()
        .and_then(|status| status.package_manager_hint.as_deref())
}

/// The line the Pembaruan section shows for the last check.
pub fn check_headline(state: &SettingsState) -> Option<String> {
    let status = state.checked()?;
    Some(match &status.available {
        Some(available) => dblocal_core::strings::settings_available(&available.version),
        None => dblocal_core::strings::UI_SETTINGS_UP_TO_DATE.to_owned(),
    })
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
        InstallKind::PortableTarball { .. } => "Tarball portabel",
        InstallKind::WindowsZip { .. } => "Zip Windows",
        InstallKind::WindowsMsi { .. } => "MSI Windows",
        InstallKind::MacApp { .. } => "Aplikasi macOS",
        InstallKind::DistPackage { .. } => "Paket distribusi",
        InstallKind::Homebrew { .. } => "Homebrew",
        InstallKind::Unknown { .. } => "Tidak dikenali",
        _ => "Tidak dikenali",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use dblocal_core::service::AvailableUpdateView;
    use dblocal_core::update::InstallKind;

    use super::*;

    fn available(can_self_update: bool) -> AvailableUpdateView {
        AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Catatan".to_owned(),
            asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 1024,
            can_self_update,
        }
    }

    fn status(available_update: Option<AvailableUpdateView>, kind: InstallKind) -> UpdateStatus {
        UpdateStatus {
            current: "0.8.0".to_owned(),
            available: available_update,
            package_manager_hint: kind
                .package_manager_hint()
                .map(dblocal_core::update::PackageManagerHint::message),
            install_kind: kind,
        }
    }

    fn portable() -> InstallKind {
        InstallKind::PortableTarball {
            exe_path: std::path::PathBuf::from("/opt/dblocal/dblocal"),
        }
    }

    fn distro() -> InstallKind {
        InstallKind::DistPackage {
            exe_path: std::path::PathBuf::from("/usr/bin/dblocal"),
            hint: dblocal_core::update::PackageManagerHint::Dpkg,
        }
    }

    #[test]
    fn opening_resets_every_previous_answer() {
        let mut state = SettingsState {
            open: false,
            check: UpdateCheckState::Failed(UserFacingError::new("DBL-8003", "offline")),
            install: InstallState::Installed,
            login: LoginState::Failed(UserFacingError::new("DBL-8504", "no")),
        };
        state.open();
        assert!(state.open);
        assert_eq!(state.check, UpdateCheckState::Idle);
        assert_eq!(state.install, InstallState::Idle);
        assert_eq!(state.login, LoginState::Unknown);
    }

    #[test]
    fn installing_offers_the_button_only_for_a_self_updatable_install() {
        let mut state = SettingsState::default();
        state.check = UpdateCheckState::Checked(status(Some(available(true)), portable()));
        assert!(can_install(&state));
        assert!(package_manager_hint(&state).is_none());

        state.check = UpdateCheckState::Checked(status(Some(available(false)), distro()));
        assert!(
            !can_install(&state),
            "a distro install must not self-update"
        );
        assert!(
            package_manager_hint(&state).is_some(),
            "the hint must replace the install button"
        );
    }

    #[test]
    fn no_release_means_no_install_and_an_up_to_date_line() {
        let mut state = SettingsState::default();
        state.check = UpdateCheckState::Checked(status(None, portable()));
        assert!(!can_install(&state));
        assert_eq!(
            check_headline(&state).as_deref(),
            Some(dblocal_core::strings::UI_SETTINGS_UP_TO_DATE)
        );
    }

    #[test]
    fn a_found_release_names_the_version() {
        let mut state = SettingsState::default();
        state.check = UpdateCheckState::Checked(status(Some(available(true)), portable()));
        assert_eq!(
            check_headline(&state).as_deref(),
            Some("Versi 0.9.0 tersedia.")
        );
    }

    #[test]
    fn a_failed_enable_never_shows_enabled() {
        let mut state = SettingsState::default();
        state.login = LoginState::Known(LoginStatus {
            enabled: false,
            location: std::path::PathBuf::from("/tmp/x"),
        });
        assert!(!state.login_enabled());

        // The user flips the switch; the write fails.
        state.login = LoginState::Failed(UserFacingError::new("DBL-8502", "systemctl"));
        assert!(!state.login_enabled(), "a failed enable reported enabled");
        assert!(state.login_error().is_some());

        // A successful enable then shows the confirmed state.
        state.login = LoginState::Known(LoginStatus {
            enabled: true,
            location: std::path::PathBuf::from("/tmp/x"),
        });
        assert!(state.login_enabled());
    }

    #[test]
    fn busy_flags_follow_the_in_flight_states() {
        let mut state = SettingsState::default();
        state.check = UpdateCheckState::Checking;
        assert!(state.is_checking());
        state.check = UpdateCheckState::Idle;
        assert!(!state.is_checking());

        state.install = InstallState::Installing;
        assert!(state.is_installing());
    }

    #[test]
    fn every_install_kind_has_a_label() {
        use dblocal_core::update::PackageManagerHint;
        let kinds = [
            InstallKind::AppImage {
                appimage_path: "/a.AppImage".into(),
            },
            portable(),
            InstallKind::WindowsZip {
                exe_path: "C:\\dblocal.exe".into(),
            },
            InstallKind::WindowsMsi {
                exe_path: "C:\\dblocal.exe".into(),
            },
            InstallKind::MacApp {
                bundle_path: "/Applications/DBLocal.app".into(),
            },
            distro(),
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
        assert_eq!(install_kind_label(&distro()), "Paket distribusi");
        assert_eq!(
            install_kind_label(&InstallKind::Homebrew {
                exe_path: "/x".into()
            }),
            "Homebrew"
        );
        let _ = PackageManagerHint::Rpm;
    }
}
