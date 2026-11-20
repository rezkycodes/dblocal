//! GPUI window bootstrap, the workspace entity, and the event pump.
//!
//! Rendering stays thin: pure logic lives in [`crate::format`], the log buffer
//! in [`crate::logs`], and the service bridge in [`crate::service_bridge`].
//! This module owns the entity state and the element tree; the submodules
//! render the title bar, status bar, command palette, and the two modals.

mod actions;
mod create_dialog;
mod delete_dialog;
mod palette;
mod status_bar;
mod title_bar;

use std::sync::Arc;
use std::time::Instant;

use dblocal_core::model::{
    AuthMode, CatalogStatus, DeleteMode, DownloadProgress, EngineKey, InstanceId, InstanceSpec,
    InstanceStatus, Port,
};
use dblocal_core::service::{
    CatalogView, DblService, FailureInfo, InstanceView, OperationId, OperationStage, UriFormat,
    UserFacingError,
};
use dblocal_core::strings;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla,
    IntoElement, ParentElement, Render, SharedString, Subscription, Window, WindowAppearance,
    WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Root, Sizable, WindowExt,
    button::{Button, ButtonVariants as _},
    command::CommandState,
    h_flex,
    input::InputState,
    notification::Notification,
    tab::{Tab, TabBar},
    v_flex,
};

use crate::format::{self, PaletteAction};
use crate::logs::LogBuffer;
use crate::service_bridge::{self, UiEvent};

