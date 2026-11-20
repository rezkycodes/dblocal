use crate::progress::{self, UiEvent};
use crate::tray;
use flume::{Receiver, Sender};
use gpui::{
    App, AsyncApp, Bounds, ClipboardItem, Context, Entity, FontWeight, Hsla, IntoElement,
    ParentElement, Render, Rgba, SharedString, Styled, Window, WindowBounds, WindowKind,
    WindowOptions, div, prelude::*, px, rgb, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;
use gpui_component::{ActiveTheme, Theme, ThemeMode};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Running,
    Stopped,
    Failed,
}

impl InstanceStatus {
    fn dot_color(self) -> Hsla {
        match self {
            InstanceStatus::Running => rgb(0x2ecc71).into(),
            InstanceStatus::Stopped => rgb(0x7f8c8d).into(),
            InstanceStatus::Failed => rgb(0xe74c3c).into(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            InstanceStatus::Running => "running",
            InstanceStatus::Stopped => "stopped",
            InstanceStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub name: SharedString,
    pub engine: SharedString,
    pub version: SharedString,
    pub port: u16,
    pub status: InstanceStatus,
}

impl Instance {
    fn uri(&self) -> String {
        match self.engine.as_ref() {
            "postgres" => format!(
                "postgres://dblocal:secret@127.0.0.1:{}/{}?sslmode=disable",
                self.port,
                self.name.as_ref().replace('-', "_")
            ),
            "mysql" | "mariadb" => format!(
                "mysql://dblocal:secret@127.0.0.1:{}/{}",
                self.port,
                self.name.as_ref().replace('-', "_")
            ),
            "valkey" | "redis" => format!("redis://:secret@127.0.0.1:{}/0", self.port),
            _ => format!(
                "{}://127.0.0.1:{}",
                self.engine.as_ref().to_ascii_lowercase(),
                self.port
            ),
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

pub struct AppState {
    pub instances: Vec<Instance>,
    pub download_progress: f32,
    pub tray_note: SharedString,
    pub tray_available: bool,
    pub started_at: Instant,
    pub first_paint_logged: bool,
    pub last_copied: Option<String>,
}

impl AppState {
    /// A view instance for off-screen rendering (screenshot evidence) and tests.
    pub fn preview() -> Self {
        Self {
            instances: make_instances(),
            download_progress: 42.0,
            tray_note: "Preview build: tray status is injected for the screenshot.".into(),
            tray_available: true,
            started_at: Instant::now(),
            first_paint_logged: true,
            last_copied: None,
        }
    }

    fn toggle(&mut self, idx: usize) {
        if let Some(inst) = self.instances.get_mut(idx) {
            inst.status = match inst.status {
                InstanceStatus::Running => InstanceStatus::Stopped,
                InstanceStatus::Stopped | InstanceStatus::Failed => InstanceStatus::Running,
            };
        }
    }

    fn apply_event(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::Progress(pct) => {
                self.download_progress = pct;
                cx.notify();
            }
            UiEvent::Tray(action) => match action {
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
                    cx.notify();
                }
                InstanceAction::Quit => {
                    cx.spawn(async move |_this, cx| {
                        cx.update(|app| app.quit());
                    })
                    .detach();
                }
            },
        }
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_paint_logged {
            self.first_paint_logged = true;
            let elapsed = self.started_at.elapsed();
            eprintln!("FIRST_FRAME_MS={:.2}", elapsed.as_secs_f64() * 1000.0);
        }

        let theme = cx.theme();
        let bg = rgb(0x0e2a2a);
        let card_bg = rgb(0x143535);
        let border = rgb(0x1f4a4a);
        let fg = rgb(0xe6f4f1);
        let muted = rgb(0x8fb3ad);
        let accent = rgb(0x1fa37a);

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .font_family(theme.font_family.clone())
            .child(render_header(
                card_bg,
                border,
                muted,
                accent,
                self.tray_available,
            ))
            .child(self.render_body(cx, card_bg, border, fg, muted, accent))
            .child(self.render_footer(card_bg, border, muted))
    }
}

fn render_header(
    card_bg: Rgba,
    border: Rgba,
    muted: Rgba,
    accent: Rgba,
    tray_available: bool,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(20.))
        .py(px(14.))
        .bg(card_bg)
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().size(px(10.)).rounded_full().bg(accent))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("DBLocal — GPUI spike"),
                ),
        )
        .child(div().text_sm().text_color(muted).child(if tray_available {
            "Tray: active"
        } else {
            "Tray: unavailable (window fallback)"
        }))
}

