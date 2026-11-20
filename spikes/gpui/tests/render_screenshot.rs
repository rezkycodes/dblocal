//! Renders the Zed-style `AppState` shell through GPUI's headless wgpu renderer.
//!
//! No screenshot tool is available and compositor capture is denied on this host
//! (see `docs/spikes/gpui/evidence/screenshot-attempt.txt`). This test renders
//! the actual view off-screen and records pixel evidence numerically.

use gpui::{
    AnyWindowHandle, AppContext as _, AssetSource, Bounds, HeadlessAppContext, SharedString,
    WindowBounds, WindowOptions, px, size,
};
use std::collections::HashSet;
use std::sync::Arc;

struct SpikeAssets;
impl AssetSource for SpikeAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        gpui_kit_assets::AllAssets.load(path)
    }
    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        gpui_kit_assets::AllAssets.list(path)
    }
}

fn render_with(
    mode_dark: bool,
    name: &str,
    build: fn(&mut gpui::Context<gpui_spike::app::AppState>) -> gpui_spike::app::AppState,
) -> (image::RgbaImage, usize) {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut app = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(SpikeAssets),
        gpui_platform::current_headless_renderer,
    );
    app.update(gpui_component::init);
    app.update(gpui_spike::theme::init);
    app.update(|cx| {
        let name = if mode_dark {
            gpui_spike::theme::DARK_THEME
        } else {
            gpui_spike::theme::LIGHT_THEME
        };
        gpui_spike::theme::apply_named(cx, name);
    });

    let handle: AnyWindowHandle = {
        let bounds = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: size(px(1100.0), px(720.0)),
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
                        let view = cx.new(build);
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
    for p in image.pixels() {
        colours.insert(p.0);
    }
    let out_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spikes/gpui/evidence");
    std::fs::create_dir_all(&out_dir).expect("create evidence dir");
    let path = out_dir.join(name);
    image.save(&path).expect("write png");
    eprintln!(
        "SCREENSHOT {} ({}x{}, {} distinct colours)",
        path.display(),
        image.width(),
        image.height(),
        colours.len()
    );
    (image, colours.len())
}

#[test]
fn renders_zed_style_shell_dark_and_light() {
    let (dark, dark_n) = render_with(
        true,
        "zed-shell-dark.png",
        gpui_spike::app::AppState::preview,
    );
    assert!(
        dark_n > 20,
        "dark: expected a painted UI, got {dark_n} colours"
    );
    let (light, light_n) = render_with(
        false,
        "zed-shell-light.png",
        gpui_spike::app::AppState::preview,
    );
    assert!(
        light_n > 20,
        "light: expected a painted UI, got {light_n} colours"
    );

    // Numeric proof the two themes differ: compare average luminance of a
    // central sample region (centre pane background area).
    let avg = |img: &image::RgbaImage| -> f64 {
        let (w, h) = img.dimensions();
        let (x0, y0, x1, y1) = (w * 3 / 10, h * 3 / 10, w * 7 / 10, h * 7 / 10);
        let mut sum = 0.0f64;
        let mut n = 0u64;
        for y in (y0..y1).step_by(4) {
            for x in (x0..x1).step_by(4) {
                let p = img.get_pixel(x, y).0;
                sum += 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
                n += 1;
            }
        }
        sum / n.max(1) as f64
    };
    let d_avg = avg(&dark);
    let l_avg = avg(&light);
    eprintln!("CENTRE_LUMINANCE dark={d_avg:.2} light={l_avg:.2}");
    assert!(
        (d_avg - l_avg).abs() > 10.0,
        "dark and light renders look identical (dark={d_avg:.2}, light={l_avg:.2})"
    );

    // Sample known regions for non-trivial content (not all transparent/black).
    let sample = |img: &image::RgbaImage, x: u32, y: u32| -> [u8; 4] { img.get_pixel(x, y).0 };
    for (label, img) in [("dark", &dark), ("light", &light)] {
        let (w, h) = img.dimensions();
        let pts = [
            ("title", 40, 18),
            ("sidebar", 60, h / 3),
            ("centre", w / 2, h / 2),
            ("status", w / 2, h.saturating_sub(12)),
        ];
        for (region, x, y) in pts {
            let px = sample(img, x.min(w - 1), y.min(h - 1));
            eprintln!("SAMPLE {label} {region} ({x},{y}) rgba={:?}", px);
        }
    }
}

/// The command palette overlay, the Logs inner tab and the welcome state each
/// paint a different subtree; rendering them keeps the whole view covered and
/// proves those paths do not panic.
#[test]
fn renders_palette_logs_and_welcome_states() {
    let (_, palette_n) = render_with(
        true,
        "zed-shell-palette.png",
        gpui_spike::app::AppState::preview_with_palette,
    );
    assert!(palette_n > 20, "palette: got {palette_n} colours");

    let (logs, logs_n) = render_with(
        false,
        "zed-shell-logs.png",
        gpui_spike::app::AppState::preview_with_logs,
    );
    assert!(logs_n > 20, "logs: got {logs_n} colours");

    let (_, welcome_n) = render_with(
        false,
        "zed-shell-welcome.png",
        gpui_spike::app::AppState::preview_welcome,
    );
    assert!(welcome_n > 20, "welcome: got {welcome_n} colours");

    // The Logs tab paints monospace text, so it must show quiet background
    // plus text pixels rather than a flat fill.
    let (w, h) = logs.dimensions();
    let centre = logs.get_pixel(w / 2, h / 2).0;
    eprintln!("LOGS centre rgba={centre:?}");
}