pub use actions::{ROOT_CONTEXT, palette_hint};

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

    // ----- Rendering -------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let groups = format::group_by_engine(&self.instances);
        let mut body = v_flex().gap_3().p_2();
        if groups.is_empty() {
            body = body.child(
                div()
                    .px_2()
                    .py_3()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(strings::UI_WELCOME_SUBTITLE),
            );
        }
        for group in groups {
            let mut rows = v_flex().gap_1();
            for view in group.instances {
                rows = rows.child(self.render_instance_row(view, cx));
            }
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(group.label.to_uppercase()),
                    )
                    .child(rows),
            );
        }

        div()
            .id("sidebar")
            .w(px(260.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(strings::UI_INSTANCES.to_uppercase()),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                div()
                    .p_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .child(
                        Button::new("sidebar-new")
                            .label(strings::UI_NEW_INSTANCE_FOOTER)
                            .small()
                            .w_full()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_create_dialog(window, cx);
                            })),
                    ),
            )
    }

    fn render_instance_row(&self, view: &InstanceView, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected_instance.as_ref() == Some(&view.id);
        let dot = status_colour(view.status, &theme);
        let id = view.id.clone();
        let name = view.name.clone();
        let meta = format::instance_meta(view);
        let label = format::status_label(view.status);
        div()
            .id(SharedString::from(format!("row-{}", view.id)))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py(px(5.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.open_tab(id.clone(), cx);
            }))
            .child(div().size(px(8.)).rounded_full().bg(dot))
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(label),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(meta),
                    ),
            )
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.open_tabs.is_empty() {
            return div()
                .flex_1()
                .h_full()
                .min_w_0()
                .bg(theme.background)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(strings::UI_WELCOME_TITLE),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(strings::UI_WELCOME_SUBTITLE),
                )
                .child(
                    Button::new("welcome-create")
                        .label(strings::UI_CREATE_FIRST)
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_create_dialog(window, cx);
                        })),
                );
        }

        let mut tabs = TabBar::new("center-tabs").underline();
        for tab in &self.open_tabs {
            let name = self
                .instance(&tab.instance)
                .map(|view| view.name.clone())
                .unwrap_or_else(|| strings::UI_INSTANCES.to_owned());
            let status = self
                .instance(&tab.instance)
                .map(|view| view.status)
                .unwrap_or(InstanceStatus::Stopped);
            let dot = status_colour(status, &theme);
            tabs = tabs.child(
                Tab::new()
                    .label(name)
                    .prefix(div().mr_1().size(px(8.)).rounded_full().bg(dot)),
            );
        }
        let tabs = tabs.selected_index(self.selected_tab).on_click(cx.listener(
            |this, index: &usize, _window, cx| {
                this.selected_tab = *index;
                if let Some(tab) = this.open_tabs.get(*index).cloned() {
                    this.selected_instance = Some(tab.instance);
                }
                cx.notify();
            },
        ));

        let content = self.render_selected_tab(cx).into_any_element();

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(theme.background)
            .child(
                div()
                    .w_full()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(tabs),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
    }

    fn render_selected_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(tab) = self.open_tabs.get(self.selected_tab).cloned() else {
            return div().into_any_element();
        };
        let Some(view) = self.instance(&tab.instance).cloned() else {
            return div().into_any_element();
        };
        let inner = TabBar::new("inner-tabs")
            .underline()
            .child(Tab::new().label(strings::UI_TAB_OVERVIEW))
            .child(Tab::new().label(strings::UI_TAB_LOGS))
            .selected_index(match tab.inner {
                InnerTab::Overview => 0,
                InnerTab::Logs => 1,
            })
            .on_click(cx.listener(|this, index: &usize, _window, cx| {
                if let Some(tab) = this.open_tabs.get_mut(this.selected_tab) {
                    tab.inner = if *index == 0 {
                        InnerTab::Overview
                    } else {
                        InnerTab::Logs
                    };
                }
                cx.notify();
            }));

        let body = match tab.inner {
            InnerTab::Overview => self.render_overview(&view, cx).into_any_element(),
            InnerTab::Logs => self.render_logs(cx).into_any_element(),
        };

        v_flex()
            .size_full()
            .child(
                div()
                    .w_full()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(inner),
            )
            .child(body)
            .into_any_element()
    }

    fn render_overview(&self, view: &InstanceView, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dot = status_colour(view.status, &theme);
        let label = format::status_label(view.status);
        let id_start = view.id.clone();
        let id_stop = view.id.clone();
        let id_copy = view.id.clone();
        let id_env = view.id.clone();
        let id_delete = view.id.clone();
        let uri = self
            .service
            .connection_uri(&view.id, UriFormat::Uri)
            .map(|secret| format::mask_uri(secret.expose()))
            .unwrap_or_default();
        let mut sections = v_flex().id("overview").size_full().p_6().gap_4();

        sections = sections.child(
            h_flex()
                .items_center()
                .gap_3()
                .child(div().size(px(10.)).rounded_full().bg(dot))
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(view.name.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .when(view.auth == AuthMode::None, |this| {
                    this.child(
                        div()
                            .px_2()
                            .py(px(2.))
                            .rounded_full()
                            .bg(theme.warning)
                            .text_color(theme.warning_foreground)
                            .text_xs()
                            .child(strings::BADGE_NO_AUTH),
                    )
                }),
        );

        if let Some(failure) = &view.failure {
            sections = sections.child(render_failure(failure, cx));
        }

        sections = sections.child(
            div()
                .p_4()
                .rounded(px(6.))
                .bg(theme.muted)
                .border_1()
                .border_color(theme.border)
                .child(
                    v_flex()
                        .gap_2()
                        .child(info_row(
                            strings::UI_OVERVIEW_VERSION,
                            format!("{} {}", view.engine.display_name(), view.version),
                            theme.muted_foreground,
                        ))
                        .child(info_row(
                            strings::UI_OVERVIEW_PORT,
                            view.port.to_string(),
                            theme.muted_foreground,
                        ))
                        .child(info_row(
                            strings::UI_OVERVIEW_HOST,
                            "127.0.0.1".to_owned(),
                            theme.muted_foreground,
                        ))
                        .child(info_row(
                            strings::UI_OVERVIEW_URI,
                            uri,
                            theme.muted_foreground,
                        )),
                ),
        );

        let can_start = matches!(
            view.status,
            InstanceStatus::Stopped | InstanceStatus::Failed
        );
        let can_stop = view.status == InstanceStatus::Running;
        let can_delete = matches!(
            view.status,
            InstanceStatus::Stopped | InstanceStatus::Failed
        );

        sections.child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("start-{}", view.id)))
                        .label(strings::UI_START)
                        .compact()
                        .primary()
                        .disabled(!can_start)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.start_instance(id_start.clone());
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("stop-{}", view.id)))
                        .label(strings::UI_STOP)
                        .compact()
                        .secondary()
                        .disabled(!can_stop)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.stop_instance(id_stop.clone());
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("copy-uri-{}", view.id)))
                        .label(strings::UI_COPY_URI)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_uri(id_copy.clone(), UriFormat::Uri, window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("copy-env-{}", view.id)))
                        .label(strings::UI_COPY_DOTENV)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_uri(id_env.clone(), UriFormat::DotEnv, window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("delete-{}", view.id)))
                        .label(strings::UI_DELETE)
                        .compact()
                        .danger()
                        .disabled(!can_delete)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_delete_dialog(id_delete.clone(), window, cx);
                        })),
                ),
        )
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let lines: Vec<SharedString> = (0..self.logs.len())
            .filter_map(|index| self.logs.line(index).map(SharedString::from))
            .collect();
        let count = lines.len();
        let follow_label = if self.logs.is_following() {
            strings::UI_LOGS_FOLLOWING
        } else {
            strings::UI_LOGS_PAUSED
        };

        let body = if count == 0 {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(strings::UI_LOGS_EMPTY)
                .into_any_element()
        } else {
            gpui::uniform_list("log-lines", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        div()
                            .id(("log-line", index))
                            .h(px(18.))
                            .px_3()
                            .font_family(mono.clone())
                            .text_xs()
                            .child(lines[index].clone())
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(follow_label),
                    )
                    .child(
                        Button::new("copy-logs")
                            .label(strings::UI_LOGS_COPY)
                            .xsmall()
                            .compact()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.copy_logs(window, cx);
                            })),
                    ),
            )
            .child(body)
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

