//! Action handlers for the in-app update (`F-PKG-05`).
//!
//! Every call that can block — the GitHub check, the start, the cancel, the
//! restart and the Settings auto-check read/write — runs on the GPUI background
//! executor through [`Workspace::bg`](super::background), so a slow network
//! never freezes a frame.
//!
//! The title-bar icon and the update dialog both funnel through here, so a
//! second press while one update runs is a no-op and can never start a second.

use dblocal_core::strings;
use gpui_kit::{Context, Window};

use super::Workspace;
use super::update_state::{UpdatePhase, UpdateState};

impl Workspace {
    /// The update read model, for the views and the tests.
    pub(crate) fn update_state(&self) -> &UpdateState {
        &self.update
    }

    /// Starts the update check loop: once now, then about every 6 hours.
    ///
    /// The interval comes from the same constant the service uses for its cache,
    /// so the two never disagree. The check itself runs off the GPUI thread and
    /// is quiet when offline or rate limited.
    pub(crate) fn start_update_check_loop(&mut self, cx: &mut Context<Self>) {
        if self._update_check_task.is_some() {
            return;
        }
        self.check_update_bg(cx);
        let interval =
            std::time::Duration::from_secs(dblocal_core::consts::UPDATE_CHECK_INTERVAL_SECS);
        self._update_check_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let alive = this.update(cx, |this, cx| this.check_update_bg(cx)).is_ok();
                if !alive {
                    return;
                }
            }
        }));
    }

    /// The startup and periodic check: cached, quiet when offline, and never on
    /// the GPUI thread.
    pub(crate) fn check_update_bg(&mut self, cx: &mut Context<Self>) {
        if self.update_check_in_flight {
            return;
        }
        self.update_check_in_flight = true;
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.check_update_if_due();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.update_check_in_flight = false;
                let was_visible = this.update.dialog_visible();
                match result {
                    Ok(Some(status)) => this.update.apply_check(Some(&status)),
                    Ok(None) => {
                        if this.update.phase == UpdatePhase::Checking {
                            this.check_update_now(cx);
                            return;
                        }
                    }
                    Err(err) => {
                        // A quiet check must not pop up; a real failure is logged
                        // and the icon simply stays as it was, unless the user is
                        // actively waiting on a check in the dialog.
                        tracing::debug!(
                            code = %err.code,
                            message = %err.message,
                            "background update check failed"
                        );
                        if this.update.phase == UpdatePhase::Checking {
                            this.update.phase = UpdatePhase::Failed {
                                code: err.code,
                                message: err.message,
                            };
                        }
                    }
                }
                if was_visible && !this.update.dialog_visible() {
                    this.restore_dialog_focus_after_close();
                }
                cx.notify();
            }
        });
    }

    /// The manual "Check now": bypasses the cache and surfaces a typed failure.
    pub(crate) fn check_update_now(&mut self, cx: &mut Context<Self>) {
        if self.update_check_in_flight {
            return;
        }
        self.update_check_in_flight = true;
        self.update.checking = true;
        self.update.check_error = None;
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.check_update();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.update_check_in_flight = false;
                this.update.checking = false;
                let was_visible = this.update.dialog_visible();
                match result {
                    Ok(status) => this.update.apply_check(Some(&status)),
                    Err(err) => {
                        this.update.check_error = Some(err.clone());
                        if this.update.phase == UpdatePhase::Checking {
                            this.update.phase = UpdatePhase::Failed {
                                code: err.code,
                                message: err.message,
                            };
                        }
                    }
                }
                if was_visible && !this.update.dialog_visible() {
                    this.restore_dialog_focus_after_close();
                }
                cx.notify();
            }
        });
    }

    /// The title-bar button: one press starts the update and shows progress.
    ///
    /// A managed install cannot replace itself, so pressing it only opens the
    /// dialog that explains why and offers the release page. A running, ready
    /// or stopped update opens the dialog at its current stage instead of
    /// starting another.
    pub(crate) fn press_update_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.update.icon_visible() {
            return;
        }
        // The dialog is the update's only surface, so it takes the keyboard
        // (`F1`/`F2`) whether the press starts the download or just shows it.
        self.update.dialog_open = true;
        self.begin_modal(window, cx);
        self.focus_modal_root();
        if matches!(self.update.phase, UpdatePhase::Available) {
            self.start_update(cx);
        }
        cx.notify();
    }

    /// Starts the update the check found. Refuses while one runs or is ready.
    pub(crate) fn start_update(&mut self, cx: &mut Context<Self>) {
        if self.update.blocks_new_update() || self.update.available.is_none() {
            return;
        }
        self.update.check_error = None;
        self.update.dismiss_previous_failure();
        if !self
            .update
            .available
            .as_ref()
            .is_some_and(|v| v.can_self_update)
        {
            self.update.phase = UpdatePhase::CannotSelfUpdate;
            self.update.dialog_open = true;
            cx.notify();
            return;
        }
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.start_update();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(op) => {
                        // The first `UpdateProgress` also opens the dialog; the
                        // snapshot here means one never waits on the event pump.
                        this.update.begin_operation(op);
                    }
                    Err(err) => {
                        this.update.phase = UpdatePhase::Failed {
                            code: err.code.clone(),
                            message: err.message.clone(),
                        };
                        this.update.dialog_open = true;
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Cancels the download. Offered only while downloading.
    pub(crate) fn cancel_update(&mut self, cx: &mut Context<Self>) {
        let Some(op) = self.update.operation.clone() else {
            return;
        };
        if !self.update.can_cancel() {
            return;
        }
        let service = self.service.clone();
        self.bg(cx, move || {
            let _ = service.cancel_operation(&op);
            move |_: &mut Workspace, _: &mut Context<Workspace>| {}
        });
    }

    /// Retries a stopped attempt, or starts an available one.
    pub(crate) fn retry_update(&mut self, cx: &mut Context<Self>) {
        if self.update.running() {
            return;
        }
        // A previous run's failure has no release attached: checking again is
        // what a retry can honestly do, keeping the dialog visible through the
        // check so the user always sees an outcome (checking, up to date, a
        // newer release, or the check error).
        if self.update.available.is_none() {
            self.update.dismiss_previous_failure();
            self.update.phase = UpdatePhase::Checking;
            self.update.dialog_open = true;
            self.check_update_now(cx);
            cx.notify();
            return;
        }
        // A retry is a fresh attempt at the same release; the service re-checks
        // its cache and refuses a second run at the lock.
        self.start_update(cx);
    }

    /// Reads the failure a previous run persisted (`take_pending_update_failure`)
    /// and shows it in the update dialog.
    ///
    /// Runs once at startup, off the GPUI thread: the helper writes the file
    /// after the old process exited, so nothing in this session ever saw the
    /// failure. A quiet read is a no-op.
    pub(crate) fn read_pending_update_failure(&mut self, cx: &mut Context<Self>) {
        if self.pending_failure_read {
            return;
        }
        self.pending_failure_read = true;
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.take_pending_update_failure();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(Some(message)) => {
                        this.update.record_previous_failure(message);
                        this.begin_modal_without_window(cx);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        tracing::debug!(%err, "could not read the previous update failure");
                    }
                }
                cx.notify();
            }
        });
    }

    /// Restarts into the installed version, then quits without touching any
    /// running database server.
    ///
    /// The restart spawns the new binary detached; the quit path is the app's
    /// `Exit`, not a stop-all, so nothing signals an engine.
    pub(crate) fn restart_into_update(&mut self, cx: &mut Context<Self>) {
        if !self.update.can_restart() {
            return;
        }
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.restart_for_update();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| match result {
                Ok(()) => {
                    this.quit_stops.clear();
                    this.quit_timeout_task = None;
                    crate::app::bootstrap::request_app_quit(cx);
                }
                Err(err) => {
                    // The files are installed; only the relaunch failed. Keep
                    // the `Ready` phase so "Restart now" retries the launch
                    // instead of downloading the release a second time.
                    this.update.restart_error = Some(err.clone());
                    this.update.dialog_open = true;
                    this.report_user_facing_error(&err);
                    cx.notify();
                }
            }
        });
    }

    /// The manual "Restart now", after an automatic restart failed.
    pub(crate) fn retry_restart(&mut self, cx: &mut Context<Self>) {
        if !self.update.can_restart() {
            return;
        }
        self.update.restart_error = None;
        self.restart_into_update(cx);
    }

    /// Closes the update dialog without changing the update state.
    ///
    /// Closing a previous-run failure is what dismisses it: it is a one-time
    /// note, not a permanent banner.
    pub(crate) fn close_update_dialog(&mut self, cx: &mut Context<Self>) {
        self.update.dismiss_dialog();
        self.restore_dialog_focus_after_close();
        cx.notify();
    }

    /// Opens the release page in the OS default handler.
    pub(crate) fn open_update_release_page(&mut self, cx: &mut Context<Self>) {
        // With no newer release known (a previous run's failure), the releases
        // page is still the right place to look, so the action never no-ops.
        let url = self
            .update
            .release_url()
            .map(str::to_owned)
            .unwrap_or_else(|| self.update.releases_page());
        let launcher: std::sync::Arc<dyn dblocal_core::config::OpenLauncher> =
            std::sync::Arc::new(dblocal_core::config::SystemOpenLauncher);
        self.bg(cx, move || {
            let command = dblocal_core::config::OpenCommand {
                program: release_page_program(),
                args: release_page_args(&url),
            };
            let result = launcher.launch(&command);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Err(err) = result {
                    this.toast = Some(gpui_kit::SharedString::from(format!(
                        "{}: {err}",
                        strings::UI_UPDATE_RELEASE_PAGE
                    )));
                    cx.notify();
                }
            }
        });
    }

    /// Reads the automatic-check setting, once, for the Settings switch.
    pub(crate) fn read_auto_update_check(&mut self, cx: &mut Context<Self>) {
        if self.update.auto_check.is_some() || self.update.auto_check_busy {
            return;
        }
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.auto_update_check_enabled();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Ok(enabled) = result {
                    this.update.auto_check = Some(enabled);
                }
                cx.notify();
            }
        });
    }

    /// Writes the automatic-check setting. The switch keeps the confirmed value
    /// while the write runs and after it fails.
    pub(crate) fn set_auto_update_check(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.update.auto_check_busy {
            return;
        }
        let confirmed = self.update.auto_check;
        self.update.auto_check_busy = true;
        self.update.auto_check_error = None;
        let service = self.service.clone();
        self.bg(cx, move || {
            let result = service.set_auto_update_check(enabled);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.update.auto_check_busy = false;
                match result {
                    Ok(()) => {
                        this.update.auto_check = Some(enabled);
                        // Turning it on must take effect now, not in six hours.
                        if enabled {
                            this.check_update_bg(cx);
                        }
                    }
                    Err(err) => {
                        this.update.auto_check = confirmed;
                        this.update.auto_check_error = Some(err.user_message());
                    }
                }
                cx.notify();
            }
        });
    }
}

