//! Small accessors the view submodules call on `Workspace`.
//!
//! Kept apart from the entity definition so `app.rs` stays a readable overview
//! of construction and event handling; these are one-liners the element tree
//! needs and nothing more.

use dblocal_core::model::{AuthMode, DeleteMode, EngineKey, InstanceId, Port, Runtime};
use dblocal_core::service::{CatalogView, InstanceView, UriFormat};
use dblocal_core::strings;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{command::CommandState, input::InputState};
use gpui_kit::{App, Context, Entity, SharedString, Window};

use crate::format::{self, PaletteAction};

use super::{ActiveDownload, CreateForm, InnerTab, UriView, Workspace};

/// The user's home directory as the platform reports it, read once at
/// construction so a render never touches the environment (R9).
pub(crate) fn home_dir_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

impl Workspace {
    /// Selects a centre-pane tab and loads that instance's own state.
    pub(crate) fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.select(index) {
            self.load_logs_for(&id, cx);
            self.load_data_size(&id, cx);
        }
    }

    /// Sets the inner tab, loading what that tab needs on entry.
    ///
    /// The Databases tab reads its rows here, not on a render path, so opening
    /// it by any route (tab bar, keyboard, restored tab) issues the same reads.
    pub(crate) fn set_inner_tab(&mut self, inner: InnerTab, cx: &mut Context<Self>) {
        let index = self.tabs.selected_index();
        self.tabs.set_inner(index, inner);
        if inner == InnerTab::Logs
            && let Some(id) = self.tabs.selected_instance().cloned()
        {
            self.load_logs_for(&id, cx);
        }
        if inner == InnerTab::Databases
            && let Some(id) = self.tabs.selected_instance().cloned()
        {
            self.enter_databases_tab(&id, cx);
        }
        if inner == InnerTab::Config
            && let Some(id) = self.tabs.selected_instance().cloned()
        {
            self.enter_config_tab(&id, cx);
        }
    }

    /// Sets the inner tab from a click. Kept for the tab bar's listener, which
    /// has a window; the work itself no longer needs one.
    pub(crate) fn set_inner_tab_with_window(
        &mut self,
        inner: InnerTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_inner_tab(inner, cx);
    }

    /// Toggles follow mode for the visible Logs tab and, when turning it on,
    /// scrolls to the newest line.
    pub(crate) fn toggle_follow(&mut self) {
        let Some(id) = self.tabs.selected_tab().map(|tab| tab.instance.clone()) else {
            return;
        };
        let now_following = {
            let Some(buffer) = self.logs.get_mut(&id) else {
                return;
            };
            let next = !buffer.is_following();
            buffer.set_following(next);
            next
        };
        if now_following {
            self.log_scroll.scroll_to_bottom();
        }
    }

    pub(crate) fn instances(&self) -> &[InstanceView] {
        &self.state.instances
    }

    pub(crate) fn catalog(&self) -> &CatalogView {
        &self.catalog
    }

    pub(crate) fn runtimes(&self) -> &[Runtime] {
        &self.runtimes
    }

    pub(crate) fn downloads(&self) -> &[ActiveDownload] {
        &self.state.downloads
    }

    /// Whether the demo build may render sample rows in the placeholder tabs.
    ///
    /// Only the test suite reads it; no live screen branches on it any more (the
    /// Config tab is live in `W4-H`).
    #[cfg(test)]
    pub(crate) fn demo_content(&self) -> bool {
        self.demo_content
    }

    /// Flips the palette flag; a test shortcut. The real toggle is
    /// [`Self::toggle_palette_in`], which also moves focus.
    #[cfg(test)]
    pub(crate) fn toggle_palette(&mut self) {
        self.palette_open = !self.palette_open;
    }

    /// Opens or closes the palette, taking and returning keyboard focus.
    ///
    /// `F-PLH-01` AC1/AC2: the palette is keyboard-driven, so opening it must
    /// move focus into its query field and closing it must give focus back to
    /// wherever it came from (`dialog_focus` owns the capture/restore rule).
    pub(crate) fn toggle_palette_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.close_palette(cx);
        } else {
            self.open_palette(window, cx);
        }
    }

    /// Opens the palette and focuses its query field.
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // `F1`/`F2`: capture the opener for the focus return and make the modal
        // container so the palette traps Tab inside it.
        self.begin_modal(window, cx);
        self.palette_open = true;
        let command_state = self.command_state.clone();
        // Clear any query left from a previous open so the list starts fresh and
        // typing does not append to stale text.
        command_state.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.focus(window, cx);
        });
    }

    /// Closes the palette and gives focus back to the element that opened it.
    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_open = false;
        // The close often runs from a listener without a window (the palette's
        // own Escape); `render` applies the parked handle on the next frame.
        let _ = cx;
        self.restore_dialog_focus_after_close();
    }

    #[cfg(test)]
    pub(crate) fn palette_is_open(&self) -> bool {
        self.palette_open
    }

    #[cfg(test)]
    pub(crate) fn set_palette_open(&mut self, open: bool) {
        self.palette_open = open;
    }

    pub(crate) fn command_state(&self) -> &Entity<CommandState> {
        &self.command_state
    }

    /// The palette rows the command renders, in order (`F-PLH-01` AC2).
    ///
    /// `CommandState` filters these locally and reports the *original* row index
    /// on confirm, so the list `run_palette_action` indexes is this unfiltered
    /// one — never `palette_query`-filtered, or the indices would drift once the
    /// user types.
    pub(crate) fn palette_items(&self) -> Vec<format::PaletteItem> {
        format::palette_items(&self.state.instances)
    }

    /// The runnable actions of the palette rows, `None` for a disabled row.
    pub(crate) fn palette_actions(&self) -> Vec<Option<PaletteAction>> {
        self.palette_items()
            .into_iter()
            .map(|item| item.action)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn palette_labels(&self) -> Vec<String> {
        self.palette_items()
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    /// The reason a disabled palette row shows, or `None` when it is runnable.
    #[cfg(test)]
    pub(crate) fn palette_reasons(&self) -> Vec<Option<String>> {
        self.palette_items()
            .into_iter()
            .map(|item| item.reason)
            .collect()
    }

    pub(crate) fn run_palette_action(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = self.palette_actions().get(index).cloned().flatten();
        self.palette_open = false;
        match action {
            Some(PaletteAction::NewInstance) => self.open_new_instance(window, cx),
            Some(PaletteAction::Open(value)) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.open_tab(id, cx);
                }
            }
            Some(PaletteAction::Start(value)) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.start_instance(id, cx);
                }
            }
            Some(PaletteAction::Stop(value)) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.stop_instance(id, cx);
                }
            }
            Some(PaletteAction::CopyUri(value)) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.copy_uri(id, UriFormat::Uri, window, cx);
                }
            }
            Some(PaletteAction::Delete(value)) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.open_delete_dialog(id, window, cx);
                }
            }
            // A disabled row or an out-of-range index: close without running.
            None => {}
        }
        // `F1`: a confirm is a dismissal, but an action that opened another
        // overlay (NewInstance -> create dialog) owns its own focus capture, so
        // only restore when no overlay is open now. Returning focus here (or to
        // the root fallback) is what keeps `ctrl-k` working right after a
        // command ran; without it the keyboard was dead until a mouse click.
        if !self.any_overlay_open() {
            self.restore_dialog_focus_after_close();
        }
        cx.notify();
    }

    pub(crate) fn create_form(&self) -> Option<&CreateForm> {
        self.create.as_ref()
    }

    pub(crate) fn delete_target_name(&self, _cx: &App) -> Option<String> {
        let id = self.delete_target.as_ref()?;
        Some(
            self.instance(id)
                .map(|view| view.name.clone())
                .unwrap_or_else(|| id.to_string()),
        )
    }

    pub(crate) fn delete_confirmation_ok(&self, cx: &App) -> bool {
        self.delete_confirmation_matches(cx)
    }

    pub(crate) fn delete_mode(&self) -> DeleteMode {
        self.delete_mode
    }

    pub(crate) fn set_delete_mode(&mut self, mode: DeleteMode) {
        self.delete_mode = mode;
    }

    pub(crate) fn delete_size(&self) -> u64 {
        self.delete_size
    }

    pub(crate) fn delete_name_input(&self) -> Option<&Entity<InputState>> {
        self.delete_name_input.as_ref()
    }

    /// The instance header's "Hapus" button handle (`F-INS-05`), so a keyboard
    /// test can walk Tab onto it and confirm the real control opens the dialog.
    #[cfg(test)]
    pub(crate) fn delete_button_focus(&self) -> gpui_kit::FocusHandle {
        self.delete_focus.clone()
    }

    /// The instance the open delete dialog targets, for assertions.
    #[cfg(test)]
    pub(crate) fn delete_target_id(&self) -> Option<InstanceId> {
        self.delete_target.clone()
    }

    pub(crate) fn submit_create_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_create(window, cx);
    }

    pub(crate) fn cancel_create_action(&mut self) {
        self.cancel_create();
    }

    pub(crate) fn confirm_delete_action(&mut self, cx: &mut Context<Self>) {
        self.confirm_delete(cx);
    }

    pub(crate) fn close_modals_action(&mut self) {
        self.close_modals();
    }

    pub(crate) fn choose_engine_action(
        &mut self,
        engine: EngineKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_engine(engine, window, cx);
    }

    pub(crate) fn choose_version_action(
        &mut self,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_version(version, window, cx);
    }

    pub(crate) fn choose_auth_action(&mut self, auth: AuthMode) {
        self.choose_auth(auth);
    }

    /// Next free port for an instance's engine, falling back to its port + 1.
    /// Uses cached suggestion; never calls the service on the render path.
    pub(crate) fn instance_port_proposal(&self, id: &InstanceId) -> u16 {
        let suggestion = self.port_suggestions.get(id).copied();
        match self.instance(id) {
            Some(view) => format::suggested_fix_port(view, suggestion),
            None => suggestion.unwrap_or(0),
        }
    }

    /// Moves an instance to another port and starts it (`F-PRT-02` AC2):
    /// `change_port` then `start_instance`, exactly the documented pair.
    pub(crate) fn move_to_port_and_start(
        &mut self,
        id: &InstanceId,
        port: u16,
        cx: &mut Context<Self>,
    ) {
        let Some(port) = Port::new(port).ok() else {
            self.set_toast(SharedString::from(strings::UI_VALIDATE_PORT_LOW));
            cx.notify();
            return;
        };
        let service = self.service.clone();
        let iid = id.clone();
        self.bg(cx, move || {
            let result = service
                .change_port(&iid, port)
                .map(|_| service.start_instance(&iid));
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match result {
                    Ok(Ok(_)) => {
                        this.toast = Some(SharedString::from(strings::UI_TOAST_START_REQUESTED));
                        this.refresh_instances_bg(cx);
                    }
                    Ok(Err(err)) | Err(err) => this.report_error(&err),
                }
                cx.notify();
            }
        });
    }

    /// The URI text for an instance's card, masked unless revealed.
    pub(crate) fn uri_text(&self, id: &InstanceId) -> String {
        self.state
            .uri(id)
            .map(|uri| uri.text().to_owned())
            .unwrap_or_default()
    }

    /// Whether the card is currently showing the plaintext.
    pub(crate) fn uri_revealed(&self, id: &InstanceId) -> bool {
        self.state.uri(id).is_some_and(UriView::is_revealed)
    }

    /// The `.env` block for the card preview, masked unless revealed.
    /// Uses cached dotenv fetched in background; never calls service on render.
    pub(crate) fn dotenv_rows(&self, id: &InstanceId) -> Vec<crate::format::EnvLine> {
        let revealed = self.uri_revealed(id);
        let uri_secret = self.state.uri(id).map(|u| u.secret());
        match self.dotenv_cache.get(id) {
            Some(secret) => super::masked_env_rows(secret.expose(), revealed, uri_secret),
            None => Vec::new(),
        }
    }

    /// The data directory size, once read.
    pub(crate) fn data_size(&self, id: &InstanceId) -> Option<u64> {
        self.data_sizes.get(id).copied()
    }

    /// The dock filter text as typed.
    pub(crate) fn filter_text(&self, cx: &App) -> String {
        self.filter_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub(crate) fn filter_input(&self) -> Option<&Entity<InputState>> {
        self.filter_input.as_ref()
    }

    /// Whether an engine group is collapsed in the dock.
    pub(crate) fn is_collapsed(&self, engine: EngineKey) -> bool {
        self.collapsed.contains(&engine)
    }

    /// The stable focus handle of an engine-group header (`F4`, `G2`).
    pub(crate) fn dock_group_focus(&self, engine: EngineKey) -> Option<gpui_kit::FocusHandle> {
        self.dock_focus.get(&engine).cloned()
    }

    pub(crate) fn toggle_collapsed(&mut self, engine: EngineKey) {
        if !self.collapsed.remove(&engine) {
            self.collapsed.insert(engine);
        }
    }

    pub(crate) fn log_scroll(&self) -> gpui_kit::UniformListScrollHandle {
        self.log_scroll.clone()
    }

    /// A copy of an instance's log lines for the log list.
    pub(crate) fn log_lines(&self, id: &InstanceId) -> crate::logs::LogBuffer {
        self.logs.get(id).cloned().unwrap_or_default()
    }

    /// The offline/stale catalog strip above the workspace (`F-CAT-01`).
    pub(crate) fn offline_banner(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        super::status_bar::offline_banner(self, cx.theme())
    }

    /// The stale-catalog strip shown inside the centre pane (`F-CAT-01`).
    pub(crate) fn catalog_banner_el(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        super::status_bar::stale_banner(self, cx.theme())
    }

    /// The latest toast line (copy confirmations, DBL-coded failures).
    pub(crate) fn toast_text(&self) -> Option<SharedString> {
        self.toast.clone()
    }

    pub(crate) fn version(&self) -> &'static str {
        self.version
    }

    /// Whether a background service call is in flight (neutral pending state).
    pub(crate) fn pending(&self) -> bool {
        self.is_pending()
    }

    pub(crate) fn tray_available(&self) -> bool {
        self.tray_available
    }

    pub(crate) fn tray_note(&self) -> Option<SharedString> {
        self.tray_note.clone()
    }

    pub(crate) fn total_home_size(&self) -> Option<u64> {
        self.home_size_bytes
    }

    pub(crate) fn home_display(&self) -> String {
        crate::format::abbreviate_home(&self.home_root, self.home_dir.as_deref())
    }

    /// The user's home directory, for abbreviating instance paths (R9).
    pub(crate) fn home_dir(&self) -> Option<&std::path::Path> {
        self.home_dir.as_deref()
    }

    pub(crate) fn home_root(&self) -> &std::path::Path {
        &self.home_root
    }

    #[cfg(test)]
    pub(crate) fn quit_machine(&self) -> &crate::quit::QuitMachine {
        &self.quit
    }

    // tray_sender/reset_for_latency removed: tray latency test measures the
    // real bridge+model path directly, no GPUI scheduler needed.

    pub(crate) fn open_new_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_create_dialog(window, cx);
    }
}

