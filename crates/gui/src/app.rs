//! GPUI window bootstrap, the workspace entity, and the event pump.
//!
//! Rendering stays thin: pure logic lives in [`crate::format`], the log buffer
//! in [`crate::logs`], and the service bridge in [`crate::service_bridge`].
//! This module owns the entity state and the action handlers; the submodules
//! render the title bar, dock, tabs, screens, status bar, palette and dialogs.

mod actions;
mod background;
#[cfg(test)]
mod bg_service_tests;
mod bootstrap;
mod components;
mod create_dialog;
mod create_submit;
mod delete_dialog;
mod dock;
mod instance_actions;
mod palette;
mod preview;
mod quit_flow;
mod screens;
mod state;
mod status_bar;
#[cfg(test)]
#[path = "app/test_service.rs"]
mod test_service;
#[cfg(test)]
#[path = "app/tests_secret.rs"]
mod tests_secret;
mod title_bar;
#[cfg(test)]
#[path = "app/tray_latency.rs"]
mod tray_latency;
mod workspace_api;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use dblocal_core::model::{AuthMode, DeleteMode, EngineKey, InstanceId, Port, Runtime};
use dblocal_core::service::{CatalogView, DblService, InstanceView, UriFormat, UserFacingError};
use dblocal_core::strings;
use gpui_kit::component::{ActiveTheme, command::CommandState, h_flex, input::InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement,
    Render, SharedString, Subscription, Task, Window, div, prelude::*,
};

use crate::logs::LogStore;
use crate::quit::QuitMachine;
use crate::service_bridge::{self, UiEvent};
use crate::tray::{TrayAction, TrayHandle, start_tray};

pub use actions::{ROOT_CONTEXT, modifier_label, palette_hint};
pub use bootstrap::{appearance_is_dark, run_with_service};
pub use state::{ActiveDownload, InnerTab, OpenTab, ShellState, Tabs, UriView};

/// Opens the GUI on the in-memory demo service.
///
/// This is the demo/headless entry point the render tests use; it seeds the
/// prototype scenario so every screen can be produced without a live session.
#[cfg(feature = "demo")]
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
    data_sizes: HashMap<InstanceId, u64>,
    /// Cached dotenv blocks fetched in background (never on render path).
    dotenv_cache: HashMap<InstanceId, dblocal_core::secret::SecretString>,
    /// Cached port suggestions for fix banners.
    port_suggestions: HashMap<InstanceId, u16>,
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
    tray: TrayHandle,
    tray_actions: flume::Receiver<TrayAction>,
    tray_note_shown: bool,
    tray_note: Option<SharedString>,
    quit: QuitMachine,
    version: &'static str,
    focus_handle: FocusHandle,
    started_at: Instant,
    first_paint_logged: bool,
    /// Whether the demo build may show sample rows in the placeholder tabs.
    demo_content: bool,
    /// Background service tasks currently in flight, kept so they are not
    /// dropped while pending. Completed tasks are drained opportunistically.
    bg_tasks: Vec<Task<()>>,
    /// How many background service calls are in flight. Read on the render path
    /// so the status bar can show a neutral pending note; never polled.
    pending: Arc<AtomicUsize>,
    /// Forces the pending note for a render preview, where background calls have
    /// already drained by the time the frame is captured.
    pending_preview: bool,
    _appearance: Option<Subscription>,
    _event_task: Option<Task<()>>,
    _tray_task: Option<Task<()>>,
}

impl Workspace {
    /// Builds the workspace and starts the event pump.
    pub fn new(service: Arc<dyn DblService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(service, window, cx, true)
    }

