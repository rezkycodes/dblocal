//! Measures the three OPEN-08 numbers on this host.
//!
//! Run in release mode with a live session:
//! `cargo run -p dblocal-gui --release --example measure`
//!
//! It prints, for each phase, `VmRSS`/`VmHWM` from `/proc/self/status` and the
//! process CPU time consumed per wall second:
//!
//! 1. `FLOOR`  — an empty GPUI window with no DBLocal view.
//! 2. `IDLE`   — the DBLocal shell on the mock service, no downloads running.
//! 3. `AFTER_DOWNLOAD` — the same shell once a mock create/download finished.
//!
//! If the empty window alone is above the 150 MB target, the floor is the
//! cost of GPUI + wgpu, and app code cannot go below it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dblocal_core::service::{DblService, mock::MockService};
use gpui_kit::{App, Bounds, WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, size};

const TARGET_RSS_MB: f64 = 150.0;
const TARGET_CPU_PCT: f64 = 1.0;

/// The floor view: an empty window with no DBLocal widgets.
struct EmptyView;

impl gpui_kit::Render for EmptyView {
    fn render(
        &mut self,
        _window: &mut gpui_kit::Window,
        _cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        div().size_full()
    }
}

#[derive(Debug, Clone, Copy)]
struct Rss {
    rss_kb: u64,
    hwm_kb: u64,
}

fn read_rss() -> Rss {
    let mut rss_kb = 0;
    let mut hwm_kb = 0;
    if let Ok(status) = fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                rss_kb = parse_kb(rest);
            } else if let Some(rest) = line.strip_prefix("VmHWM:") {
                hwm_kb = parse_kb(rest);
            }
        }
    }
    Rss { rss_kb, hwm_kb }
}

fn parse_kb(text: &str) -> u64 {
    text.split_whitespace()
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

fn cpu_seconds() -> f64 {
    // utime + stime from /proc/self/stat, in clock ticks (USER_HZ = 100).
    if let Ok(stat) = fs::read_to_string("/proc/self/stat") {
        let fields: Vec<&str> = stat.split_whitespace().collect();
        let utime = fields
            .get(13)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let stime = fields
            .get(14)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        return (utime + stime) / 100.0;
    }
    0.0
}

fn report(phase: &str, rss: Rss, cpu_pct: f64) {
    let rss_mb = rss.rss_kb as f64 / 1024.0;
    let hwm_mb = rss.hwm_kb as f64 / 1024.0;
    println!(
        "{phase}: VmRSS={rss_mb:.1} MB VmHWM={hwm_mb:.1} MB CPU={cpu_pct:.2}% (target <{TARGET_RSS_MB:.0} MB, <{TARGET_CPU_PCT:.0}%)"
    );
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    // Floor first: an empty window, measured before anything DBLocal builds.
    gpui_kit::platform::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_kit::component::init(cx);
            let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
                kind: WindowKind::Normal,
                ..Default::default()
            };
            let _ = cx.open_window(options, |window, cx| {
                let empty = cx.new(|_| EmptyView);
                cx.new(|cx| gpui_kit::component::Root::new(empty, window, cx))
            });

            let start_cpu = cpu_seconds();
            let start = Instant::now();
            let sample_after = Duration::from_secs(15);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(sample_after).await;
                let wall = start.elapsed().as_secs_f64();
                let cpu = cpu_seconds() - start_cpu;
                report("FLOOR", read_rss(), cpu / wall * 100.0);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });

    // Idle and post-download: the real shell on the mock service.
    let service: Arc<dyn DblService> = Arc::new(MockService::new());
    if let Err(report) = dblocal_gui::vulkan::preflight() {
        eprintln!("{report}");
        return;
    }
    gpui_kit::platform::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_kit::component::init(cx);
            dblocal_gui::theme::init(cx);
            let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
                kind: WindowKind::Normal,
                ..Default::default()
            };
            let service_for_window = service.clone();
            let runtime = dblocal_gui::app::bootstrap::AppRuntime::new(service_for_window);
            cx.set_global(runtime);
            let _ = cx.open_window(options, move |window, cx| {
                let view = cx.new(|cx| dblocal_gui::app::Workspace::new(window, cx));
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });

            let start_cpu = cpu_seconds();
            let start = Instant::now();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(20))
                    .await;
                let wall = start.elapsed().as_secs_f64();
                let cpu = cpu_seconds() - start_cpu;
                report("IDLE", read_rss(), cpu / wall * 100.0);

                // Kick a mock create (no runtime installed -> full download path).
                let service = service.clone();
                let spec = dblocal_core::model::InstanceSpec::new(
                    dblocal_core::model::InstanceName::new("measure-db").expect("name"),
                    dblocal_core::model::EngineKey::Postgresql,
                    "16.6",
                    Some(dblocal_core::model::Port::new(5600).expect("port")),
                    dblocal_core::model::AuthMode::Password,
                )
                .expect("spec");
                let _ = service.create_instance(spec);
                let download_start = Instant::now();
                cx.background_executor()
                    .timer(Duration::from_secs(12))
                    .await;
                let wall = download_start.elapsed().as_secs_f64();
                let _ = wall;
                let after_cpu = cpu_seconds();
                cx.background_executor().timer(Duration::from_secs(8)).await;
                let idle_wall = 8.0;
                let cpu = cpu_seconds() - after_cpu;
                report("AFTER_DOWNLOAD", read_rss(), cpu / idle_wall * 100.0);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
