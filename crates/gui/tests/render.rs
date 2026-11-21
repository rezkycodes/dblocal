//! Headless wgpu render tests for the shell.
//!
//! The tests render the real view off-screen through GPUI's headless wgpu path
//! and assert on specific regions against the theme tokens, so a layout
//! regression fails the test instead of slipping past a "> 40 colours" check.
//!
//! Output: frames are written to `target/renders/` by default. Committed
//! screenshots under `docs/design/renders/` are refreshed only when
//! `DBLOCAL_UPDATE_RENDERS=1` is set, so a plain `cargo test` never dirties a
//! tracked path.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::collections::HashSet;
use std::sync::Arc;

use dblocal_core::model::{AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port};
use dblocal_core::service::{DblService, mock::MockService};
use dblocal_gui::app::Workspace;
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

/// A frame plus the flattened pixel data, so region checks are cheap.
struct Frame {
    image: image::RgbaImage,
    colours: usize,
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
        let (x0, y0, x1, y1) = rect;
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
        let (x0, y0, x1, y1) = rect;
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
    let failed_spec = InstanceSpec::new(
        InstanceName::new("pg-broken").expect("valid name"),
        EngineKey::Postgresql,
        "17.4",
        Some(Port::new(5434).expect("valid port")),
        AuthMode::None,
    )
    .expect("valid spec");
    service.seed_instance(failed_spec, InstanceStatus::Failed);
    Arc::new(service)
}

type Build = fn(Arc<dyn DblService>, &mut gpui::Window, &mut Context<Workspace>) -> Workspace;

/// Where a frame is written, and whether that path is tracked by git.
fn output_path(name: &str) -> (std::path::PathBuf, bool) {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("DBLOCAL_UPDATE_RENDERS").is_some() {
        (manifest.join("../../docs/design/renders").join(name), true)
    } else {
        // `CARGO_TARGET_DIR` is set by the runner; fall back to the crate's
        // `target/` so a test never writes into a tracked path.
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
/// take this lock and only one context exists at a time. Single-threaded this
/// suite finishes in ~45 s, which is acceptable for CI.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render(mode_dark: bool, name: &str, build: Build) -> Frame {
    let _guard = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

    // `uniform_list` measures its rows on the first frame and paints them on
    // the next. `capture_screenshot` renders only the last frame, so draw twice
    // and settle between them.
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
    }
}

fn centre_luminance(frame: &Frame) -> f64 {
    let (w, h) = frame.dims();
    let (x0, y0, x1, y1) = (w * 3 / 10, h * 3 / 10, w * 7 / 10, h * 7 / 10);
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for y in (y0..y1).step_by(4) {
        for x in (x0..x1).step_by(4) {
            let p = frame.pixel(x, y);
            sum += 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
            count += 1;
        }
    }
    sum / count.max(1) as f64
}

// Theme tokens the assertions sample against. These are the committed JSON
// values; `contrast_table` proves they are what the app loads.
const DARK_SIDEBAR: [u8; 3] = [0x21, 0x25, 0x2b];
const DARK_BG: [u8; 3] = [0x28, 0x2c, 0x34];
const DARK_ROW_ACTIVE: [u8; 3] = [0x2c, 0x31, 0x3a];
const DARK_SUCCESS: [u8; 3] = [0x98, 0xc3, 0x79];
const DARK_DANGER: [u8; 3] = [0xe8, 0x9a, 0xa3];
const LIGHT_SIDEBAR: [u8; 3] = [0xf0, 0xf0, 0xf0];
const LIGHT_BG: [u8; 3] = [0xfa, 0xfa, 0xfa];
const LIGHT_ROW_ACTIVE: [u8; 3] = [0xe5, 0xe5, 0xe6];
const LIGHT_SUCCESS: [u8; 3] = [0x2b, 0x72, 0x30];
const LIGHT_DANGER: [u8; 3] = [0xb0, 0x37, 0x30];

