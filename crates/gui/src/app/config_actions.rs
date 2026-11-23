//! Action handlers for the Config tab (`F-CFG-01`, `F-CFG-02`).
//!
//! Every `DblService` call here goes through [`Workspace::bg`](super::background):
//! a config read touches the file and a apply/restore takes the per-instance
//! guard, so none of them may run on the GPUI thread. A failure lands on the UI
//! thread as a typed error the card renders; nothing panics and nothing is only a
//! toast (`DBL-8603`, `DBL-8604`, `DBL-8607`, `DBL-8608`).
//!
//! A restart after an apply is the documented pair (`stop_instance` then
//! `start_instance`), not a new service call (`F-CFG-01` AC2): the apply only
//! schedules it, and the start runs when the stop's `OperationFinished` arrives,
//! because starting a `Stopping` instance would be refused.

use dblocal_core::model::InstanceId;
use dblocal_core::service::UserFacingError;
use dblocal_core::strings;
use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Window};

use super::Workspace;
use super::config_state::ConfigAction;

/// What a restore does after the file is back: nothing, or a start.
///
/// A `bool` would read backwards at the call site (`true` = start?); the two
/// variants say exactly which service call follows the restore (`F1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreFollowUp {
    /// "Pulihkan konfigurasi sebelumnya" from the dialog: only the file changes.
    FileOnly,
    /// The failure banner's "Pulihkan konfigurasi dan mulai ulang": restore, start.
    Start,
}

