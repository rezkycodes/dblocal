//! GPUI window bootstrap, the workspace entity, and the event pump.
//!
//! Rendering stays thin: pure logic lives in [`crate::format`], the log buffer
//! in [`crate::logs`], and the service bridge in [`crate::service_bridge`].
//! This module owns the entity state and the element tree; the submodules
//! render the title bar, status bar, command palette, and the two modals.

mod actions;
mod bootstrap;
mod create_dialog;
mod create_submit;
mod delete_dialog;
mod palette;
mod preview;
mod state;
mod status_bar;
mod title_bar;
mod views;
mod workspace_api;

use std::sync::Arc;
use std::time::Instant;

use dblocal_core::model::{AuthMode, CatalogStatus, DeleteMode, EngineKey, InstanceId, Port};
use dblocal_core::service::{CatalogView, DblService, InstanceView, UriFormat, UserFacingError};
use dblocal_core::strings;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Subscription, Window, div, prelude::*,
};
use gpui_component::{
    ActiveTheme, WindowExt, command::CommandState, h_flex, input::InputState,
    notification::Notification,
};

use crate::logs::LogStore;
use crate::service_bridge::{self, UiEvent};

pub use actions::{ROOT_CONTEXT, palette_hint};
pub use bootstrap::{appearance_is_dark, run_with_service};
pub use state::{ActiveDownload, ShellState};

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
    /// Name field state. `None` only in headless tests, which drive
    /// [`Workspace::submit_draft`] with explicit strings.
    pub name_input: Option<Entity<InputState>>,
    /// Port field state; see [`CreateForm::name_input`].
    pub port_input: Option<Entity<InputState>>,
}

pub use create_submit::{CreateDraft, SubmitError, Submitted};

/// The main workspace entity.
pub struct Workspace {
    service: Arc<dyn DblService>,
    events: flume::Receiver<UiEvent>,
    state: ShellState,
    catalog: CatalogView,
    open_tabs: Vec<OpenTab>,
    selected_tab: usize,
    selected_instance: Option<InstanceId>,
    /// Per-instance log buffers, keyed by instance so each tab shows its own.
    logs: LogStore,
    /// Scroll handle for the visible Logs list, driving follow and pause.
    log_scroll: gpui::UniformListScrollHandle,
    create: Option<CreateForm>,
    delete_target: Option<InstanceId>,
    delete_name_input: Option<Entity<InputState>>,
    /// Text typed into the delete confirmation, mirrored so the pure check has
    /// one source and the tests need no window.
    delete_typed_name: Option<String>,
    delete_mode: DeleteMode,
    delete_size: u64,
    palette_open: bool,
    palette_query: String,
    command_state: Entity<CommandState>,
    last_failure_mirror: Option<UserFacingError>,
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
            state: ShellState {
                instances,
                downloads: Vec::new(),
                last_failure: None,
            },
            catalog,
            open_tabs: Vec::new(),
            selected_tab: 0,
            selected_instance: None,
            logs: LogStore::new(),
            log_scroll: gpui::UniformListScrollHandle::new(),
            create: None,
            delete_target: None,
            delete_name_input: None,
            delete_typed_name: None,
            delete_mode: DeleteMode::ArchiveData,
            delete_size: 0,
            palette_open: false,
            palette_query: String::new(),
            command_state,
            last_failure_mirror: None,
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
            self.state.replace_instances(instances);
        }
    }

    fn apply_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::InstanceChanged(view) => self.state.upsert_instance(view),
            UiEvent::OperationProgress { id, stage } => self.state.on_progress(&id, stage),
            UiEvent::DownloadProgress { id, progress } => {
                self.state
                    .on_download(id, progress, strings::UI_DOWNLOADS.to_owned());
            }
            UiEvent::OperationFinished { id, result } => {
                self.state.on_finished(&id, result.clone());
                if let Err(err) = result {
                    self.toast = Some(SharedString::from(format!(
                        "{} — {}",
                        err.code, err.message
                    )));
                    self.last_failure_mirror = Some(err);
                }
                self.refresh_instances();
            }
            UiEvent::CatalogUpdated => {
                if let Ok(catalog) = self.service.catalog() {
                    self.catalog = catalog;
                }
            }
            UiEvent::LogLines { instance, lines } => {
                if let Ok(id) = instance.parse::<InstanceId>() {
                    self.state.on_log_lines(&mut self.logs, &id, lines);
                    // Follow the newest line only when the visible tab is this
                    // instance's and follow mode is on.
                    if self.visible_logs_following(&id) {
                        self.log_scroll.scroll_to_bottom();
                    }
                }
            }
            UiEvent::Resync => {
                // D: a lagged stream may have skipped InstanceChanged events,
                // so re-read the truth rather than trusting the delta stream.
                self.refresh_instances();
                if let Ok(catalog) = self.service.catalog() {
                    self.catalog = catalog;
                }
            }
            UiEvent::Closed => {}
        }
    }

    fn instance(&self, id: &InstanceId) -> Option<&InstanceView> {
        self.state.instances.iter().find(|view| &view.id == id)
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
        self.load_logs_for(&id);
        if let Err(err) = self.service.watch_logs(&id, true) {
            self.report_error(&err);
        }
        cx.notify();
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.open_tabs.len() {
            return;
        }
        let closed = self.open_tabs.remove(index);
        if let Err(err) = self.service.watch_logs(&closed.instance, false) {
            self.report_error(&err);
        }
        self.logs.close(&closed.instance);
        self.selected_tab = self
            .selected_tab
            .min(self.open_tabs.len().saturating_sub(1));
    }

    /// Loads one instance's tail into its own buffer.
    fn load_logs_for(&mut self, id: &InstanceId) {
        match self.service.tail_logs(id, crate::logs::capacity()) {
            Ok(lines) => self.logs.load(id, lines),
            Err(err) => self.report_error(&err),
        }
    }

    /// Whether the currently visible Logs tab is this instance and following.
    fn visible_logs_following(&self, id: &InstanceId) -> bool {
        let Some(tab) = self.open_tabs.get(self.selected_tab) else {
            return false;
        };
        tab.instance == *id
            && tab.inner == InnerTab::Logs
            && self
                .logs
                .get(id)
                .is_some_and(|buffer| buffer.is_following())
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
        self.last_failure_mirror = Some(facing);
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
        let text = self
            .open_tabs
            .get(self.selected_tab)
            .and_then(|tab| self.logs.get(&tab.instance))
            .map(|buffer| buffer.copy_text())
            .unwrap_or_default();
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
            name_input: Some(name_input),
            port_input: Some(port_input),
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
        // Read the entity-backed fields once, then hand plain strings to the
        // pure core.
        let draft = CreateDraft {
            engine: form.engine,
            version: form.version.clone(),
            auth: form.auth,
            name: form
                .name_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default(),
            port_text: form
                .port_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default(),
        };
        match create_submit::submit(self.service.as_ref(), &draft, &self.state.instances) {
            Ok(submitted) => {
                self.state.downloads.push(submitted.download);
            }
            Err(create_submit::SubmitError::Invalid(message)) => {
                self.create = Some(form);
                self.toast = Some(SharedString::from(message));
            }
            Err(create_submit::SubmitError::Service(err)) => {
                self.create = Some(form);
                self.report_error(&err);
            }
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
        if let Some(input) = self.delete_name_input.as_ref() {
            input.read(cx).value().as_ref() == expected.as_str()
        } else {
            self.delete_typed_name.as_deref() == Some(expected.as_str())
        }
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
        self.state.running_count()
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