fn status_colour(status: InstanceStatus, theme: &gpui_component::Theme) -> Hsla {
    match status {
        InstanceStatus::Running => theme.success,
        InstanceStatus::Failed => theme.danger,
        InstanceStatus::Provisioning
        | InstanceStatus::Initializing
        | InstanceStatus::Starting
        | InstanceStatus::Stopping
        | InstanceStatus::Deleting => theme.warning,
        _ => theme.muted_foreground,
    }
}

fn info_row(label: &str, value: String, muted: Hsla) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(90.))
                .text_sm()
                .text_color(muted)
                .child(label.to_owned()),
        )
        .child(div().text_sm().flex_1().min_w_0().child(value))
}

fn render_failure(failure: &FailureInfo, cx: &mut Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme();
    let mut column = v_flex().gap_1();
    for line in &failure.log_tail {
        column = column.child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(line.clone()),
        );
    }
    v_flex()
        .id("failure-panel")
        .p_4()
        .gap_2()
        .rounded(px(6.))
        .bg(theme.danger)
        .text_color(theme.danger_foreground)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(strings::UI_FAILURE_HEADING),
                )
                .child(div().text_xs().child(format!(
                    "{} {}",
                    strings::UI_FAILURE_CODE,
                    failure.code
                ))),
        )
        .child(div().text_sm().child(failure.message.clone()))
        .child(div().text_xs().child(format!(
            "{}: {}",
            strings::UI_FAILURE_DIAGNOSIS,
            failure.diagnosis.as_str()
        )))
        .child(div().text_xs().child(strings::UI_FAILURE_LOG_TAIL))
        .child(column)
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

/// Opens the GUI with the in-memory mock service.
pub fn run_with_mock() -> dblocal_core::Result<()> {
    let service: Arc<dyn DblService> = Arc::new(dblocal_core::service::mock::MockService::new());
    run_with_service(service)
}

