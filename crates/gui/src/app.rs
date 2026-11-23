//! GPUI window bootstrap, the workspace entity, and the event pump.
//!
//! Rendering stays thin: pure logic lives in [`crate::format`], the log buffer
//! in [`crate::logs`], and the service bridge in [`crate::service_bridge`].
//! This module owns the entity state and the action handlers; the submodules
//! render the title bar, dock, tabs, screens, status bar, palette and dialogs.

mod actions;
mod admin_actions;
mod admin_dialogs;
#[cfg(test)]
#[path = "app/admin_fix_tests.rs"]
mod admin_fix_tests;
mod admin_state;
mod background;
#[cfg(test)]
mod bg_service_tests;
pub mod bootstrap;
#[cfg(test)]
#[path = "app/bootstrap_r3_tests.rs"]
mod bootstrap_r3_tests;
#[cfg(test)]
#[path = "app/bootstrap_tests.rs"]
mod bootstrap_tests;
mod components;
mod config_actions;
mod config_dialogs;
#[cfg(test)]
#[path = "app/config_fix_tests.rs"]
mod config_fix_tests;
#[cfg(test)]
#[path = "app/config_layout_tests.rs"]
mod config_layout_tests;
mod config_state;
#[cfg(test)]
#[path = "app/config_tests.rs"]
mod config_tests;
mod create_dialog;
mod create_submit;
#[cfg(test)]
#[path = "app/databases_support.rs"]
mod databases_support;
#[cfg(test)]
#[path = "app/databases_tests.rs"]
mod databases_tests;
mod delete_dialog;
mod dialog_focus;
mod dock;
mod events;
mod instance_actions;
#[cfg(test)]
#[path = "app/instance_actions_tests.rs"]
mod instance_actions_tests;
mod login_item;
mod modal_focus;
mod open_in;
mod open_in_actions;
#[cfg(test)]
mod open_in_tests;
mod palette;
mod preview;
mod quit_flow;
#[cfg(test)]
#[path = "app/quit_flow_tests.rs"]
mod quit_flow_tests;
mod screens;
#[cfg(test)]
mod scripted_service;
mod settings_actions;
mod settings_dialog;
mod settings_state;
#[cfg(test)]
mod settings_tests;
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
mod tray_paths;
mod workspace_api;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use dblocal_core::admin::PrivilegeLevel;
use dblocal_core::clients::DetectedClient;
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

pub use actions::{ROOT_CONTEXT, modifier_label, palette_hint};
pub use bootstrap::{appearance_is_dark, run_autostart_with_service, run_with_service};
pub use state::{ActiveDownload, InnerTab, OpenTab, ShellState, Tabs, UriView};

/// Opens the GUI on the in-memory demo service.
///
/// This is the demo/headless entry point the render tests use; it seeds the
/// prototype scenario so every screen can be produced without a live session.
#[cfg(feature = "demo")]
pub fn run_with_mock() -> dblocal_core::Result<()> {
    bootstrap::run_with_mock()
}

