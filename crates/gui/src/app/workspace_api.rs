//! Small accessors the view submodules call on `Workspace`.
//!
//! Kept apart from the entity definition so `app.rs` stays a readable overview
//! of construction and event handling; these are one-liners the element tree
//! needs and nothing more.

use dblocal_core::model::{AuthMode, DeleteMode, EngineKey, InstanceId, Port};
use dblocal_core::service::{CatalogView, InstanceView, UriFormat};
use dblocal_core::strings;
use gpui_kit::component::{command::CommandState, input::InputState};
use gpui_kit::{App, Context, Entity, SharedString, Window};

use crate::format::{self, PaletteAction};

use super::{ActiveDownload, CreateForm, Workspace};

impl Workspace {
    /// Selects a centre-pane tab and loads that instance's own log buffer.
    ///
    /// Without this the Logs tab would keep showing whichever instance was
    /// opened last, because one shared buffer used to be reused.
    pub(crate) fn select_tab(&mut self, index: usize) {
        if let Some(id) = self.tabs.select(index) {
            self.load_logs_for(&id);
        }
    }

    /// Toggles follow mode for the visible Logs tab and, when turning it on,
    /// scrolls to the newest line.
    pub(crate) fn toggle_follow(&mut self, _cx: &mut Context<Self>) {
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

    pub(crate) fn downloads(&self) -> &[ActiveDownload] {
        &self.state.downloads
    }

    pub(crate) fn toggle_palette(&mut self) {
        self.palette_open = !self.palette_open;
        self.palette_query.clear();
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
            PaletteAction::NewInstance => self.open_create_dialog(window, cx),
            PaletteAction::Open(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.open_tab(id, cx);
                }
            }
            PaletteAction::Start(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.start_instance(id);
                }
            }
            PaletteAction::Stop(value) => {
                if let Ok(id) = value.parse::<InstanceId>() {
                    self.stop_instance(id);
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

    pub(crate) fn submit_create_action(&mut self, cx: &mut Context<Self>) {
        self.submit_create(cx);
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

    pub(crate) fn choose_engine_action(&mut self, engine: EngineKey) {
        self.choose_engine(engine);
    }

    pub(crate) fn choose_version_action(&mut self, version: String) {
        self.choose_version(version);
    }

    pub(crate) fn choose_auth_action(&mut self, auth: AuthMode) {
        self.choose_auth(auth);
    }

    /// Overrides the cached catalog status for preview renders.
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
    pub(crate) fn instance_port_proposal(&self, id: &InstanceId) -> u16 {
        let engine = self.instance(id).map(|view| view.engine);
        engine
            .and_then(|engine| self.service.suggest_port(engine).ok())
            .map(|port| port.get())
            .or_else(|| {
                self.instance(id)
                    .map(|view| view.port.get().saturating_add(1))
            })
            .unwrap_or(0)
    }

    /// Moves an instance to another port and clears its failure (`F-PRT-02` AC2).
    pub(crate) fn move_to_port(&mut self, id: &InstanceId, port: u16, cx: &mut Context<Self>) {
        let Some(port) = Port::new(port).ok() else {
            self.toast = Some(SharedString::from(strings::UI_VALIDATE_PORT_LOW));
            cx.notify();
            return;
        };
        match self.service.change_port(id, port) {
            Ok(()) => {
                self.refresh_instances();
                if let Some(err) = self.service.start_instance(id).err() {
                    self.report_error(&err);
                }
            }
            Err(err) => self.report_error(&err),
        }
        cx.notify();
    }

    pub(crate) fn is_revealed(&self, id: &InstanceId) -> bool {
        self.revealed.contains(id)
    }

    pub(crate) fn toggle_reveal(&mut self, id: &InstanceId) {
        if self.revealed.contains(id) {
            self.revealed.remove(id);
        } else {
            self.revealed.insert(id.clone());
        }
    }

    pub(crate) fn running_count_value(&self) -> usize {
        self.running_count()
    }

    pub(crate) fn version(&self) -> &'static str {
        self.version
    }

    pub(crate) fn tray_available(&self) -> bool {
        self.tray_available
    }

    pub(crate) fn open_new_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_create_dialog(window, cx);
    }
}
