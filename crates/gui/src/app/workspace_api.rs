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

impl Workspace {
    /// Selects a centre-pane tab and loads that instance's own state.
    pub(crate) fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.select(index) {
            self.load_logs_for(&id, cx);
            self.load_data_size(&id, cx);
        }
    }

    /// Sets the visible tab's inner tab, loading the log tail on first use.
    pub(crate) fn set_inner_tab(&mut self, inner: InnerTab, cx: &mut Context<Self>) {
        let index = self.tabs.selected_index();
        self.tabs.set_inner(index, inner);
        if inner == InnerTab::Logs
            && let Some(id) = self.tabs.selected_instance().cloned()
        {
            self.load_logs_for(&id, cx);
        }
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
    pub(crate) fn demo_content(&self) -> bool {
        self.demo_content
    }

    pub(crate) fn toggle_palette(&mut self) {
        self.palette_open = !self.palette_open;
        self.palette_query.clear();
    }

    #[allow(dead_code)]
    pub(crate) fn palette_is_open(&self) -> bool {
        self.palette_open
    }

    pub(crate) fn set_palette_open(&mut self, open: bool) {
        self.palette_open = open;
        self.palette_query.clear();
    }

    pub(crate) fn command_state(&self) -> &Entity<CommandState> {
        &self.command_state
    }

    pub(crate) fn palette_actions(&self) -> Vec<PaletteAction> {
        let items = format::palette_items(&self.state.instances);
        format::filter_palette(&items, &self.palette_query)
            .into_iter()
            .map(|item| item.action.clone())
            .collect()
    }

    pub(crate) fn palette_labels(&self) -> Vec<String> {
        let items = format::palette_items(&self.state.instances);
        format::filter_palette(&items, &self.palette_query)
            .into_iter()
            .map(|item| item.label.clone())
            .collect()
    }

    pub(crate) fn run_palette_action(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let actions = self.palette_actions();
        let Some(action) = actions.get(index).cloned() else {
            self.palette_open = false;
            return;
        };
        self.palette_open = false;
        match action {
            PaletteAction::NewInstance => self.open_new_instance(window, cx),
            PaletteAction::Open(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.open_tab(id, cx);
                }
            }
            PaletteAction::Start(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.start_instance(id, cx);
                }
            }
            PaletteAction::Stop(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.stop_instance(id, cx);
                }
            }
            PaletteAction::CopyUri(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.copy_uri(id, UriFormat::Uri, window, cx);
                }
            }
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

    /// Overrides the cached catalog status for preview renders.
    #[allow(dead_code)]
    pub(crate) fn set_catalog_status_for_preview(
        &mut self,
        status: dblocal_core::model::CatalogStatus,
    ) {
        self.catalog.status = status;
        self.catalog.banner = match status {
            dblocal_core::model::CatalogStatus::Fresh => None,
            dblocal_core::model::CatalogStatus::Stale => Some(strings::CATALOG_STALE.to_owned()),
            dblocal_core::model::CatalogStatus::Offline => {
                Some(strings::CATALOG_OFFLINE.to_owned())
            }
            _ => None,
        };
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
        self.tray.is_available()
    }

    pub(crate) fn tray_note(&self) -> Option<SharedString> {
        self.tray_note.clone()
    }

    #[allow(dead_code)]
    pub(crate) fn quit_machine(&self) -> &crate::quit::QuitMachine {
        &self.quit
    }

    // tray_sender/reset_for_latency removed: tray latency test measures the
    // real bridge+model path directly, no GPUI scheduler needed.

    pub(crate) fn open_new_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_create_dialog(window, cx);
    }
}