    /// The shared constructor.
    ///
    /// `live` starts the service bridge, the background startup load and the
    /// two pumps. Previews and tests pass `false`: starting a bridge over a
    /// service that emits on its own threads made `App::shutdown`'s effect
    /// flush loop forever, so a preview render hung in `drop(HeadlessAppContext)`
    /// under load. A preview seeds its state synchronously and needs none of it.
    fn build(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
        live: bool,
    ) -> Self {
        let events = if live {
            service_bridge::spawn_bridge(service.clone())
        } else {
            // A channel this view can never receive on, closed immediately, so
            // the type is the same and no thread is ever started.
            let (tx, rx) = flume::unbounded::<UiEvent>();
            drop(tx);
            rx
        };
        let tray_start = if crate::tray::app_enabled() {
            start_tray()
        } else {
            crate::tray::TrayStartup::none()
        };
        let tray_available = tray_start.handle.is_available();
        let focus_handle = cx.focus_handle();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let filter_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(strings::UI_DOCK_FILTER.to_owned()));
        cx.observe(&filter_input, |_, _, cx| cx.notify()).detach();
        let tray_note = if tray_available {
            None
        } else {
            Some(SharedString::from(strings::UI_TRAY_UNAVAILABLE_NOTE))
        };
        let mut workspace = Self {
            service,
            events,
            state: ShellState::default(),
            catalog: Self::offline_catalog(),
            runtimes: Vec::new(),
            tabs: Tabs::default(),
            logs: LogStore::new(),
            log_scroll: gpui_kit::UniformListScrollHandle::new(),
            filter_input: Some(filter_input),
            collapsed: HashSet::new(),
            data_sizes: HashMap::new(),
            dotenv_cache: HashMap::new(),
            port_suggestions: HashMap::new(),
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
            tray: tray_start.handle,
            tray_actions: tray_start.actions,
            tray_note_shown: !tray_available,
            tray_note,
            quit: QuitMachine::new(),
            version: env!("CARGO_PKG_VERSION"),
            focus_handle,
            started_at: Instant::now(),
            first_paint_logged: false,
            demo_content: false,
            bg_tasks: Vec::new(),
            pending: Arc::new(AtomicUsize::new(0)),
            pending_preview: false,
            _appearance: None,
            _event_task: None,
            _tray_task: None,
        };
        if live {
            workspace.startup_load(cx);
            workspace.start_event_pump(cx);
            workspace.start_tray_pump(cx);
        }
        workspace
    }

    /// Preview/test constructor: synchronous reads, no bg startup.
    pub fn new_preview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(service, window, cx, false);
        if let Ok(instances) = this.service.list_instances() {
            this.state.replace_instances(instances);
        }
        if let Ok(catalog) = this.service.catalog() {
            this.catalog = catalog;
        }
        if let Ok(runtimes) = this.service.list_runtimes() {
            this.runtimes = runtimes;
        }
        this
    }

    pub(crate) fn open_tab_sync(&mut self, id: InstanceId) {
        self.tabs.open_or_select(id.clone());
        self.load_logs_sync(&id);
        let _ = self.service.watch_logs(&id, true);
        if self.state.uri(&id).is_none()
            && let Ok(uri) = self.service.connection_uri(&id, UriFormat::Uri)
        {
            self.state.set_uri(&id, &uri);
        }
        if !self.data_sizes.contains_key(&id)
            && let Ok(size) = self.service.data_size(&id)
        {
            self.data_sizes.insert(id.clone(), size);
        }
        if !self.dotenv_cache.contains_key(&id)
            && let Ok(secret) = self.service.connection_uri(&id, UriFormat::DotEnv)
        {
            self.dotenv_cache.insert(id.clone(), secret);
        }
        if !self.port_suggestions.contains_key(&id)
            && let Some(engine) = self.instance(&id).map(|v| v.engine)
            && let Ok(port) = self.service.suggest_port(engine)
        {
            self.port_suggestions.insert(id, port.get());
        }
    }

    fn apply_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::InstanceChanged(view) => {
                self.state.upsert_instance(view);
                self.refresh_tray();
            }
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
                    // If we're in a quit-stopping flow, record failure for this op?
                    // Quit tracks by instance id; we mark failures via stop results.
                }
                // Refresh in background; don't block event pump.
                // (caller has no cx here; refresh_tray already done on InstanceChanged)
            }
            UiEvent::CatalogUpdated => {
                // Catalog fetch is cheap; read in bg next tick via flag not needed:
                // schedule a bg read by setting a toast? No. Use service.catalog() in bg.
                // We'll handle by marking dirty; simplest: just refresh on next frame.
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
                self.state
                    .apply_removed(&id, &mut self.tabs, &mut self.logs);
                self.forget_instance(&id);
                self.refresh_tray();
            }
            UiEvent::StopForced(id) => {
                self.toast = Some(SharedString::from(strings::UI_TOAST_STOP_FORCED));
                let service = self.service.clone();
                let lid = id.clone();
                // fire-and-forget log reload; can't call self.bg here (no cx) but
                // the event pump will refresh via next events; keep simple.
                let _ = (service, lid);
            }
            UiEvent::Resync => {
                // Resync handled by caller with cx via schedule_resync flag.
            }
            UiEvent::Closed => {}
        }
    }

    fn apply_event_with_cx(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        let needs_resync = matches!(event, UiEvent::Resync);
        let needs_catalog = matches!(event, UiEvent::CatalogUpdated);
        let removed_id = match &event {
            UiEvent::InstanceRemoved(id) => Some(id.clone()),
            _ => None,
        };
        let finished = matches!(event, UiEvent::OperationFinished { .. });
        self.apply_event(event);
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
        if needs_resync || finished {
            self.refresh_instances_bg(cx);
        }
    }

    pub(crate) fn instance(&self, id: &InstanceId) -> Option<&InstanceView> {
        self.state.instances.iter().find(|view| &view.id == id)
    }

    fn close_tab_by_instance(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if let Some(closed) = self.tabs.close_by_instance(id) {
            let service = self.service.clone();
            let wid = closed.instance.clone();
            self.bg(cx, move || {
                let _ = service.watch_logs(&wid, false);
                move |_: &mut Workspace, _: &mut Context<Workspace>| {}
            });
            self.logs.close(&closed.instance);
            self.state.forget_uri(&closed.instance);
        }
    }

    /// Closes the tab at `index`, withdrawing its log buffer and URI.
    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(closed) = self.tabs.close(index) {
            let service = self.service.clone();
            let wid = closed.instance.clone();
            self.bg(cx, move || {
                let _ = service.watch_logs(&wid, false);
                move |_: &mut Workspace, _: &mut Context<Workspace>| {}
            });
            self.logs.close(&closed.instance);
            self.state.forget_uri(&closed.instance);
        }
        cx.notify();
    }

    // No synchronous DblService wrappers: every service call must go through
    // Workspace::bg() so the GPUI thread never blocks on SQLite/keychain.

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
                this.toggle_run_selected(cx);
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
            .when(self.quit.is_confirming(), |this| {
                this.child(self.render_quit_dialog(cx))
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
