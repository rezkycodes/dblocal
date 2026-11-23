//! Action handlers for the Settings dialog (`F-PKG-05`, `F-AUT-01`).
//!
//! Every call that can block — the GitHub release check, the install, the
//! login-item backend (`systemctl --user` / `launchctl` / `reg`) — runs on the
//! GPUI background executor through [`Workspace::bg`](super::background), so a
//! slow network or a hung helper never freezes a frame.
//!
//! There is no timer and no check at startup: `open_settings` reads the login
//! item (local, instant) and leaves the update section in `Idle` until the user
//! presses "Periksa sekarang" (`F-PKG-05` AC1).

use std::sync::Arc;

use dblocal_core::service::UserFacingError;
use dblocal_core::strings;
use gpui_kit::Context;

use super::Workspace;
use super::login_item::{HostLoginItem, LoginItemControl};
use super::settings_state::{InstallState, LoginState, SettingsState, UpdateCheckState};

impl Workspace {
    /// Opens the dialog and reads the login-item state (never the network).
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.open();
        self.read_login_status(cx);
        cx.notify();
    }

    /// Closes the dialog; an in-flight install keeps its state.
    ///
    /// `F1`: closing returns the keyboard (opener, or the root fallback).
    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.close();
        self.restore_dialog_focus_after_close();
        cx.notify();
    }

    /// The login-item seam, built once and reused.
    ///
    /// A host with no usable backend yields the typed reason, which the dialog
    /// shows instead of a switch that cannot work.
    pub(crate) fn login_item(&mut self) -> Result<Arc<dyn LoginItemControl>, UserFacingError> {
        if let Some(seam) = &self.login {
            return seam.clone();
        }
        let built = HostLoginItem::new().map(|host| Arc::new(host) as Arc<dyn LoginItemControl>);
        self.login = Some(built.clone());
        built
    }

    /// Reads the login-item state on the background executor.
    pub(crate) fn read_login_status(&mut self, cx: &mut Context<Self>) {
        let seam = match self.login_item() {
            Ok(seam) => seam,
            Err(err) => {
                self.settings.login = LoginState::Failed(err, None);
                return;
            }
        };
        self.settings.login = LoginState::Busy(self.settings.login_known().cloned());
        self.bg(cx, move || {
            let status = seam.status();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.settings.login = match status {
                    Ok(status) => LoginState::Known(status),
                    Err(err) => LoginState::Failed(err, None),
                };
                cx.notify();
            }
        });
    }

    /// Enables or disables the login item and records only the confirmed state.
    ///
    /// The switch keeps the last confirmed status through `Busy` and `Failed`:
    /// a failed disable leaves the unit enabled, so flipping the switch off
    /// would show a state the machine is not in (F2).
    pub(crate) fn set_login_item(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let seam = match self.login_item() {
            Ok(seam) => seam,
            Err(err) => {
                let confirmed = self.settings.login_known().cloned();
                self.settings.login = LoginState::Failed(err, confirmed);
                cx.notify();
                return;
            }
        };
        let confirmed = self.settings.login_known().cloned();
        self.settings.login = LoginState::Busy(confirmed.clone());
        self.bg(cx, move || {
            let status = if enabled {
                seam.enable()
            } else {
                seam.disable()
            };
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match status {
                    Ok(status) => {
                        this.settings.login = LoginState::Known(status);
                        this.toast = Some(gpui_kit::SharedString::from(if enabled {
                            strings::UI_SETTINGS_AUTOSTART_ON
                        } else {
                            strings::UI_SETTINGS_AUTOSTART_OFF
                        }));
                    }
                    Err(err) => {
                        // A failed write keeps the last confirmed state; the
                        // typed error explains what went wrong.
                        this.settings.login = LoginState::Failed(err.clone(), confirmed);
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Runs one manual update check (`F-PKG-05` AC1).
    pub(crate) fn check_updates(&mut self, cx: &mut Context<Self>) {
        if self.settings.is_checking() {
            return;
        }
        self.settings.check = UpdateCheckState::Checking;
        self.settings.install = InstallState::Idle;
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.check_update();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.settings.check = match result {
                    Ok(status) => UpdateCheckState::Checked(status),
                    Err(err) => UpdateCheckState::Failed(err),
                };
                cx.notify();
            }
        });
    }

    /// Applies the update the last check found (`F-PKG-05`).
    pub(crate) fn install_update(&mut self, cx: &mut Context<Self>) {
        if self.settings.is_installing() || !super::settings_state::can_install(&self.settings) {
            return;
        }
        self.settings.install = InstallState::Installing;
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.apply_update();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.settings.install = match result {
                    Ok(()) => {
                        this.toast =
                            Some(gpui_kit::SharedString::from(strings::UI_SETTINGS_INSTALLED));
                        InstallState::Installed
                    }
                    Err(err) => {
                        this.report_user_facing_error(&err);
                        InstallState::Failed(err)
                    }
                };
                cx.notify();
            }
        });
    }

    /// The Settings state, for the view and the tests.
    pub(crate) fn settings_state(&self) -> &SettingsState {
        &self.settings
    }
}
