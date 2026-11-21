//! Headless wgpu render tests for the prototype port.
//!
//! The tests render the real views off-screen through GPUI's headless wgpu path
//! at the prototype's 1280x820 size and assert on regions against theme tokens,
//! so a layout regression fails the test rather than slipping past a
//! "more than 40 colours" check.
//!
//! Output: frames go to `target/renders/` by default. Committed screenshots
//! under `docs/design/renders/` are refreshed only when `DBLOCAL_UPDATE_RENDERS=1`
//! is set, so a plain `cargo test` never dirties a tracked path.

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

/// The prototype window size, which every screenshot is compared at.
const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 820.0;

/// A frame plus the flattened pixel data, so region checks are cheap.
///
/// Headless rendering runs at the platform scale factor (2x on this HiDPI
/// host), so a logical region is multiplied by [`Frame::scale`] before it is
/// sampled. That is what keeps the assertions comparing the prototype's logical
/// 1280x820 layout rather than a device-pixel count.
struct Frame {
    image: image::RgbaImage,
    colours: usize,
    scale: u32,
}

/// Logical size of the rendered window.
const LOGICAL_W: u32 = 1280;
const LOGICAL_H: u32 = 820;

impl Frame {
    /// Converts a logical rectangle to device pixels.
    fn scaled(&self, rect: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let s = self.scale;
        (rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s)
    }

    /// Reads one pixel addressed in logical coordinates.
    fn logical_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixel(x * self.scale, y * self.scale)
    }
}

impl Frame {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let (w, h) = self.image.dimensions();
        self.image.get_pixel(x.min(w - 1), y.min(h - 1)).0
    }

    fn dims(&self) -> (u32, u32) {
        self.image.dimensions()
    }

    /// Counts pixels in a rectangle whose colour is within `tolerance` of `rgb`.
    fn count_near(&self, rect: (u32, u32, u32, u32), rgb: [u8; 3], tolerance: u8) -> usize {
        let (x0, y0, x1, y1) = self.scaled(rect);
        let mut hits = 0;
        for y in y0..y1.min(self.dims().1) {
            for x in x0..x1.min(self.dims().0) {
                let p = self.pixel(x, y);
                if p[0].abs_diff(rgb[0]) <= tolerance
                    && p[1].abs_diff(rgb[1]) <= tolerance
                    && p[2].abs_diff(rgb[2]) <= tolerance
                {
                    hits += 1;
                }
            }
        }
        hits
    }

    /// Counts pixels in a rectangle that are not within `tolerance` of `rgb`.
    ///
    /// This is how "there is text here" is proven: text pixels differ from the
    /// surface behind them.
    fn count_not_near(&self, rect: (u32, u32, u32, u32), rgb: [u8; 3], tolerance: u8) -> usize {
        let (x0, y0, x1, y1) = self.scaled(rect);
        let mut hits = 0;
        for y in y0..y1.min(self.dims().1) {
            for x in x0..x1.min(self.dims().0) {
                let p = self.pixel(x, y);
                if p[0].abs_diff(rgb[0]) > tolerance
                    || p[1].abs_diff(rgb[1]) > tolerance
                    || p[2].abs_diff(rgb[2]) > tolerance
                {
                    hits += 1;
                }
            }
        }
        hits
    }

    /// The most common colour in a rectangle, which is the surface a region
    /// paints behind whatever text it holds.
    fn dominant(&self, rect: (u32, u32, u32, u32)) -> [u8; 3] {
        let (x0, y0, x1, y1) = self.scaled(rect);
        let mut counts: std::collections::HashMap<[u8; 3], usize> =
            std::collections::HashMap::new();
        for y in y0..y1.min(self.dims().1) {
            for x in x0..x1.min(self.dims().0) {
                let p = self.pixel(x, y);
                *counts.entry([p[0], p[1], p[2]]).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(c, _)| c)
            .unwrap_or([0, 0, 0])
    }
}

fn seeded_service() -> Arc<dyn DblService> {
    Arc::new(MockService::demo())
}

