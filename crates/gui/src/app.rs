//! GPUI window bootstrap, the workspace entity, and the event pump.
//!
//! Rendering stays thin: pure logic lives in [`crate::format`], the log buffer
//! in [`crate::logs`], and the service bridge in [`crate::service_bridge`].
//! This module owns the entity state and the element tree; the submodules
//! render the title bar, status bar, command palette, and the two modals.

mod actions;
mod bootstrap;
mod create_dialog;
mod delete_dialog;
mod palette;
mod preview;
mod status_bar;
mod title_bar;
mod views;

use std::sync::Arc;
use std::time::Instant;

use dblocal_core::model::{
    AuthMode, CatalogStatus, DeleteMode, DownloadProgress, EngineKey, InstanceId, InstanceSpec,
    Port,
};
use dblocal_core::service::{
    CatalogView, DblService, InstanceView, OperationId, OperationStage, UriFormat, UserFacingError,
};
use dblocal_core::strings;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Subscription, Window, div, prelude::*,
};
use gpui_component::{
    ActiveTheme, WindowExt, command::CommandState, h_flex, input::InputState,
    notification::Notification,
};

use crate::format::{self, PaletteAction};
use crate::logs::LogBuffer;
use crate::service_bridge::{self, UiEvent};

pub use actions::{ROOT_CONTEXT, palette_hint};
pub use bootstrap::{appearance_is_dark, run_with_service};

/// Opens the GUI on the in-memory mock service.
pub fn run_with_mock() -> dblocal_core::Result<()> {
    bootstrap::run_with_mock()
}

/// Which inner tab of an instance is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerTab {
    /// Overview: engine, port, URI, actions.
    Overview,
    /// Logs: virtualised tail with follow and copy.
    Logs,
}

/// One open tab in the centre pane.
#[derive(Debug, Clone)]
pub struct OpenTab {
    /// Instance the tab shows.
    pub instance: InstanceId,
    /// Active inner tab.
    pub inner: InnerTab,
}

/// One in-flight download the status bar tracks.
#[derive(Debug, Clone)]
pub struct ActiveDownload {
    /// Operation that owns it.
    pub operation: OperationId,
    /// Display title, e.g. `PostgreSQL 17.4`.
    pub title: String,
    /// Latest sample.
    pub progress: DownloadProgress,
}

/// What the create dialog currently has selected.
#[derive(Clone)]
pub struct CreateForm {
    /// Engine chosen.
    pub engine: EngineKey,
    /// Version tag chosen (must exist in the catalog entry).
    pub version: String,
    /// Auth mode.
    pub auth: AuthMode,
    /// Suggested port for the engine, refreshed on open.
    pub suggested_port: Option<Port>,
    /// Name field state.
    pub name_input: Entity<InputState>,
    /// Port field state.
    pub port_input: Entity<InputState>,
}

/// The main workspace entity.
pub struct Workspace {
    service: Arc<dyn DblService>,
    events: flume::Receiver<UiEvent>,
    instances: Vec<InstanceView>,
    catalog: CatalogView,
    open_tabs: Vec<OpenTab>,
    selected_tab: usize,
    selected_instance: Option<InstanceId>,
    logs: LogBuffer,
    create: Option<CreateForm>,
    delete_target: Option<InstanceId>,
    delete_name_input: Option<Entity<InputState>>,
    delete_mode: DeleteMode,
    delete_size: u64,
    palette_open: bool,
    palette_query: String,
    command_state: Entity<CommandState>,
    downloads: Vec<ActiveDownload>,
    last_failure: Option<UserFacingError>,
    toast: Option<SharedString>,
    tray_available: bool,
    version: &'static str,
    focus_handle: FocusHandle,
    started_at: Instant,
    first_paint_logged: bool,
    /// Instances whose connection URI password is currently revealed.
    revealed: std::collections::HashSet<InstanceId>,
    _appearance: Option<Subscription>,
    _event_task: Option<gpui::Task<()>>,
}

