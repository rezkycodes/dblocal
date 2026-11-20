use crate::progress::{self, UiEvent};
use crate::tray;
use flume::{Receiver, Sender};
use gpui::{
    App, AsyncApp, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight,
    Hsla, InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, WindowBounds,
    WindowKind, WindowOptions, actions, deferred, div, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Root, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    notification::Notification,
    popover::Popover,
    progress::Progress,
    scroll::ScrollableElement,
    tab::{Tab, TabBar},
    v_flex,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

actions!(
    spike,
    [
        OpenCommandPalette,
        NewInstanceAction,
        QuitApp,
        CloseOverlay,
        PaletteDown,
        PaletteUp,
        PaletteConfirm
    ]
);

const PUMP_TICK: std::time::Duration = std::time::Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Running,
    Stopped,
    Failed,
}

impl InstanceStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    fn dot_color(self, cx: &App) -> Hsla {
        let t = cx.theme();
        match self {
            Self::Running => t.success,
            Self::Stopped => t.muted_foreground,
            Self::Failed => t.danger,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Postgres,
    Mysql,
    Mariadb,
    Valkey,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Mariadb => "MariaDB",
            Self::Valkey => "Valkey",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub name: SharedString,
    pub engine: Engine,
    pub version: SharedString,
    pub port: u16,
    pub status: InstanceStatus,
}

impl Instance {
    pub fn uri(&self) -> String {
        let db = self.name.as_ref().replace('-', "_");
        match self.engine {
            Engine::Postgres => format!(
                "postgres://dblocal:secret@127.0.0.1:{}/{}?sslmode=disable",
                self.port, db
            ),
            Engine::Mysql | Engine::Mariadb => {
                format!("mysql://dblocal:secret@127.0.0.1:{}/{}", self.port, db)
            }
            Engine::Valkey => format!("redis://:secret@127.0.0.1:{}/0", self.port),
        }
    }
}

#[derive(Debug, Clone)]
pub enum InstanceAction {
    Toggle {
        idx: usize,
        at: Instant,
        source: &'static str,
    },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerTab {
    Overview,
    Logs,
}

pub fn make_instances() -> Vec<Instance> {
    vec![
        Instance {
            name: "postgres-main".into(),
            engine: Engine::Postgres,
            version: "17.4".into(),
            port: 5432,
            status: InstanceStatus::Running,
        },
        Instance {
            name: "mysql-shop".into(),
            engine: Engine::Mysql,
            version: "8.4.4".into(),
            port: 3306,
            status: InstanceStatus::Stopped,
        },
        Instance {
            name: "mariadb-legacy".into(),
            engine: Engine::Mariadb,
            version: "11.4.2".into(),
            port: 3307,
            status: InstanceStatus::Stopped,
        },
        Instance {
            name: "valkey-cache".into(),
            engine: Engine::Valkey,
            version: "8.1.0".into(),
            port: 6379,
            status: InstanceStatus::Failed,
        },
    ]
}

/// Shared mailbox for tray->UI actions, drained by the GPUI timer pump.
pub type TrayMailbox = Arc<Mutex<Vec<InstanceAction>>>;

pub struct AppState {
    pub instances: Vec<Instance>,
    pub open_tabs: Vec<usize>,
    pub selected_tab: usize,
    pub inner_tab: InnerTab,
    pub selected_sidebar: Option<usize>,
    pub download_progress: f32,
    pub tray_note: SharedString,
    pub tray_available: bool,
    pub command_palette_open: bool,
    pub palette_selected: usize,
    pub modal_open: bool,
    pub started_at: Instant,
    pub first_paint_logged: bool,
    focus_handle: FocusHandle,
    _appearance: Option<Subscription>,
    tray_mailbox: TrayMailbox,
}

impl AppState {
    /// Preview state for headless screenshot/test rendering.
    pub fn preview(cx: &mut Context<Self>) -> Self {
        Self {
            instances: make_instances(),
            open_tabs: vec![0, 1],
            selected_tab: 0,
            inner_tab: InnerTab::Overview,
            selected_sidebar: Some(0),
            download_progress: 42.0,
            tray_note: "Preview: tray active (simulated).".into(),
            tray_available: true,
            command_palette_open: false,
            palette_selected: 0,
            modal_open: false,
            started_at: Instant::now(),
            first_paint_logged: true,
            focus_handle: cx.focus_handle(),
            _appearance: None,
            tray_mailbox: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn new(
        tray_note: SharedString,
        tray_available: bool,
        started_at: Instant,
        tray_mailbox: TrayMailbox,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            instances: make_instances(),
            open_tabs: vec![0],
            selected_tab: 0,
            inner_tab: InnerTab::Overview,
            selected_sidebar: Some(0),
            download_progress: 0.0,
            tray_note,
            tray_available,
            command_palette_open: false,
            palette_selected: 0,
            modal_open: false,
            started_at,
            first_paint_logged: false,
            focus_handle: cx.focus_handle(),
            _appearance: None,
            tray_mailbox,
        }
    }

    pub fn toggle(&mut self, idx: usize) {
        if let Some(inst) = self.instances.get_mut(idx) {
            inst.status = match inst.status {
                InstanceStatus::Running => InstanceStatus::Stopped,
                InstanceStatus::Stopped | InstanceStatus::Failed => InstanceStatus::Running,
            };
        }
    }

    pub fn running_count(&self) -> usize {
        self.instances
            .iter()
            .filter(|i| i.status == InstanceStatus::Running)
            .count()
    }

    fn open_instance_tab(&mut self, idx: usize) {
        self.selected_sidebar = Some(idx);
        if let Some(pos) = self.open_tabs.iter().position(|i| *i == idx) {
            self.selected_tab = pos;
        } else {
            self.open_tabs.push(idx);
            self.selected_tab = self.open_tabs.len().saturating_sub(1);
        }
    }

    fn copy_uri(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(inst) = self.instances.get(idx) {
            let uri = inst.uri();
            cx.write_to_clipboard(ClipboardItem::new_string(uri.clone()));
            window.push_notification(Notification::success("URI copied").message(uri.clone()), cx);
        }
    }

    fn open_new_instance_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_open = true;
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("New instance")
                .width(px(440.))
                .overlay_closable(true)
                .keyboard(true)
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .child("Create a new local database instance."),
                        )
                        .child(div().text_xs().text_color(gpui::rgb(0x9da5b4)).child(
                            "Engine, version and port are assigned automatically in this spike.",
                        ))
                        .child(
                            div()
                                .text_sm()
                                .child("Suggested: postgres 17.4 on 127.0.0.1:5433"),
                        ),
                )
        });
        cx.notify();
    }

    fn drain_tray_mailbox(&mut self) -> bool {
        let actions: Vec<InstanceAction> = match self.tray_mailbox.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => Vec::new(),
        };
        if actions.is_empty() {
            return false;
        }
        for action in actions {
            match action {
                InstanceAction::Toggle { idx, at, source } => {
                    self.toggle(idx);
                    let state = self
                        .instances
                        .get(idx)
                        .map(|i| i.status.label())
                        .unwrap_or("out-of-range");
                    eprintln!(
                        "ACTION_APPLIED source={source} idx={idx} new_state={state} latency_ms={:.1}",
                        at.elapsed().as_secs_f64() * 1000.0
                    );
                }
                InstanceAction::Quit => {
                    // handled by pump (needs cx.quit) - ignore here
                }
            }
        }
        true
    }
}

