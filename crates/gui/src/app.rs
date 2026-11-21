//! GPUI window bootstrap, the workspace entity, and the event pump.
//!
//! Rendering stays thin: pure logic lives in [`crate::format`], the log buffer
//! in [`crate::logs`], and the service bridge in [`crate::service_bridge`].
//! This module owns the entity state and the action handlers; the submodules
//! render the title bar, dock, tabs, screens, status bar, palette and dialogs.

mod actions;
mod bootstrap;
mod components;
mod create_dialog;
mod create_submit;
mod delete_dialog;
mod dock;
mod palette;
mod preview;
mod screens;
mod state;
mod status_bar;
mod title_bar;
mod workspace_api;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use dblocal_core::model::{
    AuthMode, CatalogStatus, DeleteMode, EngineKey, InstanceId, Port, Runtime,
};
use dblocal_core::service::{CatalogView, DblService, InstanceView, UriFormat, UserFacingError};
use dblocal_core::strings;
use gpui_kit::component::{
    ActiveTheme, WindowExt, command::CommandState, h_flex, input::InputState,
    notification::Notification,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Subscription, Window, div, prelude::*,
};

use crate::logs::LogStore;
use crate::service_bridge::{self, UiEvent};

pub use actions::{ROOT_CONTEXT, modifier_label, palette_hint};
pub use bootstrap::{appearance_is_dark, run_with_service};
pub use state::{ActiveDownload, InnerTab, OpenTab, ShellState, Tabs, UriView};

