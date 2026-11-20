//! Renders the real `AppState` view through GPUI's headless wgpu renderer.
//!
//! Why this exists: this machine has no `grim`, `spectacle`, `gnome-screenshot`,
//! `scrot` or `maim`; XWayland root capture comes back black; and GNOME's
//! `org.gnome.Shell.Screenshot` D-Bus API is denied in this session (see
//! `docs/spikes/gpui/evidence/screenshot-attempt.txt`). Rendering the actual view
//! off-screen proves what the window paints without compositor permissions.
//!
//! Run: `spikes/gpui/scripts/capture-screenshot.sh`

use gpui::{
    AnyWindowHandle, App, AppContext as _, AssetSource, Bounds, Entity, HeadlessAppContext, Render,
    SharedString, Window, WindowBounds, WindowOptions, px, size,
};
use gpui_component::{Theme, ThemeMode};
use std::sync::Arc;

/// Asset source that serves gpui-component's bundled icons at runtime.
struct SpikeAssets;

impl AssetSource for SpikeAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        gpui_kit_assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        gpui_kit_assets::AllAssets.list(path)
    }
}

fn render_view(
    window: &mut Window,
    cx: &mut App,
) -> Entity<gpui_spike::app::AppState> {
    let _ = window;
    cx.new(|_| gpui_spike::app::AppState::preview())
}

#[test]
fn renders_the_real_view_to_png() {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut app = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(SpikeAssets),
        || gpui_platform::current_headless_renderer(),
    );

    // gpui-component needs its theme/global state installed before any view renders.
    app.update(gpui_component::init);
    app.update(|cx| Theme::change(ThemeMode::Dark, None, cx));

    let handle: AnyWindowHandle = {
        let bounds = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: size(px(980.0), px(680.0)),
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
                    render_view,
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

    let mut colours = std::collections::HashSet::new();
    for pixel in image.pixels() {
        colours.insert(pixel.0);
    }

    let out_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spikes/gpui/evidence");
    std::fs::create_dir_all(&out_dir).expect("create evidence dir");
    let path = out_dir.join("window-rendered.png");
    image.save(&path).expect("write png");

    eprintln!(
        "SCREENSHOT {} ({}x{}, {} distinct colours)",
        path.display(),
        image.width(),
        image.height(),
        colours.len()
    );

    assert!(
        colours.len() > 8,
        "expected a painted UI, got only {} distinct colours",
        colours.len()
    );
}