/// Opens the GUI against any [`DblService`] implementation.
pub fn run_with_service(service: Arc<dyn DblService>) -> dblocal_core::Result<()> {
    gpui_platform::application()
        .with_assets(gpui_kit_assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            crate::theme::init(cx);
            actions::bind_keys(cx);

            let bounds = gpui::Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui_component::TitleBar::title_bar_options()),
                kind: WindowKind::Normal,
                window_min_size: Some(size(px(900.), px(600.))),
                ..Default::default()
            };

            let service_for_window = service.clone();
            let result = cx.open_window(options, move |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(service_for_window.clone(), window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            });

            match result {
                Ok(_handle) => {
                    cx.activate(true);
                }
                Err(err) => {
                    tracing::error!("could not open the main window: {err:#}");
                    cx.quit();
                }
            }
        });
    Ok(())
}

/// Returns whether a system appearance is dark.
pub fn appearance_is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

impl Workspace {
    /// Builds a preview populated from a seeded mock service.
    ///
    /// Preview constructors drive the headless render tests and let reviewers
    /// exercise every shell state without a live session.
    pub fn preview(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::new(service, window, cx);
        if let Some(first) = workspace.instances.first().map(|view| view.id.clone()) {
            workspace.open_tab(first, cx);
        }
        let failure_id = workspace
            .instances
            .iter()
            .find(|view| view.status == InstanceStatus::Failed)
            .map(|view| view.id.clone());
        if let Some(id) = failure_id {
            if let Some(position) = workspace
                .open_tabs
                .iter()
                .position(|tab| tab.instance == id)
            {
                workspace.selected_tab = position;
            } else {
                workspace.open_tabs.push(OpenTab {
                    instance: id.clone(),
                    inner: InnerTab::Overview,
                });
                workspace.selected_tab = workspace.open_tabs.len() - 1;
            }
            workspace.selected_instance = Some(id);
        }
        workspace
    }

    /// Preview with the command palette open.
    pub fn preview_palette(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.palette_open = true;
        workspace
    }

    /// Preview on the Logs tab, with lines already in the buffer.
    pub fn preview_logs(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        if let Some(tab) = workspace.open_tabs.first_mut() {
            tab.inner = InnerTab::Logs;
        }
        workspace.logs.replace_with(demo_log_lines());
        workspace
    }

    /// Preview with the create modal open.
    pub fn preview_create(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        workspace.open_create_dialog(window, cx);
        workspace
    }

    /// Preview focused on a failed instance, exercising the diagnosis panel.
    pub fn preview_failure(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self::preview(service, window, cx);
        let failure = FailureInfo {
            code: "DBL-1005".to_owned(),
            message: strings::ERR_PORT_IN_USE.to_owned(),
            diagnosis: dblocal_core::model::StartFailure::PortInUse,
            log_tail: vec![
                "[error] bind failed: port 5434 in use".to_owned(),
                "[error] could not start listener".to_owned(),
            ],
        };
        if let Some(view) = workspace
            .instances
            .iter_mut()
            .find(|view| view.status == InstanceStatus::Failed)
        {
            view.failure = Some(failure);
            let id = view.id.clone();
            if let Some(tab) = workspace.open_tabs.iter_mut().find(|t| t.instance == id) {
                tab.inner = InnerTab::Overview;
            } else {
                workspace.open_tabs.push(OpenTab {
                    instance: id.clone(),
                    inner: InnerTab::Overview,
                });
                workspace.selected_tab = workspace.open_tabs.len() - 1;
            }
            workspace.selected_instance = Some(id);
        }
        workspace
    }
}

/// Deterministic log lines for preview renders.
pub fn demo_log_lines() -> Vec<String> {
    (0..64)
        .map(|index| match index % 4 {
            0 => format!("[info] baris log contoh {index}"),
            1 => format!("[debug] health check ok ({index}ms)"),
            2 => "[info] listening on 127.0.0.1 only".to_string(),
            _ => format!("[debug] checkpoint complete: wrote {index} buffers"),
        })
        .collect()
}
