//! The event reduction into the workspace (`W1-G`; the Databases and Config
//! invalidations are `W4-B`/`W4-H`).
//!
//! `apply_event` folds a [`UiEvent`] into the pure `ShellState` and the
//! per-screen caches. `apply_event_with_cx` then runs the side effects that need
//! a context: a tray refresh, a re-read, a pending restart. Splitting the
//! reduction out of `app.rs` keeps that file under its size cap; the behaviour is
//! unchanged.

use dblocal_core::model::InstanceId;
use dblocal_core::strings;
use gpui_kit::{Context, SharedString};

use super::Workspace;
use super::admin_state;
use crate::service_bridge::UiEvent;

impl Workspace {
    /// Folds an event into the pure state and the per-screen caches.
    pub(crate) fn apply_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::InstanceChanged(view) => {
                self.state.upsert_instance(view);
                self.tray_needs_refresh = true;
            }
            UiEvent::OperationProgress { id, stage } => self.state.on_progress(&id, stage),
            UiEvent::DownloadProgress { id, progress } => {
                self.state
                    .on_download(id, progress, strings::UI_DOWNLOADS.to_owned());
            }
            UiEvent::OperationFinished { id, result } => {
                self.state.on_finished(&id, result.clone());
                if let Err(err) = &result {
                    self.toast = Some(SharedString::from(format!(
                        "{} — {}",
                        err.code, err.message
                    )));
                    self.last_failure_mirror = Some(err.clone());
                }
                // Quit-stop completion is driven by the finished operation, never
                // by request acceptance. The actual state transition is applied in
                // `apply_event_with_cx` where we have `cx`.
            }
            UiEvent::CatalogUpdated => {}
            UiEvent::LogLines { instance, lines } => {
                if let Ok(id) = instance.parse::<InstanceId>() {
                    self.state.on_log_lines(&mut self.logs, &id, lines);
                    if self.visible_logs_following(&id) {
                        self.log_scroll.scroll_to_bottom();
                    }
                }
            }
            UiEvent::InstanceRemoved(id) => {
                self.state
                    .apply_removed(&id, &mut self.tabs, &mut self.logs);
                self.admin.forget(&id);
                self.config.forget(&id);
                self.forget_config_fields(&id);
                self.forget_instance(&id);
                self.tray_needs_refresh = true;
            }
            UiEvent::AdminChanged(id) => {
                // The rows are stale. The visible instance is re-read below
                // (`apply_event_with_cx`); any other instance goes back to
                // `Idle`, so opening its tab issues a fresh read instead of
                // showing rows that changed while it was in the background.
                if let Some(admin) = self.admin.get(&id).cloned() {
                    let mut admin = admin;
                    if admin.data.ready().is_some() {
                        admin.data = admin_state::Loadable::Idle;
                        *self.admin.entry(&id) = admin;
                    }
                }
            }
            UiEvent::ConfigChanged(id) => {
                self.invalidate_config(&id);
            }
            UiEvent::StopForced(_id) => {
                self.toast = Some(SharedString::from(strings::UI_TOAST_STOP_FORCED));
            }
            UiEvent::UpdateChecked { .. } => {
                // `check_update` returns the full status to the caller, so the
                // dialog already has it; the event only exists for consumers
                // that poll nothing, and a stray one must not overwrite a check
                // the dialog is showing with a partial answer.
            }
            UiEvent::Resync => {}
            UiEvent::Closed => {}
        }
    }

    /// Sends a background instance's Config view back to `Idle` on a
    /// `ConfigChanged`, so opening its tab re-reads instead of showing a stale
    /// file. The visible tab is re-read (and its edits re-based) in
    /// `apply_event_with_cx`.
    fn invalidate_config(&mut self, id: &InstanceId) {
        if self
            .config
            .get(id)
            .is_some_and(|config| config.view().is_some())
        {
            self.config.entry(id).data = admin_state::Loadable::Idle;
        }
    }

    /// Applies an event and then runs the side effects that need a context.
    pub(crate) fn apply_event_with_cx(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        let needs_resync = matches!(event, UiEvent::Resync);
        let needs_catalog = matches!(event, UiEvent::CatalogUpdated);
        let removed_id = match &event {
            UiEvent::InstanceRemoved(id) => Some(id.clone()),
            _ => None,
        };
        let admin_changed = match &event {
            UiEvent::AdminChanged(id) => Some(id.clone()),
            _ => None,
        };
        let config_changed = match &event {
            UiEvent::ConfigChanged(id) => Some(id.clone()),
            _ => None,
        };
        // F1: a start or stop changes whether the Databases tab can be read at
        // all, so the rows read before the transition are stale even though no
        // `AdminChanged` was emitted. Record the transition here, where the
        // status is still visible before `apply_event` overwrites it.
        let status_changed = match &event {
            UiEvent::InstanceChanged(view) => self
                .instance(&view.id)
                .map(|current| current.status != view.status)
                .unwrap_or(false)
                .then(|| view.id.clone()),
            _ => None,
        };
        let quit_stop_finished = match &event {
            UiEvent::OperationFinished { id, result } => Some((id.clone(), result.is_ok())),
            _ => None,
        };
        // `F2`: the stop a pending "Restart sekarang" queued failed, so that
        // restart can never happen. Drop it here — matching the operation id,
        // never clearing on an unrelated operation — instead of leaving it to
        // fire on the next manual stop.
        if let UiEvent::OperationFinished { id, result: Err(_) } = &event
            && self.config_restart_stop_op.as_ref() == Some(id)
        {
            self.config_restart_after_stop = None;
            self.config_restart_stop_op = None;
        }
        let finished = quit_stop_finished.is_some();
        let removed = removed_id.is_some();
        self.apply_event(event);
        if self.tray_needs_refresh {
            self.tray_needs_refresh = false;
            self.refresh_tray(cx);
        }
        if let Some((op, ok)) = quit_stop_finished {
            self.on_quit_stop_finished(&op, ok, cx);
        }
        if let Some(id) = removed_id {
            let service = self.service.clone();
            self.bg(cx, move || {
                let _ = service.watch_logs(&id, false);
                move |_: &mut Workspace, _: &mut Context<Workspace>| {}
            });
        }
        if needs_catalog {
            self.reload_catalog_bg(cx);
        }
        if let Some(id) = status_changed {
            self.after_status_change(&id, cx);
        }
        if let Some(id) = config_changed {
            // The file changed (another window, the CLI, a restore). Re-read only
            // the visible instance; a background one went back to `Idle` in
            // `apply_event` and is read on entry.
            if self.config_tab_visible(&id) {
                self.reload_config_bg(&id, cx);
            }
        }
        if let Some(id) = admin_changed {
            // Re-read only the instance whose Databases tab is on screen; an
            // event for a background instance just clears its "ready" state in
            // `apply_event`, and it is read again when the user opens it.
            if self.database_tab_visible(&id) {
                self.reload_admin_bg(&id, cx);
            }
        }
        if needs_resync || finished {
            self.refresh_instances_bg(cx);
        }
        if finished || removed {
            self.refresh_home_size_bg(cx);
        }
    }

    /// After a start/stop: invalidate the Databases rows, run a pending
    /// "Restart sekarang" once the stop has settled, and retire the "Tersimpan"
    /// banner once the change is in effect (`F4`).
    fn after_status_change(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        use dblocal_core::model::InstanceStatus;
        self.admin.invalidate(id);
        if self.database_tab_visible(id) {
            self.reload_admin_bg(id, cx);
        }
        let status = self.instance(id).map(|v| v.status);
        // A Config apply's "Restart sekarang" waits for the stop to settle:
        // starting a `Stopping` instance would be refused, so the start runs
        // here, on the transition to `Stopped`, and only for the stop it queued
        // (`F-CFG-01` AC2, `F2`).
        if self.config_restart_after_stop.as_ref() == Some(id) {
            match status {
                Some(InstanceStatus::Stopped) => {
                    self.config_restart_after_stop = None;
                    self.config_restart_stop_op = None;
                    self.start_instance(id.clone(), cx);
                }
                // The instance left `Stopping` for anything but `Stopped`; this
                // stop will never complete the restart. Drop it rather than let
                // the next manual stop start the instance by itself.
                Some(InstanceStatus::Running | InstanceStatus::Failed)
                    if self.config_restart_stop_op.is_some() =>
                {
                    self.clear_pending_restart_for(id);
                }
                _ => {}
            }
        }
        // `F4`: once the instance is running again the applied config is in
        // effect, so the "Tersimpan. Berlaku setelah restart." banner is stale.
        // The stop half of our own queued restart keeps it, so the banner does
        // not flicker between stop and start.
        if status == Some(InstanceStatus::Running) {
            self.config.entry(id).applied = None;
        }
    }
}