impl Workspace {
    /// Builds the workspace and starts the event pump.
    pub fn new(service: Arc<dyn DblService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = service_bridge::spawn_bridge(service.clone());
        let instances = service.list_instances().unwrap_or_default();
        let catalog = service.catalog().unwrap_or_else(|_| CatalogView {
            status: CatalogStatus::Offline,
            banner: Some(strings::CATALOG_OFFLINE.to_owned()),
            engines: Vec::new(),
        });
        let focus_handle = cx.focus_handle();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let mut workspace = Self {
            service,
            events,
            instances,
            catalog,
            open_tabs: Vec::new(),
            selected_tab: 0,
            selected_instance: None,
            logs: LogBuffer::new(),
            create: None,
            delete_target: None,
            delete_name_input: None,
            delete_mode: DeleteMode::ArchiveData,
            delete_size: 0,
            palette_open: false,
            palette_query: String::new(),
            command_state,
            downloads: Vec::new(),
            last_failure: None,
            toast: None,
            tray_available: false,
            version: env!("CARGO_PKG_VERSION"),
            focus_handle,
            started_at: Instant::now(),
            first_paint_logged: false,
            revealed: std::collections::HashSet::new(),
            _appearance: None,
            _event_task: None,
        };
        workspace.refresh_instances();
        workspace.start_event_pump(cx);
        workspace
    }