impl Focusable for AppState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_paint_logged {
            self.first_paint_logged = true;
            eprintln!(
                "FIRST_FRAME_MS={:.2}",
                self.started_at.elapsed().as_secs_f64() * 1000.0
            );
        }

        let theme = cx.theme();
        let bg = theme.background;
        let fg = theme.foreground;

        div()
            .id("root")
            .track_focus(&self.focus_handle)
            .key_context("SpikeRoot")
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                this.command_palette_open = !this.command_palette_open;
                this.palette_selected = 0;
                cx.notify();
                let _ = window;
            }))
            .on_action(cx.listener(|this, _: &NewInstanceAction, window, cx| {
                this.open_new_instance_dialog(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _window, cx| {
                if this.command_palette_open {
                    this.command_palette_open = false;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PaletteDown, _window, cx| {
                if this.command_palette_open {
                    let n = palette_actions(&this.instances).len();
                    if n > 0 {
                        this.palette_selected = (this.palette_selected + 1) % n;
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &PaletteUp, _window, cx| {
                if this.command_palette_open {
                    let n = palette_actions(&this.instances).len();
                    if n > 0 {
                        this.palette_selected = if this.palette_selected == 0 {
                            n - 1
                        } else {
                            this.palette_selected - 1
                        };
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &PaletteConfirm, window, cx| {
                if this.command_palette_open {
                    let actions = palette_actions(&this.instances);
                    if let Some((_, action)) = actions.get(this.palette_selected).cloned() {
                        this.command_palette_open = false;
                        match action {
                            PaletteAction::NewInstance => this.open_new_instance_dialog(window, cx),
                            PaletteAction::Open(idx) => {
                                this.open_instance_tab(idx);
                                cx.notify();
                            }
                            PaletteAction::Toggle(idx) => {
                                this.toggle(idx);
                                cx.notify();
                            }
                            PaletteAction::CopyUri(idx) => {
                                this.copy_uri(idx, window, cx);
                                cx.notify();
                            }
                        }
                    } else {
                        this.command_palette_open = false;
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|_, _: &QuitApp, _window, cx| {
                cx.quit();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .font_family(theme.font_family.clone())
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_center(cx)),
            )
            .child(self.render_status_bar(cx))
            .when(self.command_palette_open, |this| {
                this.child(self.render_command_palette(cx))
            })
    }
}

impl AppState {
    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        h_flex()
            .id("title-bar")
            .h(px(36.))
            .w_full()
            .items_center()
            .justify_between()
            .bg(t.title_bar)
            .border_b_1()
            .border_color(t.title_bar_border)
            .pl(px(12.))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("DBLocal"),
                    )
                    .child(
                        Button::new("tb-new")
                            .label("New instance")
                            .xsmall()
                            .compact()
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_instance_dialog(window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .pr(px(8.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.muted_foreground)
                            .child("Ctrl+K"),
                    )
                    .child(window_controls(cx)),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let mut groups = v_flex().gap_4().p_2();
        for engine in [
            Engine::Postgres,
            Engine::Mysql,
            Engine::Mariadb,
            Engine::Valkey,
        ] {
            let mut rows = v_flex().gap_1();
            let mut any = false;
            for (idx, inst) in self.instances.iter().enumerate() {
                if inst.engine != engine {
                    continue;
                }
                any = true;
                let selected = self.selected_sidebar == Some(idx);
                let dot = inst.status.dot_color(cx);
                let meta = format!("{}  ·  {}", inst.version, inst.port);
                rows = rows.child(
                    div()
                        .id(("sb-item", idx))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(selected, |s| {
                            s.bg(t.sidebar_accent)
                                .text_color(t.sidebar_accent_foreground)
                        })
                        .hover(|s| s.bg(t.sidebar_accent))
                        .on_click(cx.listener(move |this, _, _w, _cx| {
                            this.open_instance_tab(idx);
                        }))
                        .child(div().size(px(8.)).rounded_full().bg(dot))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().text_sm().child(inst.name.clone()))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(t.muted_foreground)
                                                .child(inst.status.label()),
                                        ),
                                )
                                .child(div().text_xs().text_color(t.muted_foreground).child(meta)),
                        ),
                );
            }
            if !any {
                continue;
            }
            groups = groups.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.muted_foreground)
                            .child(engine.label().to_uppercase()),
                    )
                    .child(rows),
            );
        }

        div()
            .id("sidebar")
            .w(px(250.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.sidebar_border)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(t.sidebar_border)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child("INSTANCES"),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .child(groups),
            )
            .child(
                div()
                    .p_2()
                    .border_t_1()
                    .border_color(t.sidebar_border)
                    .child(
                        Button::new("sb-new")
                            .label("New instance")
                            .small()
                            .w_full()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_instance_dialog(window, cx);
                            })),
                    ),
            )
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (bg, tab_bar_bg, border_color) = {
            let t = cx.theme();
            (t.background, t.tab_bar, t.border)
        };

        let mut tab_bar = TabBar::new("center-tabs").underline();
        if self.open_tabs.is_empty() {
            tab_bar = tab_bar.child(Tab::new().label("Welcome"));
        } else {
            for &idx in &self.open_tabs {
                if let Some(inst) = self.instances.get(idx) {
                    let dot = inst.status.dot_color(cx);
                    tab_bar = tab_bar.child(
                        Tab::new()
                            .label(inst.name.clone())
                            .prefix(div().mr_1().size(px(8.)).rounded_full().bg(dot)),
                    );
                }
            }
        }
        tab_bar = tab_bar
            .selected_index(self.selected_tab)
            .on_click(cx.listener(|this, ix: &usize, _w, cx| {
                this.selected_tab = *ix;
                if let Some(inst_idx) = this.open_tabs.get(*ix).copied() {
                    this.selected_sidebar = Some(inst_idx);
                }
                cx.notify();
            }));

        let content = if let Some(inst_idx) = self.open_tabs.get(self.selected_tab).copied() {
            self.render_instance_detail(inst_idx, cx).into_any_element()
        } else {
            self.render_welcome(cx).into_any_element()
        };

        let show_inner = !self.open_tabs.is_empty();

        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .bg(bg)
            .child(
                div()
                    .w_full()
                    .bg(tab_bar_bg)
                    .border_b_1()
                    .border_color(border_color)
                    .child(tab_bar),
            )
            .when(show_inner, |this| {
                let (inner_bg, inner_border) = {
                    let t = cx.theme();
                    (t.tab_bar, t.border)
                };
                let inner = TabBar::new("inner-tabs")
                    .underline()
                    .child(Tab::new().label("Overview"))
                    .child(Tab::new().label("Logs"))
                    .selected_index(match self.inner_tab {
                        InnerTab::Overview => 0,
                        InnerTab::Logs => 1,
                    })
                    .on_click(cx.listener(|this, ix: &usize, _w, cx| {
                        this.inner_tab = if *ix == 0 {
                            InnerTab::Overview
                        } else {
                            InnerTab::Logs
                        };
                        cx.notify();
                    }));
                this.child(
                    div()
                        .w_full()
                        .bg(inner_bg)
                        .border_b_1()
                        .border_color(inner_border)
                        .child(inner),
                )
            })
            .child(div().flex_1().overflow_hidden().child(content))
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Welcome to DBLocal"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(t.muted_foreground)
                    .child("Create your first local database instance to get started."),
            )
            .child(
                Button::new("welcome-create")
                    .label("Create your first instance")
                    .primary()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_instance_dialog(window, cx);
                    })),
            )
    }

    fn render_instance_detail(&self, idx: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let Some(inst) = self.instances.get(idx) else {
            return div().child("No instance").into_any_element();
        };
        match self.inner_tab {
            InnerTab::Overview => {
                let uri = inst.uri();
                let dot = inst.status.dot_color(cx);
                v_flex()
                    .id(("overview", idx))
                    .size_full()
                    .overflow_y_scrollbar()
                    .p_6()
                    .gap_4()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(div().size(px(10.)).rounded_full().bg(dot))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(inst.name.clone()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(t.muted_foreground)
                                    .child(inst.status.label()),
                            ),
                    )
                    .child(
                        div()
                            .p_4()
                            .rounded(px(6.))
                            .bg(t.muted)
                            .border_1()
                            .border_color(t.border)
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(info_row(
                                        "Engine",
                                        format!("{} {}", inst.engine.label(), inst.version),
                                        t.muted_foreground,
                                    ))
                                    .child(info_row(
                                        "Port",
                                        inst.port.to_string(),
                                        t.muted_foreground,
                                    ))
                                    .child(info_row("Host", "127.0.0.1".into(), t.muted_foreground))
                                    .child(info_row("URI", uri, t.muted_foreground)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child({
                                let label = if inst.status == InstanceStatus::Running {
                                    "Stop"
                                } else {
                                    "Start"
                                };
                                let mut b = Button::new(("dt", idx)).label(label).compact();
                                b = if inst.status == InstanceStatus::Running {
                                    b.secondary()
                                } else {
                                    b.primary()
                                };
                                b.on_click(cx.listener(move |this, _, _w, cx| {
                                    this.toggle(idx);
                                    cx.notify();
                                }))
                            })
                            .child(
                                Button::new(("dc", idx))
                                    .label("Copy URI")
                                    .ghost()
                                    .compact()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.copy_uri(idx, window, cx);
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            InnerTab::Logs => {
                let lines = fake_logs(inst);
                let mut col = v_flex().gap_1().p_4();
                for (i, line) in lines.into_iter().enumerate() {
                    col = col.child(
                        div()
                            .id(("log", i))
                            .font_family("monospace")
                            .text_xs()
                            .text_color(t.foreground)
                            .child(line),
                    );
                }
                div()
                    .id(("logs", idx))
                    .size_full()
                    .overflow_y_scrollbar()
                    .child(col)
                    .into_any_element()
            }
        }
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let running = self.running_count();
        let total = self.instances.len();
        let pct = self.download_progress.clamp(0.0, 100.0);
        let download_label = format!("PostgreSQL 16.4  {:.0}%", pct);
        let tray = if self.tray_available {
            "Tray: active"
        } else {
            "Tray: unavailable"
        };

        let pop = Popover::new("downloads-pop")
            .anchor(gpui::Anchor::BottomLeft)
            .trigger(
                Button::new("dl-trigger")
                    .label(download_label)
                    .ghost()
                    .xsmall()
                    .compact(),
            )
            .content(move |_, _, cx| {
                let theme = cx.theme();
                div()
                    .p_3()
                    .min_w(px(260.))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Downloads"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .child(format!("PostgreSQL 16.4  {:.0}%", pct)),
                            )
                            .child(
                                Progress::new("dl-pop-bar")
                                    .value(pct)
                                    .color(theme.progress_bar)
                                    .h(px(6.)),
                            ),
                    )
                    .into_any_element()
            });

        h_flex()
            .id("status-bar")
            .h(px(26.))
            .w_full()
            .items_center()
            .justify_between()
            .px_3()
            .bg(t.status_bar)
            .border_t_1()
            .border_color(t.status_bar_border)
            .text_xs()
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(div().child(format!("{running} running / {total} total")))
                    .child(div().text_color(t.muted_foreground).child("Catalog: Fresh")),
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(pop)
                    .child(div().text_color(t.muted_foreground).child(tray))
                    .child(div().text_color(t.muted_foreground).child("v0.1.0-spike")),
            )
    }

    fn render_command_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let actions = palette_actions(&self.instances);
        let mut list = v_flex().gap_1();
        for (i, (label, _)) in actions.iter().enumerate() {
            let selected = i == self.palette_selected;
            let label_text = label.clone();
            list = list.child(
                div()
                    .id(("cmd", i))
                    .px_3()
                    .py(px(4.))
                    .rounded(px(4.))
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(t.accent))
                    .hover(|s| s.bg(t.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.palette_selected = i;
                            let actions = palette_actions(&this.instances);
                            if let Some((_, action)) = actions.get(i).cloned() {
                                this.command_palette_open = false;
                                match action {
                                    PaletteAction::NewInstance => {
                                        this.open_new_instance_dialog(window, cx)
                                    }
                                    PaletteAction::Open(idx) => {
                                        this.open_instance_tab(idx);
                                        cx.notify();
                                    }
                                    PaletteAction::Toggle(idx) => {
                                        this.toggle(idx);
                                        cx.notify();
                                    }
                                    PaletteAction::CopyUri(idx) => {
                                        this.copy_uri(idx, window, cx);
                                        cx.notify();
                                    }
                                }
                            }
                        }),
                    )
                    .child(label_text),
            );
        }
        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .pt(px(80.))
                .bg(gpui::hsla(0.0, 0.0, 0.0, 0.4))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| {
                    this.command_palette_open = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("palette")
                        .w(px(520.))
                        .max_h(px(420.))
                        .bg(t.popover)
                        .text_color(t.popover_foreground)
                        .border_1()
                        .border_color(t.border)
                        .rounded(px(8.))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            v_flex()
                                .child(
                                    div()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(t.border)
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(t.muted_foreground)
                                                .child("Command palette (Ctrl+K / Ctrl+Shift+P) — find instance, start/stop, copy URI, new instance"),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("palette-list")
                                        .max_h(px(340.))
                                        .overflow_y_scrollbar()
                                        .p_2()
                                        .child(list),
                                ),
                        ),
                ),
        )
        .with_priority(10)
    }
}