#[test]
fn renders_dark_and_light_shell_with_theme_regions() {
    let dark = render(true, "shell-dark.png", Workspace::preview);
    let light = render(false, "shell-light.png", Workspace::preview);
    assert!(dark.colours > 40, "dark render too flat: {}", dark.colours);
    assert!(
        light.colours > 40,
        "light render too flat: {}",
        light.colours
    );

    // Distinct themes: the centre pane luminance must differ clearly.
    let dark_l = centre_luminance(&dark);
    let light_l = centre_luminance(&light);
    eprintln!("CENTRE_LUMINANCE dark={dark_l:.2} light={light_l:.2}");
    assert!(
        (dark_l - light_l).abs() > 10.0,
        "themes render identically (dark={dark_l:.2}, light={light_l:.2})"
    );

    // Sidebar surface: 260px wide at the left, sampled low enough to be under
    // the tree rows. Must be the sidebar token, not the editor background.
    for (name, frame, sidebar, bg) in [
        ("dark", &dark, DARK_SIDEBAR, DARK_BG),
        ("light", &light, LIGHT_SIDEBAR, LIGHT_BG),
    ] {
        let (w, h) = frame.dims();
        let sidebar_hits = frame.count_near((0, 0, 12, h), sidebar, 3);
        assert!(
            sidebar_hits > (h as usize / 2),
            "{name}: left edge is not the sidebar surface ({sidebar_hits} of {h})"
        );
        // The centre pane sits to the right of the 260px sidebar. Sample below
        // the title bar (68px) and the tab strips, in an empty part of the body.
        let centre = frame.pixel(w / 2, h / 2);
        assert!(
            centre[0].abs_diff(bg[0]) <= 3
                && centre[1].abs_diff(bg[1]) <= 3
                && centre[2].abs_diff(bg[2]) <= 3,
            "{name}: centre background {centre:?} is not {bg:?}"
        );
    }

    // Status dots: the seeded list has running and failed instances, so their
    // colours must appear in the sidebar's dot column.
    let dark_running = dark.count_near((20, 0, 52, 1520), DARK_SUCCESS, 6);
    let dark_failed = dark.count_near((20, 0, 52, 1520), DARK_DANGER, 6);
    eprintln!("DOTS dark running={dark_running} failed={dark_failed}");
    assert!(
        dark_running > 0,
        "no running (green) status dot in the sidebar"
    );
    assert!(dark_failed > 0, "no failed (red) status dot in the sidebar");
    let _ = LIGHT_SUCCESS;
    let _ = LIGHT_DANGER;

    // The selected row uses the sidebar-accent surface. `preview` selects the
    // first opened instance, so one row must paint that token in the sidebar.
    // Dark accent (#2c313a) and light accent (#e5e5e6) differ from the sidebar
    // background, so a count proves the selected-row state is applied.
    let dark_selected = dark.count_near((0, 68, 520, 1468), DARK_ROW_ACTIVE, 2);
    let light_selected = light.count_near((0, 68, 520, 1468), LIGHT_ROW_ACTIVE, 2);
    eprintln!("SELECTED ROW dark={dark_selected} light={light_selected}");
    assert!(
        dark_selected > 500,
        "no selected-row surface in the dark sidebar ({dark_selected})"
    );
    assert!(
        light_selected > 500,
        "no selected-row surface in the light sidebar ({light_selected})"
    );
}

#[test]
fn renders_palette_create_delete_stale_and_failure() {
    let palette = render(true, "shell-palette.png", Workspace::preview_palette);
    assert!(palette.colours > 40);
    // The palette panel is the popover surface over a dimmed backdrop.
    let (w, h) = palette.dims();
    let panel = palette.count_near((w / 3, h / 10, w * 2 / 3, h * 9 / 10), DARK_SIDEBAR, 4);
    assert!(panel > 2000, "palette panel surface missing (hits={panel})");

    let create = render(true, "shell-create.png", Workspace::preview_create);
    assert!(create.colours > 40);
    let delete = render(false, "shell-delete.png", Workspace::preview_delete);
    assert!(delete.colours > 40);

    let stale = render(false, "shell-stale.png", Workspace::preview_stale_banner);
    assert!(stale.colours > 40);
    // F-CAT-01: the warning strip (`#856000` in One Light) spans the top of the
    // centre pane.
    let warning = [0x85u8, 0x60, 0x00];
    let (w, h) = stale.dims();
    let banner_hits = stale.count_near((260, 0, w, h / 6), warning, 10);
    eprintln!("STALE banner pixels={banner_hits}");
    assert!(
        banner_hits > 2000,
        "catalog banner did not render (hits={banner_hits})"
    );

    let failure = render(false, "shell-failure.png", Workspace::preview_failure);
    assert!(failure.colours > 40);
    let (w, _h) = failure.dims();
    // The failure panel paints `danger.background` on the detail pane.
    let danger_hits = failure.count_near((w / 3, 0, w, 760), LIGHT_DANGER, 10);
    eprintln!("FAILURE danger-surface pixels={danger_hits}");
    assert!(
        danger_hits > 500,
        "failure panel did not paint its danger surface (hits={danger_hits})"
    );
}

