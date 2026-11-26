//! Action handlers for the Settings dialog (`F-PKG-05`, `F-AUT-01`).
//!
//! Every call that can block — the GitHub release check, the update, the
//! login-item backend (`systemctl --user` / `launchctl` / `reg`) — runs on the
//! GPUI background executor through [`Workspace::bg`](super::background), so a
//! slow network or a hung helper never freezes a frame.
//!
//! The Updates section renders the one in-app update read model
//! ([`super::update_state::UpdateState`]); its check and update actions live in
//! `update_actions`, so the title-bar icon and Settings share one source.

use std::sync::Arc;

use dblocal_core::service::UserFacingError;
use dblocal_core::strings;
use gpui_kit::{Context, Window};

use super::Workspace;
use super::login_item::{HostLoginItem, LoginItemControl};
use super::settings_state::{LoginState, SettingsState};

impl Workspace {
    /// Opens the dialog and reads the login-item state (never the network).
    ///
    /// `F1`/`F2`: captures the opener, makes the modal container and gives the
    /// keyboard to it, so a dialog of only buttons takes focus without a click.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.open();
        self.begin_modal(window, cx);
        self.focus_modal_root();
        self.read_login_status(cx);
        self.read_auto_update_check(cx);
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

    /// The Settings state, for the view and the tests.
    pub(crate) fn settings_state(&self) -> &SettingsState {
        &self.settings
    }
}
