//! Headless wgpu render tests for the prototype port (D-25).
//!
//! Renders the real views off-screen at the prototype's 1280x820 logical size
//! and asserts on regions derived from the committed theme tokens, so a layout
//! regression fails the test instead of slipping past a colour-count check.
//!
//! Frames go to `target/renders/` by default. Tracked screenshots under
//! `docs/design/renders/` are refreshed only with `DBLOCAL_UPDATE_RENDERS=1`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::collections::HashSet;
use std::sync::Arc;

use dblocal_core::service::{DblService, mock::MockService};
use dblocal_gui::app::Workspace;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, AssetSource, Bounds, Context, HeadlessAppContext,
    SharedString, WindowBounds, WindowOptions, px, size,
};

struct GuiAssets;
impl AssetSource for GuiAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        gpui_kit::assets::AllAssets.load(path)
    }
    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 820.0;
const LOGICAL_W: u32 = 1280;
const LOGICAL_H: u32 = 820;

/// Dock width, title bar, tab strip — prototype layout constants.
const DOCK: u32 = 272;
const TAB_TOP: u32 = 34;
const TAB_BOTTOM: u32 = 70;

/// Theme tokens as opaque RGB (committed JSON; contrast_table proves them).
const DARK_BG: [u8; 3] = [0x15, 0x16, 0x1d];
const DARK_SIDEBAR: [u8; 3] = [0x1c, 0x1d, 0x26];
const DARK_ACTIVE_ROW: [u8; 3] = [0x2a, 0x2c, 0x3a];
const DARK_SUCCESS: [u8; 3] = [0x98, 0xc3, 0x79];
const DARK_DANGER: [u8; 3] = [0xef, 0x88, 0x91];
const DARK_PRIMARY: [u8; 3] = [0x61, 0xaf, 0xef];

const LIGHT_BG: [u8; 3] = [0xf8, 0xf8, 0xfb];
const LIGHT_SIDEBAR: [u8; 3] = [0xf0, 0xf0, 0xf4];
const LIGHT_PRIMARY: [u8; 3] = [0x1a, 0x60, 0x9f];
const LIGHT_WARNING: [u8; 3] = [0x73, 0x53, 0x00];

struct Frame {
    image: image::RgbaImage,
    colours: usize,
    scale: u32,
}

impl Frame {
    fn dims(&self) -> (u32, u32) {
        self.image.dimensions()
    }
    fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let (w, h) = self.dims();
        self.image.get_pixel(x.min(w - 1), y.min(h - 1)).0
    }
    /// Scale a logical rect to device pixels, clamped to the frame.
    fn rect(&self, rect: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (w, h) = self.dims();
        let s = self.scale;
        (
            (rect.0 * s).min(w),
            (rect.1 * s).min(h),
            (rect.2 * s).min(w),
            (rect.3 * s).min(h),
        )
    }
    fn count_near(&self, rect: (u32, u32, u32, u32), rgb: [u8; 3], tol: u8) -> usize {
        let (x0, y0, x1, y1) = self.rect(rect);
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.px(x, y);
                if p[0].abs_diff(rgb[0]) <= tol
                    && p[1].abs_diff(rgb[1]) <= tol
                    && p[2].abs_diff(rgb[2]) <= tol
                {
                    hits += 1;
                }
            }
        }
        hits
    }
    fn count_not_near(&self, rect: (u32, u32, u32, u32), rgb: [u8; 3], tol: u8) -> usize {
        let (x0, y0, x1, y1) = self.rect(rect);
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.px(x, y);
                if p[0].abs_diff(rgb[0]) > tol
                    || p[1].abs_diff(rgb[1]) > tol
                    || p[2].abs_diff(rgb[2]) > tol
                {
                    hits += 1;
                }
            }
        }
        hits
    }
    /// Count pixels whose channel distance to `rgb` is within `tol` in the rect,
    /// but also require the pixel to be "tinted" (differs from `base`).
    fn count_tint(&self, rect: (u32, u32, u32, u32), base: [u8; 3], tint: [u8; 3]) -> usize {
        let (x0, y0, x1, y1) = self.rect(rect);
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.px(x, y);
                let near_base = p[0].abs_diff(base[0]) <= 6
                    && p[1].abs_diff(base[1]) <= 6
                    && p[2].abs_diff(base[2]) <= 6;
                let toward_tint = (p[0] as i16 - base[0] as i16).signum()
                    == (tint[0] as i16 - base[0] as i16).signum()
                    && (p[1] as i16 - base[1] as i16).signum()
                        == (tint[1] as i16 - base[1] as i16).signum();
                if !near_base && toward_tint {
                    hits += 1;
                }
            }
        }
        hits
    }
}