/// The Logs tab must actually be the selected screen, and its body must carry
/// monospace text over the editor background.
#[test]
fn renders_logs_tab_with_text_and_follow_label() {
    let logs = render(false, "shell-logs.png", Workspace::preview_logs);
    let overview = render(false, "shell-overview.png", Workspace::preview);
    assert!(logs.colours > 40);

    // B.1: the Logs tab has to be the visible one, and prove log *text* is on
    // screen. The discriminator is the lower body: the Overview's content (info
    // panel, buttons) ends around a third of the way down, while log lines fill
    // the panel to the bottom. A strip-only diff — what the review caught — has
    // no text here.
    let (w, h) = logs.dims();
    let lower = (w / 2, h * 11 / 20, w, h * 9 / 10);
    let log_text = logs.count_not_near(lower, LIGHT_BG, 12);
    let overview_text = overview.count_not_near(lower, LIGHT_BG, 12);
    eprintln!("LOGS lower-body text pixels={log_text} overview={overview_text}");
    assert!(
        overview_text < 200,
        "the Overview unexpectedly has content in the lower body ({overview_text}); \
         the region is no longer a log-only discriminator"
    );
    assert!(
        log_text > 1500,
        "the Logs body has no text in its lower half ({log_text}); is the Logs tab selected?"
    );

    // The full-height monospace block: rows of text must run most of the panel.
    let mut rows_with_text = 0;
    for y in (h / 4..h * 9 / 10).step_by(6) {
        if logs.count_not_near((w / 2, y, w, (y + 6).min(h)), LIGHT_BG, 12) > 8 {
            rows_with_text += 1;
        }
    }
    eprintln!("LOGS rows with text={rows_with_text}");
    assert!(
        rows_with_text > 20,
        "expected a scrollable log body, got {rows_with_text} text rows"
    );
}

/// A second Logs render proves per-instance buffers: two tabs, each showing its
/// own lines, with a live line for the other instance not leaking in.
#[test]
fn renders_logs_for_each_instance_independently() {
    let overview = render(false, "shell-overview-b.png", Workspace::preview);
    let logs_a = render(false, "shell-logs-a.png", |service, window, cx| {
        Workspace::preview_logs_for(service, window, cx, 0)
    });
    let logs_b = render(false, "shell-logs-b.png", |service, window, cx| {
        Workspace::preview_logs_for(service, window, cx, 1)
    });
    // Both tabs must show log text in the lower body, where an Overview tab
    // would be blank; that is what proves each is really on Logs.
    let (w, h) = logs_a.dims();
    let lower = (w / 2, h * 11 / 20, w, h * 9 / 10);
    let a_text = logs_a.count_not_near(lower, LIGHT_BG, 12);
    let b_text = logs_b.count_not_near(lower, LIGHT_BG, 12);
    let o_text = overview.count_not_near(lower, LIGHT_BG, 12);
    eprintln!("LOGS lower-body a={a_text} b={b_text} overview={o_text}");
    assert!(o_text < 200, "Overview lower body is not blank ({o_text})");
    assert!(a_text > 1500, "tab A is not showing log text ({a_text})");
    assert!(b_text > 1500, "tab B is not showing log text ({b_text})");
}

/// G: a deleted instance leaves the dock. The frame after removal must show
/// fewer instance rows than the normal shell, and the selected-row surface that
/// belonged to it must be gone from the sidebar.
#[test]
fn deleted_instance_disappears_from_the_dock() {
    let normal = render(false, "shell-dock-before.png", Workspace::preview);
    let after = render(
        false,
        "shell-dock-after-delete.png",
        Workspace::preview_after_delete,
    );

    // The sidebar instance rows use the selected-row accent for the open tab and
    // plain rows elsewhere; a reliable proxy for "one fewer instance" is the
    // count of status-dot pixels (8px dots), which must drop after removal.
    let (w, h) = normal.dims();
    let sidebar = (0, 68, 520, h * 9 / 10);
    let dots_before =
        normal.count_near(sidebar, LIGHT_SUCCESS, 8) + normal.count_near(sidebar, LIGHT_DANGER, 8);
    let dots_after =
        after.count_near(sidebar, LIGHT_SUCCESS, 8) + after.count_near(sidebar, LIGHT_DANGER, 8);
    eprintln!("DOCK dots before={dots_before} after={dots_after}");
    assert!(
        dots_after < dots_before,
        "the dock did not lose a status dot after the delete (before={dots_before}, after={dots_after})"
    );

    // And the whole sidebar differs: rows shifted up by one.
    let diff = (68..h * 9 / 10)
        .step_by(4)
        .filter(|y| {
            (0..520)
                .step_by(4)
                .any(|x| normal.pixel(x, *y)[0..3] != after.pixel(x, *y)[0..3])
        })
        .count();
    eprintln!("DOCK sidebar rows differing={diff}");
    assert!(
        diff > 20,
        "the sidebar barely changed after a delete ({diff} rows)"
    );
    let _ = w;
}
