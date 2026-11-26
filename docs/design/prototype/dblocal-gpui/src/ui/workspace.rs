//! The main Zed-style workspace: title bar, Instances dock on the left, a tabbed pane per
//! instance in the centre, and a status bar at the bottom (D-23).

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Sizable, TitleBar, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString,
    Window, actions, div, px,
};

use crate::model::{
    EngineKind, Failure, Instance, LogLevel, LogLine, Status, demo_instances, demo_password,
    next_free_port,
};
use crate::theme::tokens;
use crate::ui::components::{engine_badge, mono, status_dot, status_pill};
use crate::ui::new_instance::{NewInstanceForm, NewInstanceSpec};

actions!(dblocal, [NewInstance, CopyUri, ToggleRun, ToggleReveal]);

const CONTEXT: &str = "Workspace";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewInstance, Some(CONTEXT)),
        KeyBinding::new("cmd-n", NewInstance, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-c", CopyUri, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-c", CopyUri, Some(CONTEXT)),
        KeyBinding::new("ctrl-enter", ToggleRun, Some(CONTEXT)),
        KeyBinding::new("cmd-enter", ToggleRun, Some(CONTEXT)),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Overview,
    Logs,
    Databases,
    Config,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Overview,
        Section::Logs,
        Section::Databases,
        Section::Config,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Logs => "Logs",
            Self::Databases => "Databases",
            Self::Config => "Config",
        }
    }
}

pub struct Workspace {
    pub(crate) instances: Vec<Instance>,
    pub(crate) selected: usize,
    pub(crate) open_tabs: Vec<usize>,
    pub(crate) section: Section,
    pub(crate) reveal: bool,
    pub(crate) collapsed: HashSet<EngineKind>,
    pub(crate) filter: Entity<InputState>,
    pub(crate) new_db_name: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter…"));
        let new_db_name = cx.new(|cx| InputState::new(window, cx).placeholder("nama_database"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self {
            instances: demo_instances(),
            selected: 0,
            open_tabs: vec![0, 3, 2],
            section: Section::Overview,
            reveal: false,
            collapsed: HashSet::new(),
            filter,
            new_db_name,
            focus_handle,
        }
    }

    pub(crate) fn current(&self) -> &Instance {
        &self.instances[self.selected]
    }

    // ---------------------------------------------------------------- aksi

    pub(crate) fn select(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.selected = idx;
        if !self.open_tabs.contains(&idx) {
            self.open_tabs.push(idx);
        }
        if self.instances[idx].status == Status::Failed {
            self.section = Section::Logs;
        }
        cx.notify();
    }

    fn close_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.open_tabs.retain(|&t| t != idx);
        if self.selected == idx {
            if let Some(&next) = self.open_tabs.last() {
                self.selected = next;
            }
        }
        cx.notify();
    }

    pub(crate) fn copy_uri(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let inst = self.current();
        cx.write_to_clipboard(ClipboardItem::new_string(inst.uri(true)));
        let note = if inst.status == Status::Running {
            "Connection URI copied to clipboard".to_string()
        } else {
            format!("URI copied — {} is not running yet", inst.name)
        };
        window.push_notification(Notification::success(note), cx);
    }