fn seeded_service() -> Arc<dyn DblService> {
    Arc::new(MockService::demo())
}

type Build = fn(Arc<dyn DblService>, &mut gpui_kit::Window, &mut Context<Workspace>) -> Workspace;

fn output_path(name: &str) -> (std::path::PathBuf, bool) {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("DBLOCAL_UPDATE_RENDERS").is_some() {
        (manifest.join("../../docs/design/renders").join(name), true)
    } else {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| manifest.join("../../target"));
        (target.join("renders").join(name), false)
    }
}

static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render(mode_dark: bool, name: &str, build: Build) -> Frame {
    let _guard = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut app = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(GuiAssets),
        gpui_kit::platform::current_headless_renderer,
    );
    app.update(gpui_kit::init);
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
            origin: gpui_kit::point(px(0.0), px(0.0)),
            size: size(px(WIDTH), px(HEIGHT)),
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
                        cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
                    },
                )
            })
            .expect("open headless window");
        app.run_until_parked();
        window.into()
    };

    // uniform_list paints rows on the next frame after layout.
    for _ in 0..4 {
        app.update_window(handle, |_view, window, cx| {
            let _ = window.draw(cx);
        })
        .expect("draw frame");
        app.run_until_parked();
    }
    let image = app
        .capture_screenshot(handle)
        .expect("headless wgpu renderer must produce a frame");
    assert!(image.width() > 0 && image.height() > 0, "empty frame");
    let scale = image.width() / LOGICAL_W;
    assert!(scale >= 1, "unexpected render width {}", image.width());
    assert_eq!(
        (image.width() / scale, image.height() / scale),
        (LOGICAL_W, LOGICAL_H),
        "render must match the prototype logical size"
    );
    let mut colours = HashSet::new();
    for pixel in image.pixels() {
        colours.insert(pixel.0);
    }
    let (path, tracked) = output_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create render dir");
    }
    image.save(&path).expect("write png");
    eprintln!(
        "RENDER {} ({}x{}, {} colours, {})",
        path.display(),
        image.width(),
        image.height(),
        colours.len(),
        if tracked { "tracked" } else { "target-only" }
    );
    Frame {
        image,
        colours: colours.len(),
        scale,
    }
}

fn text_rows(frame: &Frame, rect: (u32, u32, u32, u32), bg: [u8; 3], tol: u8) -> usize {
    let (x0, y0, x1, y1) = frame.rect(rect);
    let mut rows = 0;
    let mut y = y0;
    while y + 6 * frame.scale <= y1 {
        let mut row_hits = 0;
        for x in x0..x1 {
            let p = frame.px(x, y);
            if p[0].abs_diff(bg[0]) > tol
                || p[1].abs_diff(bg[1]) > tol
                || p[2].abs_diff(bg[2]) > tol
            {
                row_hits += 1;
            }
        }
        if row_hits > 8 {
            rows += 1;
        }
        y += 6 * frame.scale;
    }
    rows
}