/// Opens the GUI on the in-memory demo service.
///
/// This is the demo/headless entry point the render tests use; it seeds the
/// prototype scenario so every screen can be produced without a live session.
pub fn run_with_mock() -> dblocal_core::Result<()> {
    bootstrap::run_with_mock()
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
    /// Installed runtimes, re-read after a create finishes.
    runtimes: Vec<Runtime>,
    /// Centre-pane tabs and the current selection.
    tabs: Tabs,
    /// Per-instance log buffers, keyed by instance so each tab shows its own.
    logs: LogStore,
    /// Scroll handle for the visible Logs list, driving follow and pause.
    log_scroll: gpui_kit::UniformListScrollHandle,
    /// Dock filter text, typed into the sidebar's input.
    filter_input: Option<Entity<InputState>>,
    /// Collapsed engine groups in the dock.
    collapsed: HashSet<EngineKey>,
    /// Data size per instance, read on demand for the Overview card.
    data_sizes: std::collections::HashMap<InstanceId, u64>,
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
    /// Whether the demo build may show sample rows in the placeholder tabs.
    demo_content: bool,
    _appearance: Option<Subscription>,
    _event_task: Option<gpui_kit::Task<()>>,
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
        let runtimes = service.list_runtimes().unwrap_or_default();
        let focus_handle = cx.focus_handle();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let filter_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(strings::UI_DOCK_FILTER.to_owned()));
        cx.observe(&filter_input, |_, _, cx| cx.notify()).detach();
        let mut workspace = Self {
            service,
            events,
            state: ShellState {
                instances,
                downloads: Vec::new(),
                last_failure: None,
                uris: std::collections::HashMap::new(),
            },
            catalog,
            runtimes,
            tabs: Tabs::default(),
            logs: LogStore::new(),
            log_scroll: gpui_kit::UniformListScrollHandle::new(),
            filter_input: Some(filter_input),
            collapsed: HashSet::new(),
            data_sizes: std::collections::HashMap::new(),
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
            demo_content: false,
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

    pub(crate) fn refresh_instances(&mut self) {
        if let Ok(instances) = self.service.list_instances() {
            self.state.replace_instances(instances);
        }
        if let Ok(runtimes) = self.service.list_runtimes() {
            self.runtimes = runtimes;
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
                    if self.visible_logs_following(&id) {
                        self.log_scroll.scroll_to_bottom();
                    }
                }
            }
            UiEvent::InstanceRemoved(id) => {
                // A deleted (or rolled-back) instance is gone for good: drop the
                // row, its tab, the selection, its log buffer and its URI.
                self.state
                    .apply_removed(&id, &mut self.tabs, &mut self.logs);
                if let Err(err) = self.service.watch_logs(&id, false) {
                    self.report_error(&err);
                }
            }
            UiEvent::StopForced(id) => {
                self.toast = Some(SharedString::from(strings::UI_TOAST_STOP_FORCED));
                self.load_logs_for(&id);
            }
            UiEvent::Resync => {
                // A lagged stream may have skipped `InstanceChanged` events, so
                // re-read the truth rather than trusting the delta stream.
                self.refresh_instances();
                if let Ok(catalog) = self.service.catalog() {
                    self.catalog = catalog;
                }
            }
            UiEvent::Closed => {}
        }
    }

    pub(crate) fn instance(&self, id: &InstanceId) -> Option<&InstanceView> {
        self.state.instances.iter().find(|view| &view.id == id)
    }

    pub(crate) fn open_tab(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        self.tabs.open_or_select(id.clone());
        self.load_logs_for(&id);
        if let Err(err) = self.service.watch_logs(&id, true) {
            self.report_error(&err);
        }
        // The URI is fetched once per opened tab and held masked; the plaintext
        // only appears while the user asks for it.
        if self.state.uri(&id).is_none()
            && let Ok(uri) = self.service.connection_uri(&id, UriFormat::Uri)
        {
            self.state.set_uri(&id, &uri);
        }
        self.load_data_size(&id);
        cx.notify();
    }

    /// Loads one instance's tail into its own buffer.
    pub(crate) fn load_logs_for(&mut self, id: &InstanceId) {
        match self.service.tail_logs(id, crate::logs::capacity()) {
            Ok(lines) => self.logs.load(id, lines),
            Err(err) => self.report_error(&err),
        }
    }

    /// Reads the data directory size once per instance, on first open.
    pub(crate) fn load_data_size(&mut self, id: &InstanceId) {
        if self.data_sizes.contains_key(id) {
            return;
        }
        if let Ok(size) = self.service.data_size(id) {
            self.data_sizes.insert(id.clone(), size);
        }
    }

    /// Whether the currently visible Logs tab is this instance and following.
    fn visible_logs_following(&self, id: &InstanceId) -> bool {
        let Some(tab) = self.tabs.selected_tab() else {
            return false;
        };
        tab.instance == *id
            && tab.inner == InnerTab::Logs
            && self
                .logs
                .get(id)
                .is_some_and(|buffer| buffer.is_following())
    }

    pub(crate) fn start_instance(&mut self, id: InstanceId) {
        match self.service.start_instance(&id) {
            Ok(_) => self.toast = Some(SharedString::from(strings::UI_TOAST_START_REQUESTED)),
            Err(err) => self.report_error(&err),
        }
    }

    pub(crate) fn stop_instance(&mut self, id: InstanceId) {
        match self.service.stop_instance(&id) {
            Ok(_) => self.toast = Some(SharedString::from(strings::UI_TOAST_STOP_REQUESTED)),
            Err(err) => self.report_error(&err),
        }
    }

    /// Start/stop for the visible tab, which the `Ctrl+Enter` action runs.
    pub(crate) fn toggle_run_selected(&mut self) {
        let Some(tab) = self.tabs.selected_tab() else {
            return;
        };
        let id = tab.instance.clone();
        let Some(view) = self.instance(&id) else {
            return;
        };
        if crate::format::can_start(view.status) {
            self.start_instance(id);
        } else if crate::format::can_stop(view.status) {
            self.stop_instance(id);
        }
    }

    pub(crate) fn report_error(&mut self, err: &dblocal_core::DblError) {
        let facing = UserFacingError::from(err);
        self.toast = Some(SharedString::from(format!(
            "{} — {}",
            facing.code, facing.message
        )));
        self.last_failure_mirror = Some(facing);
    }

    pub(crate) fn copy_uri(
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

    /// Turns the plaintext on or off for one tab, fetching it only to reveal.
    pub(crate) fn toggle_reveal(&mut self, id: &InstanceId) {
        let revealed = self.state.uri(id).is_some_and(UriView::is_revealed);
        if revealed {
            if let Some(uri) = self.state.uri_mut(id) {
                uri.hide();
            }
            return;
        }
        match self.service.connection_uri(id, UriFormat::Uri) {
            Ok(secret) => {
                if let Some(uri) = self.state.uri_mut(id) {
                    uri.reveal(secret);
                } else {
                    self.state.set_uri(id, &secret);
                }
            }
            Err(err) => self.report_error(&err),
        }
    }

    pub(crate) fn copy_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .tabs
            .selected_tab()
            .and_then(|tab| self.logs.get(&tab.instance))
            .map(|buffer| buffer.copy_text())
            .unwrap_or_default();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast = Some(SharedString::from(strings::UI_TOAST_LOGS_COPIED));
        window.push_notification(Notification::success(strings::UI_TOAST_LOGS_COPIED), cx);
        cx.notify();
    }

    pub(crate) fn set_autostart(&mut self, id: &InstanceId, enabled: bool, cx: &mut Context<Self>) {
        match self.service.set_autostart(id, enabled) {
            Ok(()) => {
                self.toast = Some(SharedString::from(if enabled {
                    strings::UI_TOAST_AUTOSTART_ON
                } else {
                    strings::UI_TOAST_AUTOSTART_OFF
                }));
                self.refresh_instances();
            }
            Err(err) => self.report_error(&err),
        }
        cx.notify();
    }

    pub(crate) fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let engine = EngineKey::Postgresql;
        let (version, port) = self.default_engine_version(engine);
        let suggested = self.service.suggest_port(engine).ok();
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strings::UI_FIELD_NAME.to_owned()));
        let port_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strings::UI_FIELD_PORT.to_owned()));
        let name = crate::format::suggested_name(engine, &version);
        name_input.update(cx, |state, cx| state.set_value(name, window, cx));
        if let Some(port) = suggested.or(port) {
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

    pub(crate) fn default_engine_version(&self, engine: EngineKey) -> (String, Option<Port>) {
        let version = self
            .catalog
            .engines
            .iter()
            .find(|entry| entry.engine == engine)
            .map(crate::format::default_version)
            .and_then(|version| version.map(|v| v.version.clone()))
            .unwrap_or_default();
        (version, self.service.suggest_port(engine).ok())
    }

    pub(crate) fn choose_engine(
        &mut self,
        engine: EngineKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (version, port) = self.default_engine_version(engine);
        if let Some(form) = self.create.as_mut() {
            form.engine = engine;
            form.version = version.clone();
            form.suggested_port = port;
        }
        // The prototype pre-fills a name and port for the new engine, so a
        // user who only picks a card still gets a submittable form.
        let name = crate::format::suggested_name(engine, &version);
        let port_text = port.map(|p| p.to_string()).unwrap_or_default();
        self.set_create_field_texts(&name, &port_text, window, cx);
        cx.notify();
    }

    pub(crate) fn choose_version(
        &mut self,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = match self.create.as_mut() {
            Some(form) => {
                form.version = version.clone();
                form.engine
            }
            None => return,
        };
        let name = crate::format::suggested_name(engine, &version);
        self.set_create_field_texts(&name, "", window, cx);
        cx.notify();
    }

    /// Writes prefill text into the dialog's inputs without touching the entity
    /// while a borrow of `self.create` is live.
    fn set_create_field_texts(
        &mut self,
        name: &str,
        port: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name_input, port_input) = match self.create.as_ref() {
            Some(form) => (form.name_input.clone(), form.port_input.clone()),
            None => return,
        };
        if let Some(input) = name_input {
            input.update(cx, |state, cx| state.set_value(name.to_owned(), window, cx));
        }
        if let Some(input) = port_input.filter(|_| !port.is_empty()) {
            input.update(cx, |state, cx| state.set_value(port.to_owned(), window, cx));
        }
    }

    pub(crate) fn choose_auth(&mut self, auth: AuthMode) {
        if let Some(form) = self.create.as_mut() {
            form.auth = auth;
        }
    }

    pub(crate) fn cancel_create(&mut self) {
        self.create = None;
    }

    pub(crate) fn submit_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.create.take() else {
            return;
        };
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
                window.push_notification(Notification::error(strings::ERR_INVALID_INPUT), cx);
            }
            Err(create_submit::SubmitError::Service(err)) => {
                self.create = Some(form);
                self.report_error(&err);
            }
        }
        cx.notify();
    }

    pub(crate) fn open_delete_dialog(
        &mut self,
        id: InstanceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(strings::UI_DELETE_NAME_PLACEHOLDER.to_owned())
        });
        self.delete_target = Some(id.clone());
        self.delete_mode = DeleteMode::ArchiveData;
        self.delete_name_input = Some(name_input);
        self.delete_size = self.service.data_size(&id).unwrap_or(0);
        cx.notify();
    }

    pub(crate) fn delete_confirmation_matches(&self, cx: &App) -> bool {
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

    pub(crate) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
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
                self.toast = Some(SharedString::from(strings::UI_TOAST_DELETED));
                self.refresh_instances();
            }
            Err(err) => self.report_error(&err),
        }
        cx.notify();
    }

    fn close_tab_by_instance(&mut self, id: &InstanceId) {
        if let Some(closed) = self.tabs.close_by_instance(id) {
            if let Err(err) = self.service.watch_logs(&closed.instance, false) {
                self.report_error(&err);
            }
            self.logs.close(&closed.instance);
            // Closing the tab drops the plaintext, not only the render.
            self.state.forget_uri(&closed.instance);
        }
    }

    /// Closes the tab at `index`, withdrawing its log buffer and URI.
    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(closed) = self.tabs.close(index) {
            if let Err(err) = self.service.watch_logs(&closed.instance, false) {
                self.report_error(&err);
            }
            self.logs.close(&closed.instance);
            self.state.forget_uri(&closed.instance);
        }
        cx.notify();
    }

    /// One service call, kept here so `workspace_api` needs no field access.
    pub(crate) fn service_suggest_port(&self, engine: EngineKey) -> Option<u16> {
        self.service
            .suggest_port(engine)
            .ok()
            .map(|port| port.get())
    }

    /// Reads the connection URI for one instance in the requested format.
    pub(crate) fn service_connection_uri(
        &self,
        id: &InstanceId,
        format: UriFormat,
    ) -> dblocal_core::Result<dblocal_core::secret::SecretString> {
        self.service.connection_uri(id, format)
    }

    /// Moves an instance to another port.
    pub(crate) fn service_change_port(
        &mut self,
        id: &InstanceId,
        port: Port,
    ) -> dblocal_core::Result<()> {
        self.service.change_port(id, port)
    }

    /// The three status-bar counts.
    pub(crate) fn counts(&self) -> crate::format::InstanceCounts {
        self.state.counts()
    }

    /// Replaces the toast line the workspace shows.
    pub(crate) fn set_toast(&mut self, text: SharedString) {
        self.toast = Some(text);
    }

    pub(crate) fn close_modals(&mut self) {
        self.palette_open = false;
        self.create = None;
        self.delete_target = None;
        self.delete_name_input = None;
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
            .on_action(cx.listener(|this, _: &actions::CopyUri, window, cx| {
                if let Some(id) = this.tabs.selected_instance().cloned() {
                    this.copy_uri(id, UriFormat::Uri, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &actions::ToggleRun, _window, cx| {
                this.toggle_run_selected();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &actions::CloseOverlay, _window, cx| {
                this.close_modals();
                cx.notify();
            }))
            .child(self.render_title_bar(cx))
            .when_some(self.offline_banner(cx), |this, banner| this.child(banner))
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

/// Splits a `.env` block into rows the preview renders, masking the secret.
///
/// When the card is hidden the block is masked against the URI the view holds,
/// so both `DATABASE_URL`/`REDIS_URL` userinfo and the raw/percent-encoded
/// password in `*PASSWORD` are covered, not only the two known key prefixes.
pub(crate) fn masked_env_rows(
    block: &str,
    revealed: bool,
    uri: Option<&dblocal_core::secret::SecretString>,
) -> Vec<crate::format::EnvLine> {
    let text = if revealed {
        block.to_owned()
    } else {
        match uri {
            Some(uri) => crate::format::mask_dotenv_with_secret(block, uri.expose()),
            None => crate::format::mask_dotenv(block),
        }
    };
    crate::format::env_lines(&text)
}