impl AppState {
    fn render_body(
        &self,
        cx: &mut Context<Self>,
        card_bg: Rgba,
        border: Rgba,
        _fg: Rgba,
        muted: Rgba,
        accent: Rgba,
    ) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap(px(10.));
        for idx in 0..self.instances.len() {
            list = list.child(self.render_instance_row(idx, cx, card_bg, border, muted, accent));
        }

        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().text_color(muted).child(
                        "Instances (fake state). Tray actions update this view via channel.",
                    ))
                    .when_some(self.last_copied.clone(), |this, uri| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(accent)
                                .child(format!("Copied: {uri}")),
                        )
                    }),
            )
            .child(list)
            .child(render_progress(
                self.download_progress,
                card_bg,
                border,
                muted,
            ))
            .child(self.render_tray_note(card_bg, border, muted))
    }

    fn render_instance_row(
        &self,
        idx: usize,
        cx: &mut Context<Self>,
        card_bg: Rgba,
        border: Rgba,
        muted: Rgba,
        _accent: Rgba,
    ) -> impl IntoElement {
        let inst = &self.instances[idx];
        let status = inst.status;
        let uri = inst.uri();
        let uri_for_copy = uri.clone();
        let name = inst.name.clone();
        let subtitle = format!(
            "{} {}  ·  127.0.0.1:{}",
            inst.engine, inst.version, inst.port
        );

        let toggle_label = match status {
            InstanceStatus::Running => "Stop",
            _ => "Start",
        };

        div()
            .id(("instance-row", idx))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .rounded(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().size(px(10.)).rounded_full().bg(status.dot_color()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(div().font_weight(FontWeight::MEDIUM).child(name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(status.label().to_string()),
                                    ),
                            )
                            .child(div().text_xs().text_color(muted).child(subtitle)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child({
                        let mut btn = Button::new(("toggle", idx))
                            .label(toggle_label)
                            .compact()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                            this.toggle(idx);
                            eprintln!(
                                "ACTION_APPLIED source=ui idx={idx} new_state={} latency_ms=0.0",
                                this.instances
                                    .get(idx)
                                    .map(|i| i.status.label())
                                    .unwrap_or("out-of-range")
                            );
                            cx.notify();
                        }));
                        btn = if status == InstanceStatus::Running {
                            btn.secondary()
                        } else {
                            btn.primary()
                        };
                        btn
                    })
                    .child(
                        Button::new(("copy", idx))
                            .label("Copy URI")
                            .ghost()
                            .compact()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    uri_for_copy.clone(),
                                ));
                                this.last_copied = Some(uri_for_copy.clone());
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_footer(&self, card_bg: Rgba, border: Rgba, muted: Rgba) -> impl IntoElement {
        div()
            .px(px(20.))
            .py(px(10.))
            .bg(card_bg)
            .border_t_1()
            .border_color(border)
            .text_xs()
            .text_color(muted)
            .child(
                "GPUI + gpui-component spike · dark teal theme · background thread -> channel -> UI refresh",
            )
    }

    fn render_tray_note(&self, card_bg: Rgba, border: Rgba, muted: Rgba) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .py(px(12.))
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .rounded(px(10.))
            .child(div().text_sm().child("Tray / environment note"))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(self.tray_note.clone()),
            )
    }
}

fn render_progress(value: f32, card_bg: Rgba, border: Rgba, muted: Rgba) -> impl IntoElement {
    let pct = value.clamp(0.0, 100.0);
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .px(px(14.))
        .py(px(12.))
        .bg(card_bg)
        .border_1()
        .border_color(border)
        .rounded(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_sm().child("Background download (simulated)"))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{pct:.0}%")),
                ),
        )
        .child(
            Progress::new("download-progress")
                .value(pct)
                .color(rgb(0x1fa37a))
                .h(px(10.))
                .rounded(px(6.)),
        )
}