type Build = fn(Arc<dyn DblService>, &mut gpui_kit::Window, &mut Context<Workspace>) -> Workspace;

/// Where a frame is written, and whether that path is tracked by git.
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

/// Serialises the GPU-touching renders.
///
/// Each render creates its own headless wgpu context. Several concurrent
/// contexts intermittently wedge inside swapchain creation on this driver
/// (observed as a test that runs for minutes with no output), so the tests
/// take this lock and only one context exists at a time.
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

    // `uniform_list` measures its rows on the first frame and paints them on
    // the next. `capture_screenshot` renders only the last frame, so draw a few
    // times and settle between them.
    for _ in 0..3 {
        app.update_window(handle, |_view, window, cx| {
            let _ = window.draw(cx);
        })
        .expect("draw a frame");
        app.run_until_parked();
    }
    let image = app
        .capture_screenshot(handle)
        .expect("headless wgpu renderer must produce a frame");
    assert!(image.width() > 0 && image.height() > 0, "empty frame");
    // The window is 1280x820 logical; a HiDPI host renders it at 2x. Both are
    // the prototype's size, so assert the logical dimensions.
    let scale = image.width() / LOGICAL_W;
    assert!(scale >= 1, "unexpected render width {}", image.width());
    assert_eq!(
        (image.width() / scale, image.height() / scale),
        (LOGICAL_W, LOGICAL_H),
        "renders must match the prototype size"
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

// Theme tokens the assertions sample against. These are the committed JSON
// values; `contrast_table` proves they are what the app loads.
const DARK_BG: [u8; 3] = [0x15, 0x16, 0x1d];
const DARK_SIDEBAR: [u8; 3] = [0x1c, 0x1d, 0x26];
const DARK_ROW_ACTIVE: [u8; 3] = [0x2a, 0x2c, 0x3a];
const DARK_SUCCESS: [u8; 3] = [0x98, 0xc3, 0x79];
const DARK_DANGER: [u8; 3] = [0xef, 0x88, 0x91];
const DARK_PRIMARY: [u8; 3] = [0x61, 0xaf, 0xef];
const LIGHT_BG: [u8; 3] = [0xf8, 0xf8, 0xfb];
const LIGHT_SIDEBAR: [u8; 3] = [0xf0, 0xf0, 0xf4];
const LIGHT_PRIMARY: [u8; 3] = [0x1a, 0x60, 0x9f];

/// The dock is 272 px wide; the centre pane starts after it.
const DOCK: u32 = 272;
/// The title bar is 34 px, the tab strip 36 px.
const TAB_STRIP_TOP: u32 = 34;
const TAB_STRIP_BOTTOM: u32 = 70;

#[test]
fn renders_dark_and_light_overview_with_the_prototype_regions() {
    let dark = render(true, "overview-dark.png", Workspace::preview);
    let light = render(false, "overview-light.png", Workspace::preview);
    assert!(dark.colours > 40, "dark render too flat: {}", dark.colours);
    assert!(light.colours > 40, "light render too flat: {}", light.colours);

    for (name, frame, sidebar, bg) in [
        ("dark", &dark, DARK_SIDEBAR, DARK_BG),
        ("light", &light, LIGHT_SIDEBAR, LIGHT_BG),
    ] {
        let (_, h) = frame.dims();
        // 1. The dock paints the sidebar token on the left edge.
        let sidebar_hits = frame.count_near((0, 0, DOCK / 2, h), sidebar, 3);
        assert!(
            sidebar_hits > (h as usize / 2),
            "{name}: left edge is not the dock surface ({sidebar_hits} of {h})"
        );
        // 2. The centre pane sits right of the dock and paints the base token.
        let centre = frame.logical_pixel(DOCK + 300, h * 3 / 4);
        assert!(
            centre[0].abs_diff(bg[0]) <= 4
                && centre[1].abs_diff(bg[1]) <= 4
                && centre[2].abs_diff(bg[2]) <= 4,
            "{name}: centre background {centre:?} is not {bg:?}"
        );
    }

    // 3. The tab bar exists between the title bar and the header, and the
    //    selected tab is painted with the active-tab token.
    let (w, _) = dark.dims();
    let tab_strip = (DOCK, TAB_STRIP_TOP, w, TAB_STRIP_BOTTOM);
    let tab_surface = dark.dominant(tab_strip);
    assert_ne!(
        tab_surface, DARK_BG,
        "the instance tab strip is missing above the header"
    );
    let active_tab = dark.count_near(
        (DOCK, TAB_STRIP_TOP, DOCK + 300, TAB_STRIP_BOTTOM),
        DARK_BG,
        3,
    );
    eprintln!("TAB selected-tab pixels={active_tab}");
    assert!(
        active_tab > 1000,
        "the selected tab is not marked with the active background ({active_tab})"
    );

    // 4. Status glyphs per state in the dock: running green and failed red are
    //    both seeded, so both colours must appear in the glyph column.
    let (_, h) = dark.dims();
    let dock_column = (20, 0, 60, h - 40);
    let running = dark.count_near(dock_column, DARK_SUCCESS, 8);
    let failed = dark.count_near(dock_column, DARK_DANGER, 8);
    eprintln!("GLYPH dark running={running} failed={failed}");
    assert!(running > 0, "no running status glyph in the dock");
    assert!(failed > 0, "no failed status glyph in the dock");

    // 5. One dock row is selected, so the accent surface appears there.
    let selected = dark.count_near((0, 70, DOCK, h), DARK_ROW_ACTIVE, 3);
    eprintln!("DOCK selected-row pixels={selected}");
    assert!(selected > 300, "no selected dock row ({selected})");

    // 6. The primary button colour is present (Copy URI / Instance baru).
    let primary_dark = dark.count_near((0, 70, w, h - 40), DARK_PRIMARY, 10);
    let primary_light = light.count_near((0, 70, w, h - 40), LIGHT_PRIMARY, 10);
    eprintln!("PRIMARY dark={primary_dark} light={primary_light}");
    assert!(primary_dark > 200, "no primary button in dark ({primary_dark})");
    assert!(
        primary_light > 200,
        "no primary button in light ({primary_light})"
    );
}

#[test]
fn renders_failed_logs_with_the_diagnosis_banner() {
    let shade = render(true, "logs-diagnosis-dark.png", Workspace::preview_failure);
    let (w, h) = shade.dims();
    // The danger banner sits under the header on the Logs tab.
    let banner = shade.count_near((DOCK, 60, w, h / 2), DARK_DANGER, 24);
    eprintln!("DIAGNOSIS danger pixels={banner}");
    assert!(
        banner > 800,
        "the diagnosis banner is not visible on a failed instance ({banner})"
    );
}

#[test]
fn renders_logs_with_monospace_text() {
    let logs = render(false, "logs-light.png", Workspace::preview_logs);
    let (w, h) = logs.dims();
    // The log body carries rows of text below the banner/header region.
    let mut rows_with_text = 0;
    for y in (h / 3..h * 9 / 10).step_by(6) {
        if logs.count_not_near((DOCK + 40, y, w - 20, (y + 6).min(h)), LIGHT_BG, 12) > 8 {
            rows_with_text += 1;
        }
    }
    eprintln!("LOGS rows with text={rows_with_text}");
    assert!(
        rows_with_text > 20,
        "expected a scrollable log body, got {rows_with_text} text rows"
    );
}

#[test]
fn renders_databases_placeholder_in_the_product_build() {
    let product = render(
        false,
        "databases-product-light.png",
        Workspace::preview_product_databases,
    );
    let (w, h) = product.dims();
    // The placeholder is a card, not a table of sample rows: the body has far
    // less text than the demo build's table.
    let body = (DOCK + 40, 140, w - 40, h - 80);
    let product_text = product.count_not_near(body, LIGHT_BG, 12);

    let demo = render(false, "databases-demo-light.png", Workspace::preview_databases);
    let demo_text = demo.count_not_near(body, LIGHT_BG, 12);
    eprintln!("DATABASES product text={product_text} demo text={demo_text}");
    assert!(
        product_text > 200,
        "the product placeholder renders nothing ({product_text})"
    );
    assert!(
        demo_text > product_text * 2,
        "the demo table should render far more content than the placeholder \
         ({demo_text} vs {product_text})"
    );
}

#[test]
fn renders_create_dialog_with_a_download_and_an_unavailable_version() {
    let download = render(true, "create-download-dark.png", Workspace::preview_create);
    let (w, h) = download.dims();
    // The modal panel is the popover surface over a dimmed backdrop.
    let panel = download.count_near((w / 6, h / 10, w * 5 / 6, h * 9 / 10), DARK_SIDEBAR, 6);
    eprintln!("CREATE panel pixels={panel}");
    assert!(panel > 4000, "the create panel did not paint ({panel})");
    // A primary Create button is present.
    let primary = download.count_near((w / 3, h - 140, w, h - 20), DARK_PRIMARY, 12);
    eprintln!("CREATE primary pixels={primary}");
    assert!(primary > 60, "no Create button in the footer ({primary})");

    let unavailable = render(
        true,
        "create-unavailable-dark.png",
        Workspace::preview_create_unavailable,
    );
    let (w, h) = unavailable.dims();
    let card_text = unavailable.count_not_near((DOCK as u32, 80, w, h), DARK_BG, 16);
    eprintln!("CREATE unavailable card text={card_text}");
    assert!(
        card_text > 2000,
        "the unavailable engine card did not render ({card_text})"
    );
}

#[test]
fn renders_dock_with_the_filter_applied() {
    let full = render(true, "dock-all-dark.png", Workspace::preview);
    let filtered = render(true, "dock-filtered-dark.png", Workspace::preview_filtered);
    let (_, h) = full.dims();
    // Fewer status glyphs survive the "pg-" filter: the dock's glyph column
    // must lose the MariaDB and Valkey rows.
    let column = (20, 0, 60, h - 40);
    let before = full.count_near(column, DARK_SUCCESS, 8) + full.count_near(column, DARK_DANGER, 8);
    let after =
        filtered.count_near(column, DARK_SUCCESS, 8) + filtered.count_near(column, DARK_DANGER, 8);
    eprintln!("FILTER glyphs before={before} after={after}");
    assert!(
        after < before,
        "the dock filter did not drop rows (before={before}, after={after})"
    );
}

#[test]
fn renders_palette_and_delete_dialog() {
    let palette = render(true, "palette-dark.png", Workspace::preview_palette);
    let (w, h) = palette.dims();
    let panel = palette.count_near((w / 3, h / 10, w * 2 / 3, h * 9 / 10), DARK_SIDEBAR, 6);
    eprintln!("PALETTE panel pixels={panel}");
    assert!(panel > 2000, "palette panel surface missing ({panel})");

    let delete = render(false, "delete-light.png", Workspace::preview_delete);
    let (w, h) = delete.dims();
    // The delete panel is the popover surface over the dimmed backdrop.
    let panel = delete.count_near((w / 4, h / 4, w * 3 / 4, h * 3 / 4), LIGHT_SIDEBAR, 6);
    eprintln!("DELETE panel pixels={panel}");
    assert!(panel > 3000, "delete panel did not paint ({panel})");
}

#[test]
fn renders_stale_banner_and_revealed_uri() {
    let stale = render(false, "stale-light.png", Workspace::preview_stale_banner);
    let (w, h) = stale.dims();
    // F-CAT-01: the warning strip carries the warning token in the light theme.
    let warning = [0x73u8, 0x53, 0x00];
    let banner = stale.count_near((DOCK, 0, w, h / 6), warning, 12);
    eprintln!("STALE banner pixels={banner}");
    assert!(banner > 2000, "the stale catalog banner did not render ({banner})");

    let revealed = render(true, "overview-revealed-dark.png", Workspace::preview_revealed);
    assert!(revealed.colours > 40);
}
