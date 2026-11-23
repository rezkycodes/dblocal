//! Action handlers for the Databases tab (`F-DBU-01..03`).
//!
//! The service is synchronous and can take a driver round trip (a TCP connect,
//! a DDL statement), so every call here goes through
//! [`Workspace::bg`](super::background). The result lands on the UI thread with
//! a typed error that the card renders: a failure is never a panic and never
//! only a toast.
//!
//! Two one-time values cross this module — a created user's password
//! (`F-DBU-02` AC2) and a reset superuser URI (`F-DBU-03` AC1). Both are moved
//! straight into [`SecretDialog`] and dropped when it closes, so they never
//! enter a toast, a log line or the card state.

use dblocal_core::admin::PrivilegeLevel;
use dblocal_core::model::InstanceId;
use dblocal_core::secret::SecretString;
use dblocal_core::service::{UriFormat, UserFacingError};
use dblocal_core::strings;
use gpui_kit::component::input::InputState;
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Window};

use super::Workspace;
use super::admin_dialogs::{DropDialog, SecretDialog, SecretKind};
use super::admin_state::{
    AdminAction, AdminData, InstanceAdmin, Loadable, validate_database_name, validate_user_name,
};

impl Workspace {
    /// Reads databases and users for one instance, off the GPUI thread.
    pub(crate) fn reload_admin_bg(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let Some(view) = self.instance(id).cloned() else {
            return;
        };
        if !crate::format::is_running(view.status) {
            // A stopped instance has no server to ask; the view shows the
            // empty state with a Start button instead of a connection error.
            // This is "not read", never a `Ready` with zero rows: a fake empty
            // read would look like an answer forever (the F1 defect).
            let admin = self.admin.entry(id);
            admin.data = Loadable::Idle;
            admin.error = None;
            admin.reset_failed = false;
            return;
        }
        let service = self.service.clone();
        let iid = id.clone();
        self.admin.entry(id).data = Loadable::Loading;
        self.admin.entry(id).error = None;
        self.bg(cx, move || {
            let databases = service.list_databases(&iid);
            let users = match &databases {
                Ok(_) => service.list_users(&iid),
                // One failure is enough to explain the card; do not overwrite it
                // with the users read.
                Err(_) => Ok(Vec::new()),
            };
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let admin = this.admin.entry(&iid);
                match (databases, users) {
                    (Ok(databases), Ok(users)) => {
                        admin.data = Loadable::Ready(AdminData { databases, users });
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        admin.data = Loadable::Failed(err.clone());
                        admin.error = Some(err);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Reads the tab's rows on entry: the first time, and again whenever the
    /// rows are not a current answer (never read, a failure, or a read
    /// invalidated by a status change).
    ///
    /// The text fields already exist (they are built with the workspace), so
    /// entry needs no window and works from any path that selects the tab.
    pub(crate) fn enter_databases_tab(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let needs_read = match self.admin.get(id) {
            None => true,
            Some(admin) => !admin.data.is_loading() && admin.data.ready().is_none(),
        };
        if needs_read {
            self.reload_admin_bg(id, cx);
        }
    }

    /// Re-reads the tab after a failure ("Muat ulang").
    pub(crate) fn retry_admin(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.reload_admin_bg(id, cx);
        cx.notify();
    }

    /// The database-name field's value.
    fn db_name_text(&self, cx: &App) -> String {
        self.db_name_input.read(cx).value().to_string()
    }

    /// Creates a database from the visible tab's field (`F-DBU-01` AC2/AC4).
    pub(crate) fn create_database(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let raw = self.db_name_text(cx);
        let name = match validate_database_name(&raw) {
            Ok(name) => name,
            Err(err) => {
                // No round trip for the obvious case: the core allowlist fails
                // it, so the card shows the service's own sentence.
                self.admin.entry(id).error = Some(err.clone());
                self.report_user_facing_error(&err);
                cx.notify();
                return;
            }
        };
        let input = self.db_name_input.clone();
        let service = self.service.clone();
        let iid = id.clone();
        let created = name.clone();
        self.admin.entry(id).begin(AdminAction::CreateDatabase);
        self.admin.entry(id).error = None;
        self.bg(cx, move || {
            let result = service.create_database(&iid, &name);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let admin = this.admin.entry(&iid);
                admin.end(AdminAction::CreateDatabase);
                match result {
                    Ok(()) => {
                        // F6: the field is cleared on success, so the next click
                        // does not hit "already exists" with the old name.
                        this.clear_input(&input, cx);
                        this.toast = Some(SharedString::from(strings::databases_created(&created)));
                    }
                    Err(err) => {
                        admin.error = Some(err.clone());
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Empties a text field without a window, for a success path in `bg`.
    fn clear_input(&mut self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        let entity_id = cx.entity().entity_id();
        let input = input.clone();
        cx.with_window(entity_id, |window, cx| {
            input.update(cx, |state, cx| state.set_value(String::new(), window, cx));
        });
    }

    /// Gives a dialog the keyboard and remembers where to return it (F3).
    ///
    /// Without this the covered field keeps focus: typing `x` appended to the
    /// "Buat database" field behind the overlay while the dialog's own field
    /// stayed empty, and Enter confirmed nothing.
    fn focus_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        target: Entity<InputState>,
    ) {
        // Capture what has focus *before* moving it: that field gets it back.
        self.dialog_focus.opened(window.focused(cx));
        target.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Takes focus for a SecretDialog, which has no text field of its own.
    ///
    /// The dialog's Copy and "Sudah saya simpan" buttons are its Tab stops, so
    /// focusing the dialog itself puts the keyboard inside it while leaving the
    /// covered field behind (F3).
    fn focus_dialog_buttons(&mut self, cx: &mut Context<Self>) {
        let entity_id = cx.entity().entity_id();
        cx.with_window(entity_id, |window, cx| {
            self.dialog_focus.opened(window.focused(cx));
            window.focus_next(cx);
        });
    }

    /// Returns focus to the field the dialog covered (F3).
    ///
    /// Nothing is focused when no dialog had captured a handle, so this is safe
    /// on every close path (Escape, the overlay, a button, a confirm).
    fn restore_dialog_focus(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.dialog_focus.closed() else {
            return;
        };
        let entity_id = cx.entity().entity_id();
        cx.with_window(entity_id, |window, cx| target.focus(window, cx));
    }

    /// Opens the type-the-name confirmation for dropping `name`.
    pub(crate) fn open_drop_dialog(
        &mut self,
        id: &InstanceId,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(strings::UI_DATABASES_DROP_PLACEHOLDER.to_owned())
        });
        // Enter confirms only when the typed text already matches the name; the
        // confirm button is disabled otherwise, and so is this path.
        cx.subscribe(
            &input,
            |this, _input, event: &gpui_kit::component::input::InputEvent, cx| {
                if matches!(
                    event,
                    gpui_kit::component::input::InputEvent::PressEnter { .. }
                ) && this.drop_confirmation_ok(cx)
                {
                    this.confirm_drop(cx);
                }
            },
        )
        .detach();
        self.drop_dialog = Some(DropDialog {
            instance: id.clone(),
            name,
            input: input.clone(),
        });
        // F3: the dialog owns the keyboard from here, not the field it covers.
        self.focus_dialog(window, cx, input);
        cx.notify();
    }

    /// Whether the drop dialog's typed text matches its database name.
    pub(crate) fn drop_confirmation_ok(&self, cx: &App) -> bool {
        match &self.drop_dialog {
            Some(dialog) => {
                let typed = dialog.input.read(cx).value().to_string();
                super::admin_state::drop_confirmation_matches(&typed, &dialog.name)
            }
            None => false,
        }
    }

    /// Drops the database the dialog is about (`F-DBU-01` AC3).
    pub(crate) fn confirm_drop(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.drop_dialog.clone() else {
            return;
        };
        if !self.drop_confirmation_ok(cx) {
            return;
        }
        let service = self.service.clone();
        let iid = dialog.instance.clone();
        let name = dialog.name.clone();
        self.drop_dialog = None;
        self.restore_dialog_focus(cx);
        self.admin.entry(&iid).begin(AdminAction::DropDatabase);
        self.bg(cx, move || {
            // `force == false`: a database with a live session is refused and
            // the user reads the typed "still in use" message, rather than the
            // GUI silently killing someone's connection.
            let result = service.drop_database(&iid, &name, false);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let admin = this.admin.entry(&iid);
                admin.end(AdminAction::DropDatabase);
                match &result {
                    Ok(()) => {
                        // F6: a drop gets its own confirmation, replacing the
                        // "created" line the earlier action left in the bar.
                        this.toast = Some(SharedString::from(strings::databases_dropped(&name)));
                    }
                    Err(err) => {
                        admin.error = Some(err.clone());
                        this.report_user_facing_error(err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Marks which database a new user is granted on.
    pub(crate) fn choose_user_database(&mut self, name: String, cx: &mut Context<Self>) {
        self.user_db_choice = Some(name);
        cx.notify();
    }

    /// Marks the privilege level a new user gets.
    pub(crate) fn choose_user_level(&mut self, level: PrivilegeLevel, cx: &mut Context<Self>) {
        self.user_level = level;
        cx.notify();
    }

    /// Creates a login role and shows its password exactly once (`F-DBU-02`).
    pub(crate) fn create_user(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        let raw = self.user_name_input.read(cx).value().to_string();
        let name = match validate_user_name(&raw) {
            Ok(name) => name,
            Err(err) => {
                self.admin.entry(id).error = Some(err.clone());
                self.report_user_facing_error(&err);
                cx.notify();
                return;
            }
        };
        // No database chosen is unreachable here: the submit button is disabled
        // until `user_db_choice` is set (`render_users_card`), so this branch is
        // the handler's own guard, not a user-facing path. It has no `DBL-` code
        // of the core registry: `DBL-8106` means "database in use" (F5).
        let Some(db) = self.user_db_choice.clone() else {
            return;
        };
        let level = self.user_level;
        let input = self.user_name_input.clone();
        let service = self.service.clone();
        let iid = id.clone();
        let user_name = name.clone();
        self.admin.entry(id).begin(AdminAction::CreateUser);
        self.admin.entry(id).error = None;
        self.bg(cx, move || {
            let result = service.create_user(&iid, &name, &db, level);
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                this.admin.entry(&iid).end(AdminAction::CreateUser);
                match result {
                    Ok(password) => {
                        this.secret_dialog = Some(SecretDialog {
                            instance: iid.clone(),
                            kind: SecretKind::UserPassword {
                                user: user_name.clone(),
                            },
                            value: password,
                        });
                        // F3: the Copy / "Sudah saya simpan" buttons are what a
                        // keyboard user acts on next, so the dialog takes focus
                        // away from the field behind it.
                        this.focus_dialog_buttons(cx);
                        // F6: the next create starts from an empty name.
                        this.clear_input(&input, cx);
                        this.toast = Some(SharedString::from(strings::databases_user_created(
                            &user_name,
                        )));
                    }
                    Err(err) => {
                        this.admin.entry(&iid).error = Some(err.clone());
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Opens the reset-password confirmation (`F-DBU-03`).
    pub(crate) fn open_reset_confirm(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        self.reset_confirm = Some(id.clone());
        // F3: a confirmation with no field focuses its own first button.
        self.focus_dialog_buttons(cx);
        cx.notify();
    }

    /// Resets the superuser password and shows the new URI once (`F-DBU-03`).
    ///
    /// On failure the card states that the old password still works, which is
    /// the guarantee the service makes (`F-DBU-03` AC2).
    pub(crate) fn confirm_reset(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.reset_confirm.take() else {
            return;
        };
        let service = self.service.clone();
        let iid = id.clone();
        self.admin.entry(&id).begin(AdminAction::ResetPassword);
        self.admin.entry(&id).error = None;
        self.admin.entry(&id).reset_failed = false;
        self.bg(cx, move || {
            let result = service
                .reset_superuser_password(&iid)
                .and_then(|_password| {
                    service
                        .connection_uri(&iid, UriFormat::Uri)
                        .map_err(|err| UserFacingError::from(&err))
                });
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                let admin = this.admin.entry(&iid);
                admin.end(AdminAction::ResetPassword);
                match result {
                    Ok(uri) => {
                        this.secret_dialog = Some(SecretDialog {
                            instance: iid.clone(),
                            kind: SecretKind::ResetUri,
                            value: uri.clone(),
                        });
                        // F3: focus the dialog, not the covered field.
                        this.focus_dialog_buttons(cx);
                        // The Overview card holds the old URI; replace it so the
                        // two views agree (F-DBU-03 AC1).
                        this.state.set_uri(&iid, &uri);
                        this.toast = Some(SharedString::from(strings::UI_DATABASES_RESET_DONE));
                    }
                    Err(err) => {
                        admin.error = Some(err.clone());
                        admin.reset_failed = true;
                        this.report_user_facing_error(&err);
                    }
                }
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Closes the one-time secret dialog, dropping the value with it.
    pub(crate) fn close_secret_dialog(&mut self, cx: &mut Context<Self>) {
        self.secret_dialog = None;
        self.restore_dialog_focus(cx);
        cx.notify();
    }

    /// The one-time dialog's value, for the copy button.
    pub(crate) fn secret_dialog_value(&self) -> Option<&SecretString> {
        self.secret_dialog.as_ref().map(|dialog| &dialog.value)
    }

    /// Copies the one-time value and confirms.
    pub(crate) fn copy_secret_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(value) = self.secret_dialog_value().map(|v| v.expose().to_owned()) else {
            return;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(value));
        self.toast = Some(SharedString::from(strings::UI_DATABASES_SECRET_COPIED));
        cx.notify();
    }

    /// The per-instance admin state, for the view.
    pub(crate) fn admin_state_for(&self, id: &InstanceId) -> Option<&InstanceAdmin> {
        self.admin.get(id)
    }

    /// The database-name input entity, for the view.
    pub(crate) fn db_name_input_entity(&self) -> Entity<InputState> {
        self.db_name_input.clone()
    }

    /// The new-user name input entity, for the view.
    pub(crate) fn user_name_input_entity(&self) -> Entity<InputState> {
        self.user_name_input.clone()
    }

    /// The database a new user is granted on.
    pub(crate) fn user_db_choice(&self) -> Option<&str> {
        self.user_db_choice.as_deref()
    }

    /// The privilege level a new user gets.
    pub(crate) fn user_level(&self) -> PrivilegeLevel {
        self.user_level
    }

    /// The drop dialog, for the view.
    pub(crate) fn drop_dialog(&self) -> Option<&DropDialog> {
        self.drop_dialog.as_ref()
    }

    /// The instance whose reset confirmation is open.
    pub(crate) fn reset_confirm(&self) -> Option<&InstanceId> {
        self.reset_confirm.as_ref()
    }

    /// The one-time secret dialog, for the view.
    pub(crate) fn secret_dialog(&self) -> Option<&SecretDialog> {
        self.secret_dialog.as_ref()
    }
}