fn make_instances() -> Vec<Instance> {
    vec![
        Instance {
            name: "postgres-main".into(),
            engine: "postgres".into(),
            version: "17.4".into(),
            port: 5432,
            status: InstanceStatus::Running,
        },
        Instance {
            name: "mysql-shop".into(),
            engine: "mysql".into(),
            version: "8.4.4".into(),
            port: 3306,
            status: InstanceStatus::Stopped,
        },
        Instance {
            name: "valkey-cache".into(),
            engine: "valkey".into(),
            version: "8.1.0".into(),
            port: 6379,
            status: InstanceStatus::Failed,
        },
    ]
}

/// Pumps background events into the view.
///
/// IMPORTANT (measured; see docs/spikes/gpui/README.md):
/// awaiting a channel on GPUI's foreground executor does NOT wake it on Linux.
/// GPUI's Linux dispatcher runs foreground tasks on a calloop event loop. A task that
/// parks on a foreign waker (e.g. `flume::recv_async`) stays parked until some *other*
/// loop event wakes it — input, a redraw, or a GPUI timer. Observed symptom: tray
/// actions from the D-Bus thread only reached the UI while the download thread was
/// still emitting (each event woke the loop), then stalled once the app went idle.
///
/// The fix that works with gpui-pre 0.3.12 is to drive the pump with GPUI's own timer,
/// which uses `PlatformDispatcher::dispatch_after` and therefore does wake the loop.
/// The tick does no work when the channel is empty, so idle CPU stays low; a manual
/// refresh is issued only when at least one event was applied.
const PUMP_TICK: std::time::Duration = std::time::Duration::from_millis(50);