#[test]
fn renders_dark_and_light_overview_with_the_prototype_regions() {
    let dark = render(true, "overview-dark.png", Workspace::preview);
    let light = render(false, "overview-light.png", Workspace::preview);
    assert!(dark.colours > 40, "dark render too flat: {}", dark.colours);
    assert!(
        light.colours > 40,
        "light render too flat: {}",
        light.colours
    );

    for (name, frame, sidebar, bg) in [
        ("dark", &dark, DARK_SIDEBAR, DARK_BG),
        ("light", &light, LIGHT_SIDEBAR, LIGHT_BG),
    ] {
        let sidebar_hits = frame.count_near((0, 0, DOCK / 2, LOGICAL_H), sidebar, 4);
        assert!(
            sidebar_hits > (LOGICAL_H as usize * frame.scale as usize / 2),
            "{name}: left edge is not dock surface ({sidebar_hits})"
        );
        // The centre pane paints the base background between cards.
        let centre_hits = frame.count_near(
            (DOCK + 10, TAB_BOTTOM + 10, LOGICAL_W - 10, LOGICAL_H - 40),
            bg,
            4,
        );
        assert!(
            centre_hits > 10_000,
            "{name}: centre base background not present ({centre_hits})"
        );
    }

    // Tab strip: selected tab paints the active tab background.
    let active_dark = dark.count_near((DOCK, TAB_TOP, DOCK + 280, TAB_BOTTOM), DARK_BG, 4);
    let active_light = light.count_near((DOCK, TAB_TOP, DOCK + 280, TAB_BOTTOM), LIGHT_BG, 4);
    eprintln!("TAB dark={active_dark} light={active_light}");
    assert!(
        active_dark > 500,
        "dark selected tab not marked ({active_dark})"
    );
    assert!(
        active_light > 500,
        "light selected tab not marked ({active_light})"
    );

    // Status glyphs per state in dock glyph column: running green, failed red.
    let running = dark.count_near((20, 0, 60, LOGICAL_H - 40), DARK_SUCCESS, 16);
    let failed = dark.count_near((20, 0, 60, LOGICAL_H - 40), DARK_DANGER, 16);
    eprintln!("GLYPH running={running} failed={failed}");
    assert!(running > 20, "no running glyph ({running})");
    assert!(failed > 20, "no failed glyph ({failed})");

    // A selected row in the dock.
    let selected = dark.count_near((0, 70, DOCK, LOGICAL_H - 80), DARK_ACTIVE_ROW, 4);
    eprintln!("DOCK selected={selected}");
    assert!(selected > 300, "no selected dock row ({selected})");

    // Primary button colour present in each theme.
    let primary_dark = dark.count_near((DOCK, 70, LOGICAL_W, LOGICAL_H - 40), DARK_PRIMARY, 16);
    let primary_light = light.count_near((DOCK, 70, LOGICAL_W, LOGICAL_H - 40), LIGHT_PRIMARY, 16);
    eprintln!("PRIMARY dark={primary_dark} light={primary_light}");
    assert!(
        primary_dark > 200,
        "no dark primary button ({primary_dark})"
    );
    assert!(
        primary_light > 200,
        "no light primary button ({primary_light})"
    );
}

#[test]
fn renders_failed_logs_with_the_diagnosis_banner() {
    let frame = render(true, "logs-diagnosis-dark.png", Workspace::preview_failure);
    // The diagnosis banner is a danger-tinted strip in the content area.
    let danger_tinted = frame.count_tint(
        (DOCK, 100, LOGICAL_W - 20, LOGICAL_H / 2),
        DARK_BG,
        DARK_DANGER,
    );
    let danger_text = frame.count_near((DOCK, 100, LOGICAL_W - 20, LOGICAL_H / 2), DARK_DANGER, 28);
    eprintln!("DIAGNOSIS tint={danger_tinted} danger_text={danger_text}");
    assert!(
        danger_tinted > 3000 || danger_text > 200,
        "diagnosis banner not visible (tint={danger_tinted}, text={danger_text})"
    );
}

#[test]
fn renders_logs_with_monospace_text() {
    let frame = render(false, "logs-light.png", Workspace::preview_logs);
    // Log body is below header/subtabs in the right pane.
    let rows = text_rows(
        &frame,
        (DOCK + 20, 170, LOGICAL_W - 30, LOGICAL_H - 90),
        LIGHT_BG,
        14,
    );
    eprintln!("LOGS rows={rows}");
    assert!(rows > 2, "expected log text rows, got {rows}");
}