/// The programme that opens a URL in the OS default browser.
fn release_page_program() -> std::path::PathBuf {
    match dblocal_core::platform::Os::current() {
        dblocal_core::platform::Os::Macos => std::path::PathBuf::from("open"),
        dblocal_core::platform::Os::Windows => std::path::PathBuf::from("rundll32.exe"),
        _ => std::path::PathBuf::from("xdg-open"),
    }
}

/// The argv that opens `url`, one URL per element (never a shell string).
fn release_page_args(url: &str) -> Vec<String> {
    release_page_args_for(dblocal_core::platform::Os::current(), url)
}

fn release_page_args_for(os: dblocal_core::platform::Os, url: &str) -> Vec<String> {
    match os {
        dblocal_core::platform::Os::Windows => {
            vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()]
        }
        _ => vec![url.to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dblocal_core::platform::Os;

    /// The release page always goes to the browser as exactly one argument, so
    /// a URL cannot become a second argv element.
    #[test]
    fn the_release_page_argv_keeps_the_url_whole() {
        let url = "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0";
        assert_eq!(release_page_args_for(Os::Linux, url), vec![url.to_owned()]);
        assert_eq!(release_page_args_for(Os::Macos, url), vec![url.to_owned()]);
        assert_eq!(
            release_page_args_for(Os::Windows, url),
            vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()]
        );
        let expected_current = release_page_args_for(Os::current(), url);
        assert_eq!(release_page_args(url), expected_current);
    }
}