    fn start_event_pump(&mut self, cx: &mut Context<Self>) {
        let events = self.events.clone();
        // Parks on the channel; there is no timer, so idle cost is zero.
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                let closed = matches!(event, UiEvent::Closed);
                let _ = this.update(cx, |workspace, cx| {
                    workspace.apply_event(event);
                    cx.notify();
                });
                if closed {
                    break;
                }
            }
        });
        self._event_task = Some(task);
    }

    fn refresh_instances(&mut self) {
        if let Ok(instances) = self.service.list_instances() {
            self.instances = instances;
        }
    }

    fn apply_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::InstanceChanged(view) => self.upsert_instance(view),
            UiEvent::OperationProgress { id, stage } => self.on_progress(id, stage),
            UiEvent::DownloadProgress { id, progress } => self.on_download(id, progress),
            UiEvent::OperationFinished { id, result } => self.on_finished(id, result),
            UiEvent::CatalogUpdated => {
                if let Ok(catalog) = self.service.catalog() {
                    self.catalog = catalog;
                }
            }
            UiEvent::LogLines { instance, lines } => {
                if self.logs.watched() == Some(instance.as_str()) {
                    self.logs.append(lines);
                }
            }
            UiEvent::Closed => {}
        }
    }

    fn upsert_instance(&mut self, view: InstanceView) {
        match self
            .instances
            .iter_mut()
            .find(|existing| existing.id == view.id)
        {
            Some(existing) => *existing = view,
            None => self.instances.push(view),
        }
    }

    fn on_progress(&mut self, id: OperationId, stage: OperationStage) {
        if matches!(stage, OperationStage::Done | OperationStage::Failed) {
            self.downloads.retain(|download| download.operation != id);
        }
    }

    fn on_download(&mut self, id: OperationId, progress: DownloadProgress) {
        if let Some(download) = self.downloads.iter_mut().find(|d| d.operation == id) {
            download.progress = progress;
        } else {
            self.downloads.push(ActiveDownload {
                operation: id,
                title: strings::UI_DOWNLOADS.to_owned(),
                progress,
            });
        }
    }

    fn on_finished(&mut self, id: OperationId, result: Result<(), UserFacingError>) {
        self.downloads.retain(|download| download.operation != id);
        if let Err(err) = result {
            self.toast = Some(SharedString::from(format!(
                "{} — {}",
                err.code, err.message
            )));
            self.last_failure = Some(err);
        }
        self.refresh_instances();
    }

    fn instance(&self, id: &InstanceId) -> Option<&InstanceView> {
        self.instances.iter().find(|view| &view.id == id)
    }

    fn open_tab(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        self.selected_instance = Some(id.clone());
        if let Some(position) = self.open_tabs.iter().position(|tab| tab.instance == id) {
            self.selected_tab = position;
        } else {
            self.open_tabs.push(OpenTab {
                instance: id.clone(),
                inner: InnerTab::Overview,
            });
            self.selected_tab = self.open_tabs.len().saturating_sub(1);
        }
        if let Ok(lines) = self.service.tail_logs(&id, crate::logs::capacity()) {
            self.logs.replace_with(lines);
        }
        let _ = self.service.watch_logs(&id, true);
        self.logs.set_watched(Some(id.to_string()));
        cx.notify();
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.open_tabs.len() {
            return;
        }
        let closed = self.open_tabs.remove(index);
        let _ = self.service.watch_logs(&closed.instance, false);
        self.selected_tab = self
            .selected_tab
            .min(self.open_tabs.len().saturating_sub(1));
        if self.logs.watched() == Some(closed.instance.to_string().as_str()) {
            self.logs.set_watched(None);
        }
    }

    fn start_instance(&mut self, id: InstanceId) {
        if let Err(err) = self.service.start_instance(&id) {
            self.report_error(&err);
        }
    }

    fn stop_instance(&mut self, id: InstanceId) {
        if let Err(err) = self.service.stop_instance(&id) {
            self.report_error(&err);
        }
    }

    fn report_error(&mut self, err: &dblocal_core::DblError) {
        let facing = UserFacingError::from(err);
        self.toast = Some(SharedString::from(format!(
            "{} — {}",
            facing.code, facing.message
        )));
        self.last_failure = Some(facing);
    }

    fn copy_uri(
        &mut self,
        id: InstanceId,
        format: UriFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.connection_uri(&id, format) {
            Ok(secret) => {
                cx.write_to_clipboard(ClipboardItem::new_string(secret.expose().to_owned()));
                let message = match format {
                    UriFormat::DotEnv => strings::UI_TOAST_DOTENV_COPIED,
                    _ => strings::UI_TOAST_URI_COPIED,
                };
                self.toast = Some(SharedString::from(message));
                window.push_notification(Notification::success(message), cx);
            }
            Err(err) => self.report_error(&err),
        }
        cx.notify();
    }

    fn copy_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.logs.copy_text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast = Some(SharedString::from(strings::UI_TOAST_LOGS_COPIED));
        window.push_notification(Notification::success(strings::UI_TOAST_LOGS_COPIED), cx);
        cx.notify();
    }

    fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let engine = EngineKey::Postgresql;
        let (version, port) = self.default_engine_version(engine);
        let suggested = self.service.suggest_port(engine).ok();
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strings::UI_FIELD_NAME.to_owned()));
        let port_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strings::UI_FIELD_PORT.to_owned()));
        if let Some(port) = port {
            port_input.update(cx, |state, cx| {
                state.set_value(port.to_string(), window, cx);
            });
        }
        self.create = Some(CreateForm {
            engine,
            version,
            auth: AuthMode::Password,
            suggested_port: suggested.or(port),
            name_input,
            port_input,
        });
        cx.notify();
    }

    fn default_engine_version(&self, engine: EngineKey) -> (String, Option<Port>) {
        let version = self
            .catalog
            .engines
            .iter()
            .find(|entry| entry.engine == engine)
            .and_then(|entry| entry.versions.iter().find(|v| !v.eol))
            .map(|v| v.version.clone())
            .unwrap_or_default();
        (version, self.service.suggest_port(engine).ok())
    }

    fn choose_engine(&mut self, engine: EngineKey) {
        let (version, port) = self.default_engine_version(engine);
        if let Some(form) = self.create.as_mut() {
            form.engine = engine;
            form.version = version;
            form.suggested_port = port;
        }
    }

    fn choose_version(&mut self, version: String) {
        if let Some(form) = self.create.as_mut() {
            form.version = version;
        }
    }

    fn choose_auth(&mut self, auth: AuthMode) {
        if let Some(form) = self.create.as_mut() {
            form.auth = auth;
        }
    }

    fn cancel_create(&mut self) {
        self.create = None;
    }

    fn submit_create(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.create.take() else {
            return;
        };
        let name = form.name_input.read(cx).value().to_string();
        let port_text = form.port_input.read(cx).value().to_string();
        let validation = format::validate_form(&name, &port_text, &self.instances);
        if !validation.is_valid() {
            self.create = Some(form);
            self.toast = Some(SharedString::from(
                validation
                    .conflict
                    .or(validation.name_error)
                    .or(validation.port_error)
                    .unwrap_or_else(|| strings::ERR_INVALID_INPUT.to_owned()),
            ));
            cx.notify();
            return;
        }
        let port = if port_text.trim().is_empty() {
            None
        } else {
            match port_text
                .trim()
                .parse::<u16>()
                .ok()
                .and_then(|v| Port::new(v).ok())
            {
                Some(port) => Some(port),
                None => {
                    self.create = Some(form);
                    self.toast = Some(SharedString::from(strings::UI_VALIDATE_PORT_LOW));
                    cx.notify();
                    return;
                }
            }
        };
        let spec = dblocal_core::model::InstanceName::new(name.clone()).and_then(|instance_name| {
            InstanceSpec::new(
                instance_name,
                form.engine,
                form.version.clone(),
                port,
                form.auth,
            )
        });
        match spec {
            Ok(spec) => match self.service.create_instance(spec) {
                Ok(op) => {
                    self.downloads.push(ActiveDownload {
                        operation: op,
                        title: format!("{} {}", form.engine.display_name(), form.version),
                        progress: DownloadProgress::new(0, 1, 0, None),
                    });
                }
                Err(err) => self.report_error(&err),
            },
            Err(err) => self.report_error(&err),
        }
        cx.notify();
    }

    fn open_delete_dialog(&mut self, id: InstanceId, window: &mut Window, cx: &mut Context<Self>) {
        let name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(strings::UI_DELETE_NAME_PLACEHOLDER.to_owned())
        });
        self.delete_target = Some(id.clone());
        self.delete_mode = DeleteMode::ArchiveData;
        self.delete_name_input = Some(name_input);
        self.delete_size = self.service.data_size(&id).unwrap_or(0);
        cx.notify();
    }

    fn delete_confirmation_matches(&self, cx: &App) -> bool {
        let Some(id) = self.delete_target.as_ref() else {
            return false;
        };
        let expected = self
            .instance(id)
            .map(|v| v.name.clone())
            .unwrap_or_default();
        self.delete_name_input
            .as_ref()
            .map(|input| input.read(cx).value().as_ref() == expected.as_str())
            .unwrap_or(false)
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.delete_target.clone() else {
            return;
        };
        if self.delete_mode == DeleteMode::PurgeData && !self.delete_confirmation_matches(cx) {
            self.toast = Some(SharedString::from(strings::UI_DELETE_TYPE_NAME));
            cx.notify();
            return;
        }
        match self.service.delete_instance(&id, self.delete_mode) {
            Ok(_) => {
                self.close_tab_by_instance(&id);
                self.delete_target = None;
                self.delete_name_input = None;
                self.refresh_instances();
            }
            Err(err) => self.report_error(&err),
        }
        cx.notify();
    }

    fn close_tab_by_instance(&mut self, id: &InstanceId) {
        if let Some(position) = self.open_tabs.iter().position(|tab| &tab.instance == id) {
            self.close_tab(position);
        }
    }

    fn close_modals(&mut self) {
        self.palette_open = false;
        self.create = None;
        self.delete_target = None;
        self.delete_name_input = None;
    }

    fn running_count(&self) -> usize {
        format::running_count(&self.instances)
    }

    // ----- Accessors used by submodules ----------------------------------

    pub(crate) fn instances(&self) -> &[InstanceView] {
        &self.instances
    }

    pub(crate) fn catalog(&self) -> &CatalogView {
        &self.catalog
    }

    pub(crate) fn downloads(&self) -> &[ActiveDownload] {
        &self.downloads
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
        let items = format::palette_items(&self.instances);
        format::filter_palette(&items, &self.palette_query)
            .into_iter()
            .map(|item| item.action.clone())
            .collect()
    }

    pub(crate) fn palette_labels(&self) -> Vec<String> {
        let items = format::palette_items(&self.instances);
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

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_paint_logged {
            self.first_paint_logged = true;
            tracing::info!(
                "FIRST_FRAME_MS={:.2}",
                self.started_at.elapsed().as_secs_f64() * 1000.0
            );
        }
        let deleting = self.delete_target.is_some();
        let creating = self.create.is_some();
        let colours = cx.theme().clone();
        div()
            .id("root")
            .track_focus(&self.focus_handle)
            .key_context(ROOT_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .bg(colours.background)
            .text_color(colours.foreground)
            .font_family(colours.font_family.clone())
            .on_action(cx.listener(|this, _: &actions::OpenPalette, _window, cx| {
                this.toggle_palette();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &actions::NewInstance, window, cx| {
                this.open_new_instance(window, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::CloseOverlay, _window, cx| {
                this.close_modals();
                cx.notify();
            }))
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_center(cx)),
            )
            .child(self.render_status_bar(cx))
            .when(creating, |this| this.child(create_dialog::render(self, cx)))
            .when(deleting, |this| this.child(delete_dialog::render(self, cx)))
            .when(self.palette_open, |this| {
                this.child(palette::render(self, cx))
            })
    }
}

impl Workspace {
    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        title_bar::render(self, cx)
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        status_bar::render(self, cx)
    }
}