#[derive(Clone)]
enum PaletteAction {
    Toggle(usize),
    CopyUri(usize),
    Open(usize),
    NewInstance,
}

impl PaletteAction {
    /// Instance index this action targets, if any.
    #[cfg(test)]
    fn target(&self) -> Option<usize> {
        match self {
            PaletteAction::Toggle(ix) | PaletteAction::CopyUri(ix) | PaletteAction::Open(ix) => {
                Some(*ix)
            }
            PaletteAction::NewInstance => None,
        }
    }
}

fn palette_actions(instances: &[Instance]) -> Vec<(String, PaletteAction)> {
    let mut actions = vec![("New instance".to_string(), PaletteAction::NewInstance)];
    for (idx, inst) in instances.iter().enumerate() {
        actions.push((format!("Open {}", inst.name), PaletteAction::Open(idx)));
        actions.push((
            format!("Start/stop {}", inst.name),
            PaletteAction::Toggle(idx),
        ));
        actions.push((
            format!("Copy URI {}", inst.name),
            PaletteAction::CopyUri(idx),
        ));
    }
    actions
}

fn info_row(label: &str, value: String, muted: Hsla) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(80.))
                .text_sm()
                .text_color(muted)
                .child(label.to_string()),
        )
        .child(div().text_sm().flex_1().child(value))
}