fn spawn_event_pump(view: Entity<AppState>, rx: Receiver<UiEvent>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(PUMP_TICK).await;
            // Drain everything queued since the last tick. Empty ticks cost one
            // channel try_recv and nothing else, so an idle app stays idle.
            while let Ok(event) = rx.try_recv() {
                cx.update_entity(&view, |state, ctx| state.apply_event(event, ctx));
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

    gpui_platform::application()
        .with_assets(gpui_kit_assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);

            Theme::change(ThemeMode::Dark, None, cx);
            // Dark teal palette in the spirit of DBDelve (dense, GPU-accelerated dashboard feel).
            cx.update_global::<Theme, _>(|theme, _| {
                theme.background = rgb(0x0e2a2a).into();
                theme.foreground = rgb(0xe6f4f1).into();
                theme.border = rgb(0x1f4a4a).into();
                theme.muted = rgb(0x143535).into();
                theme.muted_foreground = rgb(0x8fb3ad).into();
                theme.accent = rgb(0x163b3b).into();
                theme.accent_foreground = rgb(0xe6f4f1).into();
                theme.secondary = rgb(0x163b3b).into();
                theme.secondary_foreground = rgb(0xe6f4f1).into();
                theme.button_primary = rgb(0x1fa37a).into();
                theme.button_primary_hover = rgb(0x27b789).into();
                theme.button_primary_active = rgb(0x198b68).into();
                theme.button_primary_foreground = rgb(0x062019).into();
                theme.progress_bar = rgb(0x1fa37a).into();
                theme.window_radius = px(8.);
            });
            Theme::sync_base(cx);

            let bounds = Bounds::centered(None, size(px(980.), px(680.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui_component::TitleBar::title_bar_options()),
                kind: WindowKind::Normal,
                window_min_size: Some(size(px(760.), px(560.))),
                ..Default::default()
            };

            let tray_note_for_view = tray_note.clone();
            let window_handle = cx
                .open_window(options, move |_window, cx| {
                    cx.new(|_| AppState {
                        instances: make_instances(),
                        download_progress: 0.0,
                        tray_note: tray_note_for_view,
                        tray_available,
                        started_at,
                        first_paint_logged: false,
                        last_copied: None,
                    })
                })
                .expect("failed to open window");

            // F-TRY-02: closing the window keeps the app alive in the tray. Minimise
            // instead of destroying the window so tray actions still reach the live
            // view and the process stays resident. "Quit" in the tray is the only exit.
            if tray_available {
                let _ = window_handle.update(cx, |_, window, app| {
                    window.on_window_should_close(app, move |window, _app| {
                        window.minimize_window();
                        false
                    });
                });
            } else {
                // F-TRY-01 AC3: no tray available -> quit when the last window closes.
                cx.on_window_closed(|cx, _id| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
            }

            let view_entity = window_handle
                .update(cx, |_, _, cx| cx.entity())
                .expect("window closed before pump started");
            spawn_event_pump(view_entity, event_rx, cx);

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
    fn status_labels_match_fpr_contract() {
        assert_eq!(InstanceStatus::Running.label(), "running");
        assert_eq!(InstanceStatus::Stopped.label(), "stopped");
        assert_eq!(InstanceStatus::Failed.label(), "failed");
    }

    #[test]
    fn status_dot_colors_are_distinct() {
        let running = InstanceStatus::Running.dot_color();
        let stopped = InstanceStatus::Stopped.dot_color();
        let failed = InstanceStatus::Failed.dot_color();
        assert_ne!(running, stopped);
        assert_ne!(stopped, failed);
        assert_ne!(running, failed);
    }

    fn inst(engine: &'static str, name: &'static str, port: u16) -> Instance {
        Instance {
            name: name.into(),
            engine: engine.into(),
            version: "1.0.0".into(),
            port,
            status: InstanceStatus::Stopped,
        }
    }

    #[test]
    fn uri_uses_engine_scheme_and_loopback_only() {
        let pg = inst("postgres", "app-db", 5432).uri();
        assert_eq!(
            pg,
            "postgres://dblocal:secret@127.0.0.1:5432/app_db?sslmode=disable"
        );
        assert!(pg.contains("127.0.0.1"));
        assert!(!pg.contains("0.0.0.0"));

        let my = inst("mysql", "shop", 3306).uri();
        assert_eq!(my, "mysql://dblocal:secret@127.0.0.1:3306/shop");

        let maria = inst("mariadb", "legacy", 3307).uri();
        assert!(maria.starts_with("mysql://"));

        let valkey = inst("valkey", "cache", 6379).uri();
        assert_eq!(valkey, "redis://:secret@127.0.0.1:6379/0");
        assert!(!valkey.contains("0.0.0.0"));
    }

    #[test]
    fn instance_names_are_sanitised_for_uris() {
        let uri = inst("postgres", "my-project-1", 5432).uri();
        assert!(uri.contains("/my_project_1?"));
    }

    #[test]
    fn toggle_cycles_running_stopped_and_revives_failed() {
        let mut state = AppState {
            instances: vec![inst("postgres", "a", 5432)],
            download_progress: 0.0,
            tray_note: "".into(),
            tray_available: false,
            started_at: Instant::now(),
            first_paint_logged: false,
            last_copied: None,
        };
        assert_eq!(state.instances[0].status, InstanceStatus::Stopped);
        state.toggle(0);
        assert_eq!(state.instances[0].status, InstanceStatus::Running);
        state.toggle(0);
        assert_eq!(state.instances[0].status, InstanceStatus::Stopped);
        state.instances[0].status = InstanceStatus::Failed;
        state.toggle(0);
        assert_eq!(state.instances[0].status, InstanceStatus::Running);
    }

    #[test]
    fn toggle_out_of_range_is_a_no_op() {
        let mut state = AppState {
            instances: vec![inst("postgres", "a", 5432)],
            download_progress: 0.0,
            tray_note: "".into(),
            tray_available: false,
            started_at: Instant::now(),
            first_paint_logged: false,
            last_copied: None,
        };
        state.toggle(99);
        assert_eq!(state.instances.len(), 1);
        assert_eq!(state.instances[0].status, InstanceStatus::Stopped);
    }

    #[test]
    fn seed_instances_cover_all_three_states_and_loopback_ports() {
        let instances = make_instances();
        assert_eq!(instances.len(), 3);
        assert_eq!(instances[0].status, InstanceStatus::Running);
        assert_eq!(instances[1].status, InstanceStatus::Stopped);
        assert_eq!(instances[2].status, InstanceStatus::Failed);
        for instance in &instances {
            assert!(instance.uri().contains("127.0.0.1"));
        }
        assert_eq!(instances[0].port, 5432);
    }
}