impl Workspace {
    /// Reads the config view for one instance, off the GPUI thread.
    pub(crate) fn reload_config_bg(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.instance(id).is_none() {
            return;
        }
        let service = self.service.clone();
        let iid = id.clone();
        // Keep the last good rows visible while the read is in flight: a blank
        // form on every refresh drops the focus of the field being typed in and
        // flickers the whole card (`F7b`).
        self.config.entry(id).begin_refresh();
        self.bg(cx, move || {
            let view = service.config_view(&iid);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let config = this.config.entry(&iid);
                match view {
                    Ok(view) => {
                        // A re-read that lands while there are edits re-bases
                        // them, exactly as a `ConfigChanged` would.
                        config.end_refresh_ok(view);
                        // The fields are (re)built by `Render::render` from the
                        // pure state; only the text needs re-syncing here.
                        this.sync_config_fields(&iid, cx);
                    }
                    Err(err) => config.end_refresh_failed(err),
                }
                cx.notify();
            }
        });
    }

    /// Reads the tab on entry: every time, so an edit by the CLI, a second
    /// DBLocal or the external editor is seen (`F7a`).
    ///
    /// Re-reading unconditionally is what makes "Buka file konfigurasi" useful:
    /// the whole point of that button is that something else changes the file.
    pub(crate) fn enter_config_tab(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.instance(id).is_none() {
            return;
        }
        self.reload_config_bg(id, cx);
    }

    /// Re-reads the visible Config tab when the window becomes active again, so
    /// an edit made while another window was focused is visible on return
    /// (`F7a`).
    pub(crate) fn refresh_config_on_activation(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.tabs.selected_instance().cloned() else {
            return;
        };
        if self.config_tab_visible(&id) {
            self.reload_config_bg(&id, cx);
        }
    }

    /// Synchronous config read for previews only (`new_preview` seeds a frame and
    /// starts no background work). Never on a user path; production reads go
    /// through [`Workspace::reload_config_bg`].
    pub(crate) fn load_config_sync(&mut self, _cx: &mut Context<Self>) {
        let Some(id) = self.tabs.selected_instance().cloned() else {
            return;
        };
        let Ok(view) = self.service.config_view(&id) else {
            return;
        };
        self.config.entry(&id).data = super::admin_state::Loadable::Ready(view);
    }

    /// "Muat ulang": re-reads after a failure.
    ///
    /// It is itself the user's dismissal of whatever card banner was showing, so
    /// a failed apply/restore/open error goes away here too (`F10`).
    pub(crate) fn retry_config(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.config.entry(id).dismiss_card_error();
        self.reload_config_bg(id, cx);
        cx.notify();
    }

    /// Builds the field entity of every editable setting that has none yet.
    ///
    /// Called from `Render::render`, where both a window and `&mut self` exist:
    /// the tab can be reached by the tab bar, a keyboard action or a restored
    /// inner tab, and every one of those paths needs a field that exists. The
    /// entity is created once and its Change/Enter subscription installed here.
    pub(crate) fn ensure_config_fields(
        &mut self,
        id: &InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: Vec<(String, String, String)> = self
            .config
            .get(id)
            .and_then(|config| config.view().map(|view| (config, view)))
            .map(|(config, view)| {
                view.settings
                    .iter()
                    .filter(|s| !s.readonly)
                    .map(|s| (s.key.clone(), config.field_text(s), s.default.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (key, initial, placeholder) in keys {
            if self.config_fields.contains_key(&(id.clone(), key.clone())) {
                continue;
            }
            let field = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(initial)
            });
            let iid = id.clone();
            let field_key = key.clone();
            cx.subscribe(
                &field,
                move |this, input, event: &gpui_kit::component::input::InputEvent, cx| {
                    use gpui_kit::component::input::InputEvent;
                    match event {
                        InputEvent::Change => {
                            let text = input.read(cx).value().to_string();
                            this.record_config_field(&iid, &field_key, &text, cx);
                        }
                        InputEvent::PressEnter { .. } => {
                            // Enter applies only when "Terapkan" is enabled.
                            this.apply_config_if_ready(&iid, cx);
                        }
                        _ => {}
                    }
                },
            )
            .detach();
            self.config_fields.insert((id.clone(), key), field);
        }
    }

    /// Sets every existing field's text to what the pure state shows (after a
    /// reset, a re-base or a retry, where the pure state is the source).
    fn sync_config_fields(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let pairs: Vec<(String, String)> = self
            .config
            .get(id)
            .and_then(|config| config.view().map(|view| (config, view)))
            .map(|(config, view)| {
                view.settings
                    .iter()
                    .filter(|s| !s.readonly)
                    .map(|s| (s.key.clone(), config.field_text(s)))
                    .collect()
            })
            .unwrap_or_default();
        let entity_id = cx.entity().entity_id();
        let mut updates: Vec<(Entity<InputState>, String)> = Vec::new();
        for (key, text) in pairs {
            let Some(field) = self.config_fields.get(&(id.clone(), key)).cloned() else {
                continue;
            };
            // Only rewrite a field whose text the reset/re-base changed, so a
            // focused field the user is typing in is never clobbered mid-edit.
            if field.read(cx).value().as_ref() == text {
                continue;
            }
            updates.push((field, text));
        }
        if updates.is_empty() {
            return;
        }
        cx.with_window(entity_id, |window, cx| {
            for (field, text) in updates {
                field.update(cx, |state, cx| state.set_value(text, window, cx));
            }
        });
    }

    /// Stores one typed field value in the pure state, validating locally.
    pub(crate) fn record_config_field(
        &mut self,
        id: &InstanceId,
        key: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let engine = self.instance(id).map(|view| view.engine);
        let config = self.config.entry(id);
        config.set_field(key, text);
        if let Some(engine) = engine {
            config.validate(engine);
        }
        cx.notify();
    }

    /// Drops the local edits and restores each field to the file's value.
    pub(crate) fn reset_config_edits(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.config.entry(id).reset_edits();
        self.sync_config_fields(id, cx);
        cx.notify();
    }

    /// Whether "Terapkan" is enabled for one instance.
    pub(crate) fn config_can_apply(&self, id: &InstanceId) -> bool {
        self.config.get(id).is_some_and(|config| config.can_apply())
    }

    /// Applies on Enter only when "Terapkan" is enabled.
    pub(crate) fn apply_config_if_ready(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.config_can_apply(id) {
            self.apply_config(id, cx);
        }
    }

    /// Applies the local edits (`F-CFG-01` AC2).
    pub(crate) fn apply_config(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let engine = self.instance(id).map(|view| view.engine);
        let config = self.config.entry(id);
        if config.any_busy() {
            return;
        }
        let Some(engine) = engine else {
            return;
        };
        if !config.validate(engine) {
            // The bad field renders the service's own `DBL-8601` sentence under
            // it; no round trip for a value the service would refuse anyway.
            cx.notify();
            return;
        }
        let changes = config.changed_pairs();
        if changes.is_empty() {
            return;
        }
        let service = self.service.clone();
        let iid = id.clone();
        self.config.entry(id).begin(ConfigAction::Apply);
        self.bg(cx, move || {
            let result = service.config_apply(&iid, changes);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let config = this.config.entry(&iid);
                config.end(ConfigAction::Apply);
                match result {
                    Ok(applied) => {
                        config.applied = Some(applied);
                        config.reset_edits();
                        // Re-read so the file's values (and the new backup) match
                        // what the user now sees.
                        this.reload_config_bg(&iid, cx);
                    }
                    Err(err) => {
                        config.error = Some(err.clone());
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Dismisses the "Tersimpan" banner's restart offer ("Nanti").
    pub(crate) fn dismiss_config_applied(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.config.entry(id).applied = None;
        cx.notify();
    }

    /// "Restart sekarang": the documented stop-then-start pair (`F-CFG-01` AC2).
    ///
    /// The stop completes asynchronously, so the start is scheduled and runs when
    /// the stop's `OperationFinished` arrives; starting a `Stopping` instance
    /// would be refused by the service.
    pub(crate) fn restart_after_apply(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let Some(view) = self.instance(id).cloned() else {
            return;
        };
        self.config.entry(id).applied = None;
        if crate::format::can_stop(view.status) {
            self.config_restart_after_stop = Some(id.clone());
            // Track this stop's own operation id: a pending restart may only be
            // consumed by the stop it queued, never by an unrelated operation,
            // and a stop that fails must drop it instead of leaving a restart
            // that fires on the next manual stop (`F2`).
            self.stop_instance_tracked(id.clone(), cx);
        } else if crate::format::can_start(view.status) {
            // Already stopped: the restart is just a start.
            self.start_instance(id.clone(), cx);
        } else {
            // Starting/Stopping: the pending transition is what the restart waits
            // on; do not queue a second one.
        }
        cx.notify();
    }

    /// Opens the restore confirmation, naming the newest backup.
    pub(crate) fn open_config_restore(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.config_restore_confirm = Some(id.clone());
        self.focus_dialog_buttons(cx);
        cx.notify();
    }

    /// Restores the newest backup behind the confirmation (`F-CFG-01` AC3).
    pub(crate) fn confirm_config_restore(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.config_restore_confirm.take() else {
            return;
        };
        // The Confirm button takes the dialog away like Escape and Cancel do, so
        // it must return the focus the same way (`F6`).
        self.restore_dialog_focus_after_close();
        self.restore_config_now(&id, RestoreFollowUp::FileOnly, cx);
    }

    /// Restores, then starts the instance (the failure banner's action).
    ///
    /// A restore then a start are two service calls, and either can fail; both
    /// are reported typed (`F-CFG-01` AC3).
    pub(crate) fn restore_and_restart(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.restore_config_now(id, RestoreFollowUp::Start, cx);
    }

    /// The shared restore path: restore, then optionally start.
    fn restore_config_now(
        &mut self,
        id: &InstanceId,
        follow_up: RestoreFollowUp,
        cx: &mut Context<Self>,
    ) {
        if self.config.get(id).is_some_and(|config| config.any_busy()) {
            return;
        }
        let service = self.service.clone();
        let iid = id.clone();
        self.config.entry(id).begin(ConfigAction::Restore);
        self.bg(cx, move || {
            let restored = service.config_restore(&iid);
            // The start is a separate call and a separate result. It runs only
            // when the restore succeeded, so the engine reads the restored file
            // and a failed restore never starts the engine on a broken config.
            let started = match (&restored, follow_up) {
                (Ok(()), RestoreFollowUp::Start) => Some(service.start_instance(&iid)),
                _ => None,
            };
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let config = this.config.entry(&iid);
                config.end(ConfigAction::Restore);
                match restored {
                    Ok(()) => {
                        // The restored file is what the form now shows; re-read
                        // it first so a start failure set below is not cleared by
                        // the read (both are reported typed, and neither rolls
                        // the restored file back).
                        this.reload_config_bg(&iid, cx);
                        match started {
                            Some(Ok(_)) => {
                                this.toast =
                                    Some(SharedString::from(strings::UI_TOAST_START_REQUESTED));
                            }
                            Some(Err(err)) => {
                                // The restore worked but the start did not;
                                // report the real typed failure of the start.
                                let facing = UserFacingError::from(&err);
                                this.config.entry(&iid).error = Some(facing.clone());
                                this.report_user_facing_error(&facing);
                            }
                            None => {}
                        }
                    }
                    Err(err) => {
                        // The restore itself failed (`DBL-8604` busy, `DBL-8605`
                        // no backup, an IO error): report it typed and do
                        // nothing else, the file is untouched.
                        config.error = Some(err.clone());
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// "Buka file konfigurasi": opens the raw file in the OS handler
    /// (`F-CFG-02` AC1); the path is shown regardless so a failed launcher is
    /// recoverable by hand (`DBL-8608`).
    pub(crate) fn open_config_raw(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if self.config.get(id).is_some_and(|config| config.any_busy()) {
            return;
        }
        let service = self.service.clone();
        let iid = id.clone();
        self.config.entry(id).begin(ConfigAction::OpenRaw);
        self.bg(cx, move || {
            let result = service.config_open_raw(&iid);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.config.entry(&iid).end(ConfigAction::OpenRaw);
                // The launch is how the file changes: re-read now the launcher
                // has returned, so the editor's edit is visible on return even
                // when the window never loses activation (`F7a`).
                this.reload_config_bg(&iid, cx);
                match result {
                    Ok(_command) => {
                        this.toast = Some(SharedString::from(
                            dblocal_core::config::messages::MSG_OPENED,
                        ));
                    }
                    Err(err) => {
                        // The typed message is shown *after* the re-read began,
                        // so it survives that read; the card also renders the
                        // path (from the view) so the user can open it by hand.
                        this.config.entry(&iid).error = Some(err.clone());
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Copies the raw config path to the clipboard.
    pub(crate) fn copy_config_path(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let path = self
            .config
            .get(id)
            .and_then(|config| config.view())
            .map(|view| view.raw_path.display().to_string())
            .unwrap_or_default();
        if path.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path));
        self.toast = Some(SharedString::from(strings::UI_CONFIG_PATH_COPIED));
        cx.notify();
    }

    /// Drops the field entities of a removed instance.
    pub(crate) fn forget_config_fields(&mut self, id: &InstanceId) {
        let instance = id.clone();
        self.config_fields.retain(|(iid, _), _| iid != &instance);
        self.clear_pending_restart_for(id);
        if self.config_restore_confirm.as_ref() == Some(id) {
            self.config_restore_confirm = None;
        }
    }

    /// Drops a pending "Restart sekarang" that can no longer be the stop it
    /// waited for, whatever took it away (`F2`).
    pub(crate) fn clear_pending_restart_for(&mut self, id: &InstanceId) {
        if self.config_restart_after_stop.as_ref() == Some(id) {
            self.config_restart_after_stop = None;
            self.config_restart_stop_op = None;
        }
    }

    /// Stops one instance and records the operation id a pending restart waits
    /// on, so the start runs only once this stop has settled (`F2`).
    pub(crate) fn stop_instance_tracked(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let iid = id.clone();
        self.bg(cx, move || {
            let result = service.stop_instance(&iid);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(op) => {
                        // Only a queued restart for this instance waits on it.
                        if this.config_restart_after_stop.as_ref() == Some(&iid) {
                            this.config_restart_stop_op = Some(op);
                        }
                        this.toast = Some(SharedString::from(strings::UI_TOAST_STOP_REQUESTED));
                    }
                    Err(err) => {
                        // The stop never started, so this restart cannot happen.
                        this.clear_pending_restart_for(&iid);
                        this.report_error(&err);
                    }
                }
                cx.notify();
            }
        });
    }

    /// The per-instance Config state, for the view.
    pub(crate) fn config_state_for(
        &self,
        id: &InstanceId,
    ) -> Option<&super::config_state::InstanceConfig> {
        self.config.get(id)
    }

    /// The instance whose restore confirmation is open.
    pub(crate) fn config_restore_confirm(&self) -> Option<&InstanceId> {
        self.config_restore_confirm.as_ref()
    }

    /// A field entity the view holds, if it was built.
    pub(crate) fn config_field_handle(
        &self,
        id: &InstanceId,
        key: &str,
    ) -> Option<Entity<InputState>> {
        self.config_fields
            .get(&(id.clone(), key.to_owned()))
            .cloned()
    }
}