#[test]
fn renders_databases_placeholder_in_the_product_build() {
    let product = render(
        false,
        "databases-product-light.png",
        Workspace::preview_product_databases,
    );
    let demo = render(
        false,
        "databases-demo-light.png",
        Workspace::preview_databases,
    );
    let body = (DOCK + 40, 180, LOGICAL_W - 40, LOGICAL_H - 80);
    let product_text = product.count_not_near(body, LIGHT_BG, 12);
    let demo_text = demo.count_not_near(body, LIGHT_BG, 12);
    eprintln!("DATABASES product={product_text} demo={demo_text}");
    assert!(
        product_text > 100,
        "product placeholder renders nothing ({product_text})"
    );
    assert!(
        demo_text > product_text.saturating_mul(2),
        "demo should render more content than placeholder ({demo_text} vs {product_text})"
    );
}

#[test]
fn renders_create_dialog_with_a_download_and_an_unavailable_version() {
    let download = render(true, "create-download-dark.png", Workspace::preview_create);
    // Modal panel paints popover/surface over a dimmed backdrop.
    let panel = download.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), [35, 36, 47], 10)
        + download.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), DARK_SIDEBAR, 10);
    let primary = download.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), DARK_PRIMARY, 18);
    eprintln!("CREATE panel={panel} primary={primary}");
    assert!(panel > 5000, "create panel not painted ({panel})");
    assert!(primary > 200, "create primary button missing ({primary})");

    let unavailable = render(
        true,
        "create-unavailable-dark.png",
        Workspace::preview_create_unavailable,
    );
    let text = unavailable.count_not_near((DOCK, 120, LOGICAL_W - 40, LOGICAL_H - 60), DARK_BG, 16);
    eprintln!("CREATE unavailable text={text}");
    assert!(text > 1500, "unavailable card did not render ({text})");
}

#[test]
fn renders_dock_with_the_filter_applied() {
    let full = render(true, "dock-all-dark.png", Workspace::preview);
    let filtered = render(true, "dock-filtered-dark.png", Workspace::preview_filtered);
    let col = (20, 70, 60, LOGICAL_H - 80);
    let before = full.count_near(col, DARK_SUCCESS, 16)
        + full.count_near(col, DARK_DANGER, 16)
        + full.count_near(col, [157, 165, 180], 12);
    let after = filtered.count_near(col, DARK_SUCCESS, 16)
        + filtered.count_near(col, DARK_DANGER, 16)
        + filtered.count_near(col, [157, 165, 180], 12);
    eprintln!("FILTER before={before} after={after}");
    assert!(
        after < before,
        "filter did not reduce dock rows ({before} -> {after})"
    );
}

#[test]
fn renders_palette_and_delete_dialog() {
    let palette = render(true, "palette-dark.png", Workspace::preview_palette);
    let (w, h) = (LOGICAL_W, LOGICAL_H);
    let panel = palette.count_near((w / 4, 80, w * 3 / 4, h - 120), [35, 36, 47], 10)
        + palette.count_near((w / 4, 80, w * 3 / 4, h - 120), DARK_SIDEBAR, 10);
    eprintln!("PALETTE panel={panel}");
    assert!(panel > 2000, "palette panel missing ({panel})");

    let delete = render(false, "delete-light.png", Workspace::preview_delete);
    let panel = delete.count_near((w / 4, h / 4, w * 3 / 4, h * 3 / 4), [255, 255, 255], 8)
        + delete.count_near((w / 4, h / 4, w * 3 / 4, h * 3 / 4), LIGHT_SIDEBAR, 8);
    eprintln!("DELETE panel={panel}");
    assert!(panel > 2000, "delete panel missing ({panel})");
}

#[test]
fn renders_stale_banner_and_revealed_uri() {
    let stale = render(false, "stale-light.png", Workspace::preview_stale_banner);
    let banner = stale.count_near((DOCK, 34, LOGICAL_W, 90), LIGHT_WARNING, 30)
        + stale.count_tint((DOCK, 34, LOGICAL_W, 90), LIGHT_BG, LIGHT_WARNING);
    eprintln!("STALE banner={banner}");
    assert!(banner > 100, "stale catalog banner missing ({banner})");

    let revealed = render(
        true,
        "overview-revealed-dark.png",
        Workspace::preview_revealed,
    );
    assert!(revealed.colours > 40, "revealed render too flat");
}