/// Test-only shortcuts so the Databases tests drive the handlers exactly as a
/// typed click would, without spinning up a second window for every entity.
#[cfg(test)]
impl Workspace {
    /// Types `value` into the database-name field.
    pub(crate) fn set_db_name_for_test(
        &self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.db_name_input.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx)
        });
    }

    /// Types `value` into the new-user name field.
    pub(crate) fn set_user_name_for_test(
        &self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.user_name_input.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx)
        });
    }

    /// Types `value` into the drop dialog's confirmation field.
    pub(crate) fn set_drop_text_for_test(
        &self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = &self.drop_dialog {
            dialog.input.update(cx, |state, cx| {
                state.set_value(value.to_owned(), window, cx)
            });
        }
    }

    /// The database-name field's text, as the user sees it.
    #[cfg(test)]
    pub(crate) fn db_name_text_for_test(&self, cx: &App) -> String {
        self.db_name_input.read(cx).value().to_string()
    }

    /// The new-user name field's text.
    #[cfg(test)]
    pub(crate) fn user_name_text_for_test(&self, cx: &App) -> String {
        self.user_name_input.read(cx).value().to_string()
    }

    /// The instance a "Restart sekarang" is waiting to start, for assertions.
    pub(crate) fn config_restart_pending_for_test(&self) -> Option<&InstanceId> {
        self.config_restart_after_stop.as_ref()
    }

    /// Runs the Enter path ("apply only when Terapkan is enabled") for one
    /// instance, exactly as a keypress in a Config field does.
    pub(crate) fn apply_config_if_ready_for_test(
        &mut self,
        id: &InstanceId,
        cx: &mut Context<Self>,
    ) {
        self.apply_config_if_ready(id, cx);
    }

    /// Opens the drop dialog, reusing the database-name field as its input (the
    /// dialog only needs an entity to hold the typed text).
    pub(crate) fn open_drop_dialog_for_test(
        &mut self,
        id: &InstanceId,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let input = self.db_name_input.clone();
        self.drop_dialog = Some(super::admin_dialogs::DropDialog {
            instance: id.clone(),
            name: name.to_owned(),
            input,
        });
        cx.notify();
    }
}