/// The two Databases-tab text fields, built with the workspace so every entry
/// path (click, preview, keyboard, restored tab) finds them.
fn databases_tab_inputs(
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> (Entity<InputState>, Entity<InputState>) {
    let db = cx.new(|cx| {
        InputState::new(window, cx).placeholder(strings::UI_DATABASES_NAME_PLACEHOLDER.to_owned())
    });
    let user = cx.new(|cx| {
        InputState::new(window, cx).placeholder(strings::UI_DATABASES_USER_NAME.to_owned())
    });
    (db, user)
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
    /// Per-engine-card keyboard focus handles, in catalog order. Owned by the
    /// form so the view and the keyboard tests share one handle per card rather
    /// than each reaching into window element state (`F-PLH-01` AC1).
    pub engine_focus: Vec<(EngineKey, FocusHandle)>,
    /// Per-version-row keyboard focus handles, for the currently selected
    /// engine's versions, in list order.
    pub version_focus: Vec<(String, FocusHandle)>,
    /// The footer "Buat" (create) button's focus handle.
    pub submit_focus: FocusHandle,
    /// The footer "Batal" (cancel) button's focus handle.
    pub cancel_focus: FocusHandle,
}

pub use create_submit::{CreateDraft, SubmitError, Submitted};

/// The main workspace entity.
pub struct Workspace {
    service: Arc<dyn DblService>,
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
    /// Total DBLocal home size in bytes, computed in background.
    home_size_bytes: Option<u64>,
    /// DBLocal home root for status bar display.
    home_root: std::path::PathBuf,
    /// The user's home directory, computed once for path abbreviation.
    home_dir: Option<std::path::PathBuf>,
    home_size_in_flight: bool,
    home_size_dirty: bool,
    home_size_last: Option<Instant>,
    create: Option<CreateForm>,
    delete_target: Option<InstanceId>,
    delete_name_input: Option<Entity<InputState>>,
    /// Text typed into the delete confirmation, mirrored so the pure check has
    /// one source and the tests need no window.
    delete_typed_name: Option<String>,
    delete_mode: DeleteMode,
    delete_size: u64,
    /// Per-instance Databases tab state (`F-DBU-01..03`).
    admin: admin_state::AdminState,
    /// Per-instance Config tab state (`F-CFG-01`, `F-CFG-02`).
    config: config_state::ConfigState,
    /// An instance whose restore-confirmation dialog is open (`F-CFG-01` AC3).
    config_restore_confirm: Option<InstanceId>,
    /// Text fields the Config tab edits, one per (`instance`, `key`), so a field
    /// survives switching tabs and two instances never share one.
    config_fields: HashMap<(InstanceId, String), Entity<InputState>>,
    /// An instance the Config tab asked to restart after an apply; the restart
    /// runs when its stop completes, never a new service call (`F-CFG-01` AC2).
    config_restart_after_stop: Option<InstanceId>,
    /// The operation id of the stop that pending restart queued, so only that
    /// stop can consume it and a failed stop drops it (`F2`).
    config_restart_stop_op: Option<dblocal_core::service::OperationId>,
    /// The database-create field of the Databases tab.
    ///
    /// Created once with the workspace, not on first tab click: the tab can be
    /// reached by the tab bar, a preview, a keyboard action or a restored inner
    /// tab, and every one of those paths needs a field that exists.
    db_name_input: Entity<InputState>,
    /// The new-user name field; created once, as `db_name_input` is.
    user_name_input: Entity<InputState>,
    /// The database a new user is granted on.
    user_db_choice: Option<String>,
    /// The privilege level a new user gets.
    user_level: PrivilegeLevel,
    /// A drop-database dialog: the instance, the database, and the typed text.
    drop_dialog: Option<admin_dialogs::DropDialog>,
    /// A reset-password confirmation is open for this instance.
    reset_confirm: Option<InstanceId>,
    /// The one-time secret dialog (new user password or reset URI).
    secret_dialog: Option<admin_dialogs::SecretDialog>,
    /// Keyboard focus for the Databases dialogs: take it on open, give it back
    /// on close (F3).
    dialog_focus: dialog_focus::DialogFocus,
    /// A focus return the next frame must apply; set by a close path that had no
    /// window (Escape) and consumed by `render`.
    pending_focus_restore: Option<FocusHandle>,
    /// The container focus handle of the topmost open modal, if any (`F2`).
    ///
    /// Created when a modal opens and dropped when it closes; the modal's render
    /// registers a `focus_trap` on it and `open_*` uses it to put the keyboard
    /// inside the dialog immediately (so a dialog of only buttons — Settings,
    /// Quit — takes focus without a click). `None` when no modal is open.
    modal_focus: Option<FocusHandle>,
    /// Clients detected for the "Open in…" menu; `None` until the first read.
    clients: Option<Result<Vec<DetectedClient>, UserFacingError>>,
    /// An instance whose "Open in…" launch is in flight.
    opening: Option<InstanceId>,
    /// The Settings dialog's data (`F-PKG-05`, `F-AUT-01`).
    settings: settings_state::SettingsState,
    /// The login-item seam; `Err` when this host has no usable backend.
    login: Option<Result<Arc<dyn login_item::LoginItemControl>, UserFacingError>>,
    palette_open: bool,
    command_state: Entity<CommandState>,
    last_failure_mirror: Option<UserFacingError>,
    toast: Option<SharedString>,
    tray_available: bool,
    tray_note_shown: bool,
    tray_note: Option<SharedString>,
    quit: QuitMachine,
    /// Quit-issued stop operations and their outcomes, ordering-safe (R2).
    quit_stops: crate::quit::StopLedger,
    /// Wall-clock bound for quit-stop completion; injectable for tests.
    quit_stop_timeout: Duration,
    /// Task that fires the quit timeout.
    quit_timeout_task: Option<Task<()>>,
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
    tray_needs_refresh: bool,
    _appearance: Option<Subscription>,
    /// Fires when the window is activated, so the visible Config tab re-reads a
    /// file something else may have changed (`F7a`).
    _activation: Option<Subscription>,
    _event_task: Option<Task<()>>,
}

/// The owned pieces `Workspace::assemble` takes, so the constructor stays one
/// readable call per concern instead of an eight-argument function.
struct Parts {
    service: Arc<dyn DblService>,
    tray_available: bool,
    command_state: Entity<CommandState>,
    filter_input: Entity<InputState>,
    db_name_input: Entity<InputState>,
    user_name_input: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl Workspace {
    /// Builds a live workspace attached to the app-level runtime.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (service, tray_available) = {
            let runtime = cx.global::<crate::app::bootstrap::AppRuntime>();
            (runtime.service.clone(), runtime.tray_available())
        };
        Self::build_live(service, tray_available, window, cx)
    }

    /// Live constructor wired to an existing service/bridge.
    fn build_live(
        service: Arc<dyn DblService>,
        tray_available: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let filter_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(strings::UI_DOCK_FILTER.to_owned()));
        cx.observe(&filter_input, |_, _, cx| cx.notify()).detach();
        let (db_name_input, user_name_input) = databases_tab_inputs(window, cx);
        let mut workspace = Self::assemble(Parts {
            service,
            tray_available,
            command_state,
            filter_input,
            db_name_input,
            user_name_input,
            focus_handle,
        });
        workspace.startup_load(cx);
        // `F7a`: the external editor DBLocal launches, the CLI and a second
        // window all write the file without an in-process `ConfigChanged`. On
        // the way back to this window the visible Config tab re-reads, so the
        // form is never stale.
        workspace._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh_config_on_activation(cx);
            }
        }));
        // `F-PLH-02` AC1: follow the system appearance while running. A change
        // to the OS light/dark setting re-applies the matching theme, unless an
        // explicit `DBLOCAL_THEME` override is set (then the override wins).
        workspace._appearance = Some(window.observe_window_appearance(|window, cx| {
            if crate::theme::theme_override().is_none() {
                let wanted = if crate::app::appearance_is_dark(window.appearance()) {
                    crate::theme::DARK_THEME
                } else {
                    crate::theme::LIGHT_THEME
                };
                crate::theme::apply_named(cx, wanted);
            }
        }));
        workspace
    }

    /// Preview/test constructor: synchronous reads, no bg startup, no app global.
    fn build_preview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let filter_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(strings::UI_DOCK_FILTER.to_owned()));
        cx.observe(&filter_input, |_, _, cx| cx.notify()).detach();
        let (db_name_input, user_name_input) = databases_tab_inputs(window, cx);
        let mut workspace = Self::assemble(Parts {
            service,
            tray_available: false,
            command_state,
            filter_input,
            db_name_input,
            user_name_input,
            focus_handle,
        });
        workspace.tray_note_shown = true;
        workspace.tray_note = Some(SharedString::from(strings::UI_TRAY_UNAVAILABLE_NOTE));
        // The same activation re-read the live window installs (`F7a`), so a
        // test can prove the return-to-window path.
        workspace._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh_config_on_activation(cx);
            }
        }));
        workspace
    }

    /// The freshly built, unmoved pieces `assemble` needs.
    fn assemble(parts: Parts) -> Self {
        let tray_note = if parts.tray_available {
            None
        } else {
            Some(SharedString::from(strings::UI_TRAY_UNAVAILABLE_NOTE))
        };
        Self {
            service: parts.service,
            tray_available: parts.tray_available,
            tray_note_shown: !parts.tray_available,
            tray_note,
            command_state: parts.command_state,
            filter_input: Some(parts.filter_input),
            focus_handle: parts.focus_handle,
            state: ShellState::default(),
            catalog: Self::offline_catalog(),
            runtimes: Vec::new(),
            tabs: Tabs::default(),
            logs: LogStore::new(),
            log_scroll: gpui_kit::UniformListScrollHandle::new(),
            collapsed: HashSet::new(),
            data_sizes: HashMap::new(),
            dotenv_cache: HashMap::new(),
            port_suggestions: HashMap::new(),
            home_size_bytes: None,
            // Documented fallback only: `from_env` resolves the real root, so the
            // literal never reaches a user-facing path.
            home_root: dblocal_core::paths::DblPaths::from_env()
                .map(|p| p.root().to_path_buf())
                .unwrap_or_else(|_| std::path::PathBuf::from("~/.dblocal")),
            home_dir: workspace_api::home_dir_from_env(),
            home_size_in_flight: false,
            home_size_dirty: false,
            home_size_last: None,
            create: None,
            delete_target: None,
            delete_name_input: None,
            delete_typed_name: None,
            delete_mode: DeleteMode::ArchiveData,
            delete_size: 0,
            admin: admin_state::AdminState::default(),
            config: config_state::ConfigState::default(),
            config_restore_confirm: None,
            config_fields: HashMap::new(),
            config_restart_after_stop: None,
            config_restart_stop_op: None,
            db_name_input: parts.db_name_input,
            user_name_input: parts.user_name_input,
            user_db_choice: None,
            user_level: PrivilegeLevel::ReadOnly,
            drop_dialog: None,
            reset_confirm: None,
            secret_dialog: None,
            dialog_focus: dialog_focus::DialogFocus::default(),
            pending_focus_restore: None,
            modal_focus: None,
            clients: None,
            opening: None,
            settings: settings_state::SettingsState::default(),
            login: None,
            palette_open: false,
            last_failure_mirror: None,
            toast: None,
            quit: QuitMachine::new(),
            quit_stops: crate::quit::StopLedger::new(),
            quit_stop_timeout: crate::quit::QUIT_STOP_TIMEOUT,
            quit_timeout_task: None,
            version: env!("CARGO_PKG_VERSION"),
            started_at: Instant::now(),
            first_paint_logged: false,
            demo_content: false,
            bg_tasks: Vec::new(),
            pending: Arc::new(AtomicUsize::new(0)),
            pending_preview: false,
            tray_needs_refresh: false,
            _appearance: None,
            _activation: None,
            _event_task: None,
        }
    }

    /// Preview/test constructor: synchronous reads, no bg startup.
    pub fn new_preview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build_preview(service, window, cx);
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

    pub(crate) fn instance(&self, id: &InstanceId) -> Option<&InstanceView> {
        self.state.instances.iter().find(|view| &view.id == id)
    }

    /// Whether this instance's Databases tab is the one on screen.
    ///
    /// The tab must be both selected and showing Databases: a row re-read for a
    /// tab the user has on Overview would be work nobody sees, and the tab is
    /// re-read on entry anyway.
    pub(crate) fn database_tab_visible(&self, id: &InstanceId) -> bool {
        self.tabs
            .selected_tab()
            .is_some_and(|tab| &tab.instance == id && tab.inner == InnerTab::Databases)
    }

    /// Whether this instance's Config tab is the one on screen.
    pub(crate) fn config_tab_visible(&self, id: &InstanceId) -> bool {
        self.tabs
            .selected_tab()
            .is_some_and(|tab| &tab.instance == id && tab.inner == InnerTab::Config)
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
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_paint_logged {
            self.first_paint_logged = true;
            tracing::info!(
                "FIRST_FRAME_MS={:.2}",
                self.started_at.elapsed().as_secs_f64() * 1000.0
            );
        }
        let deleting = self.delete_target.is_some();
        let creating = self.create.is_some();
        // F3: a close from a listener with no window (Escape) parks the handle
        // here; the frame is the first place it can be focused.
        if let Some(target) = self.pending_focus_restore.take() {
            target.focus(window, cx);
        }
        // `F1`: when nothing holds focus (startup, or a dismissal whose opener
        // is gone, or a background event that dropped focus) the keyboard is
        // dead — Tab and every shortcut do nothing until a mouse click. Focus
        // the root once so `ctrl-k`/`ctrl-n` work on a fresh window. Only when
        // nothing is focused, so it never steals focus from a control the user
        // or a dialog placed.
        if window.focused(cx).is_none() {
            let root = self.focus_handle.clone();
            root.focus(window, cx);
        }
        // The Config tab's text fields are GPUI entities created once; build any
        // the visible tab needs before the frame reads them (`F-CFG-01`).
        if let Some(tab) = self.tabs.selected_tab()
            && tab.inner == InnerTab::Config
        {
            let id = tab.instance.clone();
            self.ensure_config_fields(&id, window, cx);
        }
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
            .on_action(cx.listener(|this, _: &actions::OpenPalette, window, cx| {
                this.toggle_palette_in(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &actions::OpenSettings, window, cx| {
                this.open_settings(window, cx);
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
                    .child(self.render_sidebar(window, cx))
                    .child(self.render_center(window, cx)),
            )
            .child(self.render_status_bar(cx))
            .when(creating, |this| {
                this.child(create_dialog::render(self, window, cx))
            })
            .when(deleting, |this| {
                this.child(delete_dialog::render(self, window, cx))
            })
            .when(self.palette_open, |this| {
                this.child(palette::render(self, window, cx))
            })
            .when(self.settings.open, |this| {
                this.child(settings_dialog::render(self, window, cx))
            })
            .when(self.drop_dialog.is_some(), |this| {
                this.child(admin_dialogs::render_drop(self, window, cx))
            })
            .when(self.reset_confirm.is_some(), |this| {
                this.child(admin_dialogs::render_reset(self, window, cx))
            })
            .when(self.secret_dialog.is_some(), |this| {
                this.child(admin_dialogs::render_secret(self, window, cx))
            })
            .when(self.config_restore_confirm.is_some(), |this| {
                this.child(config_dialogs::render_restore(self, window, cx))
            })
            .when(self.quit.is_confirming(), |this| {
                this.child(self.render_quit_dialog(window, cx))
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

#[cfg(test)]
#[path = "app/event_tests.rs"]
mod event_tests;

#[cfg(test)]
#[path = "app/keyboard_flow_tests.rs"]
mod keyboard_flow_tests;