    pub(crate) fn copy_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.current().env_block(true)));
        window.push_notification(Notification::success(".env block copied to clipboard"), cx);
    }

    pub(crate) fn copy_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .current()
            .logs
            .iter()
            .rev()
            .take(500)
            .rev()
            .map(|l| format!("{} {:?} {}", l.time, l.level, l.text))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(Notification::info("Last 500 log lines copied"), cx);
    }

    /// Start/Stop with a simulated status transition.
    pub(crate) fn toggle_run(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let inst = &mut self.instances[idx];
        match inst.status {
            Status::Running => {
                inst.status = Status::Stopping;
                inst.logs.push(LogLine::new(&now(), LogLevel::Dbl, "stop requested: pg_ctl stop -m fast"));
                self.after(idx, 700, cx, |i| {
                    i.status = Status::Stopped;
                    i.pid = None;
                    i.logs.push(LogLine::new(&now(), LogLevel::Dbl, "clean shutdown finished"));
                });
            }
            Status::Stopped | Status::Failed => {
                if let Some(Failure::PortInUse { port, .. }) = inst.failure {
                    if port == inst.port {
                        window.push_notification(
                            Notification::error(format!("Port {port} is already used by another process")),
                            cx,
                        );
                        return;
                    }
                }
                inst.status = Status::Starting;
                inst.failure = None;
                let port = inst.port;
                inst.logs.push(LogLine::new(&now(), LogLevel::Dbl, format!("bind test 127.0.0.1:{port} … OK")));
                self.after(idx, 900, cx, move |i| {
                    i.status = Status::Running;
                    i.pid = Some(48_213 + port as u32);
                    i.logs.push(LogLine::new(&now(), LogLevel::Log, format!("listening on IPv4 address \"127.0.0.1\", port {port}")));
                    i.logs.push(LogLine::new(&now(), LogLevel::Dbl, "health check OK"));
                });
            }
            _ => {}
        }
        cx.notify();
    }

    fn restart(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.instances[idx].status != Status::Running {
            return;
        }
        self.instances[idx].status = Status::Stopping;
        self.after(idx, 600, cx, |i| i.status = Status::Starting);
        self.after(idx, 1300, cx, |i| {
            i.status = Status::Running;
            i.logs.push(LogLine::new(&now(), LogLevel::Dbl, "restart finished · health check OK"));
        });
        cx.notify();
    }

    /// F-PRT-02: move to the next free port, then continue the start.
    pub(crate) fn move_port_and_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.selected;
        let Some(Failure::PortInUse { suggested, .. }) = self.instances[idx].failure else {
            return;
        };
        self.instances[idx].port = suggested;
        self.instances[idx].failure = None;
        self.instances[idx]
            .logs
            .push(LogLine::new(&now(), LogLevel::Dbl, format!("port dipindah ke {suggested}")));
        self.toggle_run(idx, window, cx);
    }

    fn after(
        &mut self,
        idx: usize,
        millis: u64,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut Instance) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(millis))
                .await;
            this.update(cx, |ws, cx| {
                if let Some(inst) = ws.instances.get_mut(idx) {
                    apply(inst);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_new_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx.entity().downgrade();
        let taken_ports: Vec<(EngineKind, u16)> =
            self.instances.iter().map(|i| (i.engine, i.port)).collect();
        let form = cx.new(|cx| NewInstanceForm::new(workspace, taken_ports, window, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("New instance")
                .w(px(720.))
                .child(form.clone())
        });
    }

    /// Called by the New Instance dialog. A runtime that is not installed goes
    /// through the `Provisioning` state (download → SHA-256 → extract → initdb).
    pub fn create_instance(&mut self, spec: NewInstanceSpec, cx: &mut Context<Self>) {
        let port = spec
            .port
            .unwrap_or_else(|| next_free_port(&self.instances, spec.engine));
        let installed = spec.installed;
        let inst = Instance {
            name: spec.name.clone(),
            engine: spec.engine,
            version: spec.version.clone(),
            port,
            status: if installed { Status::Stopped } else { Status::Provisioning },
            password: demo_password(self.instances.len() + 11),
            no_auth: spec.no_auth,
            pid: None,
            data_size: "0 B".into(),
            autostart: false,
            progress: 0.,
            failure: None,
            logs: vec![LogLine::new(&now(), LogLevel::Dbl, format!("instance {} created", spec.name))],
            databases: Vec::new(),
        };
        self.instances.push(inst);
        let idx = self.instances.len() - 1;
        self.select(idx, cx);
        self.section = Section::Overview;

        if !installed {
            cx.spawn(async move |this, cx| {
                for step in 1..=20 {
                    cx.background_executor()
                        .timer(Duration::from_millis(140))
                        .await;
                    let done = this
                        .update(cx, |ws, cx| {
                            let Some(inst) = ws.instances.get_mut(idx) else {
                                return true;
                            };
                            inst.progress = step as f32 * 5.;
                            if step == 20 {
                                inst.status = Status::Stopped;
                                inst.data_size = "39,8 MB".into();
                                inst.logs.push(LogLine::new(&now(), LogLevel::Dbl, "SHA-256 matched · runtime extracted · initdb OK"));
                            }
                            cx.notify();
                            step == 20
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    pub(crate) fn engine_counts(&self) -> (usize, usize, usize) {
        let running = self.instances.iter().filter(|i| i.status == Status::Running).count();
        let failed = self.instances.iter().filter(|i| i.status == Status::Failed).count();
        (running, self.instances.len() - running - failed, failed)
    }

    // ------------------------------------------------------------- render

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inst = self.current();
        TitleBar::new().child(
            h_flex()
                .w_full()
                .gap_3()
                .pr_2()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(22.))
                                .rounded(px(5.))
                                .bg(tokens::accent())
                                .child(
                                    gpui_kit::svg()
                                        .path("icons/database.svg")
                                        .size(px(14.))
                                        .text_color(tokens::on_accent()),
                                ),
                        )
                        .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("DBLocal")),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_color(tokens::text_muted())
                        .child(inst.engine.name())
                        .child("›")
                        .child(div().text_color(tokens::text()).child(inst.name.clone())),
                )
                .child(div().flex_1())
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_xs()
                        .text_color(tokens::text_muted())
                        .child(
                            gpui_kit::svg()
                                .path("icons/check.svg")
                                .size(px(14.))
                                .text_color(tokens::ok()),
                        )
                        .child("Catalog updated 3 hours ago"),
                )
                .child(
                    Button::new("title-new")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip("New instance (Ctrl+N)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_new_instance(window, cx)
                        })),
                ),
        )
    }

    fn render_instance_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(36.))
            .flex_none()
            .bg(tokens::bg_panel())
            .border_b_1()
            .border_color(tokens::border_strong())
            .children(self.open_tabs.iter().map(|&idx| {
                let inst = &self.instances[idx];
                let active = idx == self.selected;
                h_flex()
                    .id(("itab", idx))
                    .h_full()
                    .gap_2()
                    .pl(px(14.))
                    .pr(px(8.))
                    .border_r_1()
                    .border_color(tokens::border_strong())
                    .cursor_pointer()
                    .when(active, |s| s.bg(tokens::bg_editor()).text_color(tokens::text()))
                    .when(!active, |s| {
                        s.text_color(tokens::text_muted())
                            .hover(|s| s.text_color(tokens::text()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select(idx, cx)))
                    .child(status_dot(inst.status, 8.))
                    .child(inst.name.clone())
                    .child(
                        Button::new(("close-tab", idx))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Close tab")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(idx, cx)
                            })),
                    )
            }))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inst = self.current();
        let idx = self.selected;
        let busy = inst.status.is_busy();
        let running = inst.status == Status::Running;
        let sub = match inst.status {
            Status::Running => "running · detached".to_string(),
            Status::Failed => "failed to start".to_string(),
            Status::Provisioning => format!("menyiapkan runtime · {:.0}%", inst.progress),
            s => s.label().to_lowercase(),
        };

        h_flex()
            .flex_wrap()
            .gap_4()
            .px(px(32.))
            .pt(px(24.))
            .child(engine_badge(inst.engine, 44.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(240.))
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2p5()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(inst.name.clone()),
                            )
                            .child(status_pill(inst.status)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_color(tokens::text_muted())
                            .child(format!("{} {} ·", inst.engine.name(), inst.version))
                            .child(mono(format!("127.0.0.1:{}", inst.port), cx))
                            .child(format!("· {sub}")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("run")
                            .icon(if running { IconName::Square } else { IconName::Play })
                            .label(if running { "Stop" } else { "Start" })
                            .loading(busy)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_run(idx, window, cx)
                            })),
                    )
                    .child(
                        Button::new("restart")
                            .icon(IconName::RefreshCw)
                            .label("Restart")
                            .disabled(!running)
                            .on_click(cx.listener(move |this, _, _, cx| this.restart(idx, cx))),
                    )
                    .child(
                        Button::new("open-in")
                            .icon(IconName::ExternalLink)
                            .label("Open in…")
                            .dropdown_caret(true)
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.push_notification(
                                    Notification::info("Terdeteksi: psql, TablePlus (Phase 2)"),
                                    cx,
                                )
                            })),
                    ),
            )
    }

    fn render_sections(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = Section::ALL
            .iter()
            .position(|s| *s == self.section)
            .unwrap_or(0);
        div().px(px(32.)).pt(px(14.)).child(
            TabBar::new("sections")
                .underline()
                .selected_index(selected)
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    this.section = Section::ALL[*ix];
                    cx.notify();
                }))
                .children(Section::ALL.iter().map(|s| Tab::new().label(s.label()))),
        )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (running, stopped, failed) = self.engine_counts();
        h_flex()
            .flex_none()
            .h(px(28.))
            .px(px(14.))
            .gap_4()
            .bg(tokens::bg_panel())
            .border_t_1()
            .border_color(tokens::border_strong())
            .text_xs()
            .text_color(tokens::text_muted())
            .child(h_flex().gap_1p5().child(status_dot(Status::Running, 8.)).child(format!("{running} running")))
            .child(h_flex().gap_1p5().child(status_dot(Status::Stopped, 8.)).child(format!("{stopped} stopped")))
            .when(failed > 0, |s| {
                s.child(
                    h_flex()
                        .gap_1p5()
                        .text_color(tokens::danger())
                        .child(status_dot(Status::Failed, 8.))
                        .child(format!("{failed} failed")),
                )
            })
            .child(mono("~/.dblocal · 3,1 GB", cx))
            .child(div().flex_1())
            .child("Credential: keychain")
            .child(std::env::consts::OS.to_string() + " " + std::env::consts::ARCH)
            .child(concat!("v", env!("CARGO_PKG_VERSION")))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.section {
            Section::Overview => self.render_overview(cx).into_any_element(),
            Section::Logs => self.render_logs(cx).into_any_element(),
            Section::Databases => self.render_databases(cx).into_any_element(),
            Section::Config => self.render_config(cx).into_any_element(),
        };

        v_flex()
            .id("workspace")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewInstance, window, cx| {
                this.open_new_instance(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyUri, window, cx| this.copy_uri(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleRun, window, cx| {
                let idx = this.selected;
                this.toggle_run(idx, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleReveal, _, cx| {
                this.reveal = !this.reveal;
                cx.notify();
            }))
            .size_full()
            .bg(tokens::bg_editor())
            .text_color(tokens::text())
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.))
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_instance_tabs(cx))
                            .child(self.render_header(cx))
                            .child(self.render_sections(cx))
                            .child(
                                div()
                                    .id("section-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .child(body),
                            ),
                    ),
            )
            .child(self.render_status_bar(cx))
    }
}

pub(crate) fn now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let secs = (ms / 1000) % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        ms % 1000
    )
}

#[allow(dead_code)]
pub(crate) fn shared(s: impl Into<String>) -> SharedString {
    SharedString::from(s.into())
}