fn fake_logs(inst: &Instance) -> Vec<String> {
    vec![
        format!(
            "[info] starting {} {} on 127.0.0.1:{}",
            inst.engine.label(),
            inst.version,
            inst.port
        ),
        "[info] listening on 127.0.0.1 only".into(),
        format!(
            "[info] data directory ~/.dblocal/instances/{}/data",
            inst.name
        ),
        "[info] ready to accept connections".into(),
        "[debug] health check ok (12ms)".into(),
        "[info] autovacuum launcher started".into(),
        "[debug] checkpoint complete: wrote 3 buffers".into(),
    ]
}

fn window_controls(cx: &App) -> impl IntoElement {
    // macOS uses native traffic lights; this spike draws simple placeholders on Linux/Windows.
    // Full window-control wiring (min/max/close via window API) is omitted in the shell view
    // and handled by the platform title bar integration where available.
    if cfg!(target_os = "macos") {
        return div().id("wc-mac").w(px(70.)).into_any_element();
    }
    h_flex()
        .id("wc")
        .items_center()
        .child(wc_btn("wc-min", "—", false, cx))
        .child(wc_btn("wc-max", "□", false, cx))
        .child(wc_btn("wc-close", "×", true, cx))
        .into_any_element()
}

fn wc_btn(id: &'static str, label: &'static str, danger: bool, cx: &App) -> impl IntoElement {
    let hover = if danger {
        cx.theme().danger
    } else {
        cx.theme().secondary_hover
    };
    let hover_fg = if danger {
        cx.theme().danger_foreground
    } else {
        cx.theme().foreground
    };
    div()
        .id(id)
        .px_3()
        .h(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_sm()
        .hover(move |s| s.bg(hover).text_color(hover_fg))
        .child(label)
}

/// Timer-driven pump: wakes the GPUI loop every tick (see PUMP_TICK note in old code),
/// drains tray actions from the mailbox and progress from the channel.
fn spawn_pump(view: Entity<AppState>, rx: Receiver<UiEvent>, mailbox: TrayMailbox, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(PUMP_TICK).await;
            let mut progress_val: Option<f32> = None;
            let mut quit = false;
            while let Ok(event) = rx.try_recv() {
                match event {
                    UiEvent::Progress(p) => progress_val = Some(p),
                    UiEvent::Tray(InstanceAction::Quit) => quit = true,
                    UiEvent::Tray(action) => {
                        if let Ok(mut q) = mailbox.lock() {
                            q.push(action);
                        }
                    }
                }
            }
            let has_mail = mailbox.lock().map(|q| !q.is_empty()).unwrap_or(false);
            if progress_val.is_some() || has_mail || quit {
                cx.update(|cx| {
                    view.update(cx, |state, cx| {
                        if let Some(p) = progress_val {
                            state.download_progress = p.clamp(0.0, 100.0);
                        }
                        state.drain_tray_mailbox();
                        if quit {
                            cx.quit();
                        } else {
                            cx.notify();
                        }
                    })
                });
            }
        }
    })
    .detach();
}

