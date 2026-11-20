//! Headless wgpu render tests for the shell.
//!
//! No screenshot tool is available on this host, so the tests render the real
//! view off-screen and assert on sampled pixels and distinct-colour counts, as
//! W0-C did. Run with `cargo test -p dblocal-gui --test render`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::collections::HashSet;
use std::sync::Arc;

use dblocal_core::model::{AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port};
use dblocal_core::service::{DblService, mock::MockService};
use dblocal_gui::app::Workspace;

// The preview constructors are gated behind `test-support`; dev builds enable it.
use gpui::{
    AnyWindowHandle, AppContext as _, AssetSource, Bounds, Context, HeadlessAppContext,
    SharedString, WindowBounds, WindowOptions, px, size,
};

struct GuiAssets;
impl AssetSource for GuiAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        gpui_kit_assets::AllAssets.load(path)
    }
    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        gpui_kit_assets::AllAssets.list(path)
    }
}

fn seeded_service() -> Arc<dyn DblService> {
    let service = MockService::deterministic();
    for (name, engine, version, port, status) in [
        (
            "pg-main",
            EngineKey::Postgresql,
            "17.4",
            5432,
            InstanceStatus::Running,
        ),
        (
            "pg-report",
            EngineKey::Postgresql,
            "16.6",
            5433,
            InstanceStatus::Stopped,
        ),
        (
            "mysql-shop",
            EngineKey::Mysql,
            "8.4.4",
            3306,
            InstanceStatus::Stopped,
        ),
        (
            "valkey-cache",
            EngineKey::Valkey,
            "8.1.0",
            6379,
            InstanceStatus::Running,
        ),
    ] {
        let spec = InstanceSpec::new(
            InstanceName::new(name).expect("valid name"),
            engine,
            version,
            Some(Port::new(port).expect("valid port")),
            AuthMode::Password,
        )
        .expect("valid spec");
        service.seed_instance(spec, status);
    }
    // One failed instance, so the failure panel can be rendered.
    let failed_spec = InstanceSpec::new(
        InstanceName::new("pg-broken").expect("valid name"),
        EngineKey::Postgresql,
        "17.4",
        Some(Port::new(5434).expect("valid port")),
        AuthMode::None,
    )
    .expect("valid spec");
    let failed = service.seed_instance(failed_spec, InstanceStatus::Failed);
    let mut failure_event = None;
    for _ in 0..1 {
        failure_event = Some(failed.clone());
    }
    let _ = failure_event;
    Arc::new(service)
}

type Build = fn(Arc<dyn DblService>, &mut gpui::Window, &mut Context<Workspace>) -> Workspace;

fn render(mode_dark: bool, name: &str, build: Build) -> (image::RgbaImage, usize) {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut app = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(GuiAssets),
        gpui_platform::current_headless_renderer,
    );
    app.update(gpui_component::init);
    app.update(dblocal_gui::theme::init);
    app.update(|cx| {
        let theme = if mode_dark {
            dblocal_gui::theme::DARK_THEME
        } else {
            dblocal_gui::theme::LIGHT_THEME
        };
        dblocal_gui::theme::apply_named(cx, theme);
    });

    let service = seeded_service();
    let handle: AnyWindowHandle = {
        let bounds = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: size(px(1180.0), px(760.0)),
        };
        let window = app
            .update(|cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        focus: false,
                        show: false,
                        ..Default::default()
                    },
                    |window, cx| {
                        let service = service.clone();
                        let view = cx.new(|cx| build(service, window, cx));
                        cx.new(|cx| gpui_component::Root::new(view, window, cx))
                    },
                )
            })
            .expect("open headless window");
        app.run_until_parked();
        window.into()
    };

    let image = app
        .capture_screenshot(handle)
        .expect("headless wgpu renderer must produce a frame");
    assert!(image.width() > 0 && image.height() > 0, "empty frame");
    let mut colours = HashSet::new();
    for pixel in image.pixels() {
        colours.insert(pixel.0);
    }
    let out_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/renders");
    std::fs::create_dir_all(&out_dir).expect("create render dir");
    let path = out_dir.join(name);
    image.save(&path).expect("write png");
    eprintln!(
        "RENDER {} ({}x{}, {} distinct colours)",
        path.display(),
        image.width(),
        image.height(),
        colours.len()
    );
    (image, colours.len())
}

fn centre_luminance(image: &image::RgbaImage) -> f64 {
    let (w, h) = image.dimensions();
    let (x0, y0, x1, y1) = (w * 3 / 10, h * 3 / 10, w * 7 / 10, h * 7 / 10);
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for y in (y0..y1).step_by(4) {
        for x in (x0..x1).step_by(4) {
            let p = image.get_pixel(x, y).0;
            sum += 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
            count += 1;
        }
    }
    sum / count.max(1) as f64
}

#[test]
fn renders_dark_and_light_shell() {
    let (dark, dark_n) = render(true, "shell-dark.png", Workspace::preview);
    let (light, light_n) = render(false, "shell-light.png", Workspace::preview);
    assert!(dark_n > 40, "dark render too flat: {dark_n} colours");
    assert!(light_n > 40, "light render too flat: {light_n} colours");

    let dark_l = centre_luminance(&dark);
    let light_l = centre_luminance(&light);
    eprintln!("CENTRE_LUMINANCE dark={dark_l:.2} light={light_l:.2}");
    assert!(
        (dark_l - light_l).abs() > 10.0,
        "themes render identically (dark={dark_l:.2}, light={light_l:.2})"
    );

    let (w, h) = dark.dimensions();
    let sample =
        |img: &image::RgbaImage, x: u32, y: u32| img.get_pixel(x.min(w - 1), y.min(h - 1)).0;
    for (label, image) in [("dark", &dark), ("light", &light)] {
        for (region, x, y) in [
            ("title", 40, 16),
            ("sidebar", 60, h / 3),
            ("centre", w / 2, h / 2),
            ("status", w / 2, h.saturating_sub(10)),
        ] {
            eprintln!("SAMPLE {label} {region} rgba={:?}", sample(image, x, y));
        }
    }
}

#[test]
fn renders_palette_logs_create_and_failure() {
    let (_, palette_n) = render(true, "shell-palette.png", Workspace::preview_palette);
    assert!(palette_n > 40, "palette render too flat: {palette_n}");

    let (_, logs_n) = render(false, "shell-logs.png", Workspace::preview_logs);
    assert!(logs_n > 40, "logs render too flat: {logs_n}");

    let (_, create_n) = render(true, "shell-create.png", Workspace::preview_create);
    assert!(create_n > 40, "create modal render too flat: {create_n}");

    let (failure, failure_n) = render(false, "shell-failure.png", Workspace::preview_failure);
    assert!(failure_n > 40, "failure render too flat: {failure_n}");

    // The failure panel paints `danger.background` (#b03730 in One Light) on
    // the detail pane. Scan for that surface rather than guessing a coordinate.
    let danger = [0xb0u8, 0x37, 0x30];
    let mut hits = 0usize;
    for pixel in failure.pixels() {
        let p = pixel.0;
        if p[0].abs_diff(danger[0]) < 10
            && p[1].abs_diff(danger[1]) < 10
            && p[2].abs_diff(danger[2]) < 10
        {
            hits += 1;
        }
    }
    eprintln!("FAILURE danger-surface pixels={hits}");
    assert!(
        hits > 500,
        "failure panel did not paint its danger surface (hits={hits})"
    );
}