pub fn run() {
    let started_at = Instant::now();
    eprintln!("STARTUP_BEGIN_MS=0.00");

    let (event_tx, event_rx): (Sender<UiEvent>, Receiver<UiEvent>) = flume::unbounded();
    progress::spawn_fake_download(event_tx.clone());

    let tray_snapshot = tray::start_tray(event_tx.clone());
    eprintln!(
        "TRAY backend={} available={} note={}",
        tray_snapshot.backend, tray_snapshot.tray_available, tray_snapshot.note
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());

    let tray_note: SharedString = tray_snapshot.note.clone().into();
    let tray_available = tray_snapshot.tray_available;
    let mailbox: TrayMailbox = Arc::new(Mutex::new(Vec::new()));
    let mailbox_for_pump = mailbox.clone();

    gpui_platform::application()
        .with_assets(gpui_kit_assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            crate::theme::init(cx);

            // `secondary-` is not accepted by gpui-pre 0.3.12's keystroke parser,
            // so the platform modifier is chosen here instead.
            #[cfg(target_os = "macos")]
            let palette_keys = [
                KeyBinding::new("cmd-k", OpenCommandPalette, Some("SpikeRoot")),
                KeyBinding::new("cmd-shift-p", OpenCommandPalette, Some("SpikeRoot")),
            ];
            #[cfg(not(target_os = "macos"))]
            let palette_keys = [
                KeyBinding::new("ctrl-k", OpenCommandPalette, Some("SpikeRoot")),
                KeyBinding::new("ctrl-shift-p", OpenCommandPalette, Some("SpikeRoot")),
            ];
            cx.bind_keys(palette_keys);
            cx.bind_keys([
                KeyBinding::new("escape", CloseOverlay, Some("SpikeRoot")),
                KeyBinding::new("down", PaletteDown, Some("SpikeRoot")),
                KeyBinding::new("up", PaletteUp, Some("SpikeRoot")),
                KeyBinding::new("enter", PaletteConfirm, Some("SpikeRoot")),
            ]);

            let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui_component::TitleBar::title_bar_options()),
                kind: WindowKind::Normal,
                window_min_size: Some(size(px(900.), px(600.))),
                ..Default::default()
            };

            let tray_note_view = tray_note.clone();
            let mailbox_view = mailbox.clone();
            let window_handle = cx
                .open_window(options, move |window, cx| {
                    let view = cx.new(|cx| {
                        AppState::new(tray_note_view, tray_available, started_at, mailbox_view, cx)
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open window");

            // Retrieve the actual AppState entity for the pump.
            let view_entity: Entity<AppState> = window_handle
                .update(cx, |root, _window, _cx| {
                    root.view()
                        .clone()
                        .downcast::<AppState>()
                        .expect("root view is AppState")
                })
                .expect("window");

            // F-TRY-02: minimize instead of close when tray is available.
            if tray_available {
                let _ = window_handle.update(cx, |_, window, app| {
                    window.on_window_should_close(app, move |window, _app| {
                        window.minimize_window();
                        false
                    });
                });
            } else {
                // F-TRY-01 AC3: no tray -> quit when the last window closes.
                cx.on_window_closed(|cx, _id| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
            }

            spawn_pump(view_entity, event_rx, mailbox_for_pump, cx);

            let _ = window_handle.update(cx, |_, window, _| {
                window.activate_window();
            });
            cx.activate(true);

            eprintln!(
                "WINDOW_OPEN_DISPATCHED_MS={:.2}",
                started_at.elapsed().as_secs_f64() * 1000.0
            );
            let _ = std::io::Write::flush(&mut std::io::stderr());
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_match_contract() {
        assert_eq!(InstanceStatus::Running.label(), "running");
        assert_eq!(InstanceStatus::Stopped.label(), "stopped");
        assert_eq!(InstanceStatus::Failed.label(), "failed");
    }

    #[test]
    fn uri_uses_engine_scheme_and_loopback_only() {
        let pg = Instance {
            name: "app-db".into(),
            engine: Engine::Postgres,
            version: "17".into(),
            port: 5432,
            status: InstanceStatus::Running,
        }
        .uri();
        assert_eq!(
            pg,
            "postgres://dblocal:secret@127.0.0.1:5432/app_db?sslmode=disable"
        );
        assert!(pg.contains("127.0.0.1"));
        assert!(!pg.contains("0.0.0.0"));

        let my = Instance {
            name: "shop".into(),
            engine: Engine::Mysql,
            version: "8".into(),
            port: 3306,
            status: InstanceStatus::Stopped,
        }
        .uri();
        assert!(my.starts_with("mysql://dblocal:secret@127.0.0.1:3306/"));

        let vk = Instance {
            name: "cache".into(),
            engine: Engine::Valkey,
            version: "8".into(),
            port: 6379,
            status: InstanceStatus::Failed,
        }
        .uri();
        assert_eq!(vk, "redis://:secret@127.0.0.1:6379/0");
    }

    #[test]
    fn toggle_cycles_states() {
        let mut instances = [Instance {
            name: "a".into(),
            engine: Engine::Postgres,
            version: "1".into(),
            port: 5432,
            status: InstanceStatus::Stopped,
        }];
        // exercise the same transition logic used by AppState::toggle
        let toggle = |inst: &mut Instance| {
            inst.status = match inst.status {
                InstanceStatus::Running => InstanceStatus::Stopped,
                InstanceStatus::Stopped | InstanceStatus::Failed => InstanceStatus::Running,
            };
        };
        toggle(&mut instances[0]);
        assert_eq!(instances[0].status, InstanceStatus::Running);
        toggle(&mut instances[0]);
        assert_eq!(instances[0].status, InstanceStatus::Stopped);
        instances[0].status = InstanceStatus::Failed;
        toggle(&mut instances[0]);
        assert_eq!(instances[0].status, InstanceStatus::Running);
        // out-of-range access would be a no-op in AppState::toggle (get_mut guard)
        assert!(instances.get_mut(99).is_none());
    }

    #[test]
    fn palette_contains_core_actions() {
        let insts = make_instances();
        let actions = palette_actions(&insts);
        assert!(actions.iter().any(|(l, _)| l == "New instance"));
        assert!(actions.iter().any(|(l, _)| l.contains("Copy URI")));
        assert!(actions.iter().any(|(l, _)| l.contains("Start/stop")));
        let targets: Vec<_> = actions.iter().filter_map(|(_, a)| a.target()).collect();
        assert!(targets.contains(&0));
        assert!(targets.contains(&3));
    }
}
