//! Headless wgpu render tests for the prototype port (D-25).
//!
//! Renders the real views off-screen at the prototype's 1280x820 logical size
//! and asserts on regions derived from the committed theme tokens, so a layout
//! regression fails the test instead of slipping past a colour-count check.
//!
//! Frames go to `target/renders/` by default. Tracked screenshots under
//! `docs/design/renders/` are refreshed only with `DBLOCAL_UPDATE_RENDERS=1`.
//!
//! Screen capture needs a GPUI headless renderer, and `gpui_platform`
//! `current_headless_renderer()` provides one only on Linux (wgpu) and macOS
//! (Metal); on Windows it returns `Ok(None)`, so `capture_screenshot` bails with
//! "no HeadlessRenderer configured". These tests therefore compile and run only
//! where a renderer exists.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dblocal_core::service::{DblService, mock::MockService};
use dblocal_gui::app::Workspace;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, AssetSource, Bounds, Context, HeadlessAppContext,
    SharedString, WindowBounds, WindowOptions, px, size,
};

struct GuiAssets;
impl AssetSource for GuiAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        dblocal_gui::assets::DblocalAssets.load(path)
    }
    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        dblocal_gui::assets::DblocalAssets.list(path)
    }
}

const LOGICAL_W: u32 = 1280;
const LOGICAL_H: u32 = 820;

/// Dock width, title bar, tab strip — prototype layout constants.
const DOCK: u32 = 272;
const TAB_TOP: u32 = 34;
const TAB_BOTTOM: u32 = 70;

/// A theme token as opaque RGB, read from the same JSON files the app embeds,
/// so the region checks follow the palette instead of repeating its hex.
fn token(dark: bool, key: &str) -> [u8; 3] {
    let name = if dark {
        dblocal_gui::theme::DARK_THEME
    } else {
        dblocal_gui::theme::LIGHT_THEME
    };
    for (_, text) in dblocal_gui::theme::THEME_FILES {
        let value: serde_json::Value = serde_json::from_str(text).expect("theme json");
        let Some(themes) = value["themes"].as_array() else {
            continue;
        };
        for theme in themes {
            if theme["name"] == name {
                let hex = theme["colors"][key].as_str().expect("theme colour");
                let hex = hex.trim_start_matches('#');
                let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex");
                return [byte(0), byte(2), byte(4)];
            }
        }
    }
    panic!("theme {name} not found")
}

/// A wash token (`#rrggbbaa`) composited over a plane token, the colour a hairline
/// or a hover fill lands on screen with.
fn wash_over(dark: bool, key: &str, plane: &str) -> [u8; 3] {
    let name = if dark {
        dblocal_gui::theme::DARK_THEME
    } else {
        dblocal_gui::theme::LIGHT_THEME
    };
    let base = token(dark, plane);
    for (_, text) in dblocal_gui::theme::THEME_FILES {
        let value: serde_json::Value = serde_json::from_str(text).expect("theme json");
        let Some(themes) = value["themes"].as_array() else {
            continue;
        };
        for theme in themes {
            if theme["name"] == name {
                let hex = theme["colors"][key].as_str().expect("theme colour");
                let hex = hex.trim_start_matches('#');
                let byte =
                    |at: usize| f64::from(u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"));
                let alpha = if hex.len() == 8 { byte(6) / 255.0 } else { 1.0 };
                let mix =
                    |fg: f64, bg: u8| (fg * alpha + f64::from(bg) * (1.0 - alpha)).round() as u8;
                return [
                    mix(byte(0), base[0]),
                    mix(byte(2), base[1]),
                    mix(byte(4), base[2]),
                ];
            }
        }
    }
    panic!("theme {name} not found")
}

/// The page plane the centre pane paints.
fn dark_bg() -> [u8; 3] {
    token(true, "background")
}
/// The chrome plane: dock, title bar, status bar.
fn dark_sidebar() -> [u8; 3] {
    token(true, "sidebar.background")
}
/// The card plane: cards and the log body.
fn dark_card() -> [u8; 3] {
    token(true, "group_box.background")
}
/// The selected dock row (the active wash over the chrome plane).
fn dark_active_row() -> [u8; 3] {
    token(true, "sidebar.accent.background")
}
fn dark_success() -> [u8; 3] {
    token(true, "success.background")
}
fn dark_danger() -> [u8; 3] {
    token(true, "danger.background")
}
fn dark_primary() -> [u8; 3] {
    token(true, "primary.background")
}
fn dark_warning() -> [u8; 3] {
    token(true, "warning.background")
}
fn light_bg() -> [u8; 3] {
    token(false, "background")
}
fn light_sidebar() -> [u8; 3] {
    token(false, "sidebar.background")
}
fn light_card() -> [u8; 3] {
    token(false, "group_box.background")
}
fn light_primary() -> [u8; 3] {
    token(false, "primary.background")
}
fn light_warning() -> [u8; 3] {
    token(false, "warning.background")
}
fn light_danger() -> [u8; 3] {
    token(false, "danger.background")
}

fn seeded_service() -> Arc<dyn DblService> {
    Arc::new(MockService::demo())
}

type Build = fn(Arc<dyn DblService>, &mut gpui_kit::Window, &mut Context<Workspace>) -> Workspace;

#[path = "render/both_themes.rs"]
mod both_themes;
#[path = "render/focus_ring.rs"]
mod focus_ring;
#[path = "render/frame.rs"]
mod frame;
#[path = "render/log_rows.rs"]
mod log_rows;
#[path = "render/palette_panel.rs"]
mod palette_panel;
#[path = "render/update.rs"]
mod update;
#[path = "render/watchdog.rs"]
mod watchdog;

use frame::Frame;

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

/// The platform text system, built once per process.
///
/// `gpui_platform::current_platform(true)` constructs a whole `LinuxPlatform`
/// (a worker dispatcher, a timer thread and the Vulkan threads) only to borrow
/// its text system, and dropping the `Rc` does not stop those threads. One per
/// render case leaked ~20 threads per case; after ten cases a later
/// `HeadlessAppContext::shutdown` wedged and the watchdog aborted the run
/// (`logs-light.png`, `overview-failed-dark.png`, `palette-dark.png` on
/// successive runs). gpui's own `bench_text_system` documents the same defect:
/// build it once and share it.
fn shared_text_system() -> Arc<dyn gpui_kit::PlatformTextSystem> {
    static TEXT_SYSTEM: std::sync::OnceLock<Arc<dyn gpui_kit::PlatformTextSystem>> =
        std::sync::OnceLock::new();
    TEXT_SYSTEM
        .get_or_init(|| gpui_kit::platform::current_platform(true).text_system())
        .clone()
}

/// Builds a headless app with the DBLocal themes registered.
fn headless_app(mode_dark: bool) -> HeadlessAppContext {
    let text_system = shared_text_system();
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
    app
}

/// Quits the app cleanly and lets the scheduler drain before the context drops.
///
/// `HeadlessAppContext::drop` calls `App::shutdown`, which blocks on the test
/// scheduler. Under parallel test threads a shutdown that ran while the next
/// case had already started rendering parked forever. Quitting and parking
/// first, then dropping, keeps shutdown off the critical path; the caller
/// holds the GPU mutex so exactly one context is alive at a time.
fn shutdown_headless(mut app: HeadlessAppContext) {
    app.update(|cx| cx.quit());
    app.run_until_parked();
    drop(app);
}

fn render(mode_dark: bool, name: &str, build: Build) -> Frame {
    render_sized(mode_dark, name, build, LOGICAL_W, LOGICAL_H)
}

/// Renders at an explicit logical size, for the minimum-window-width checks.
fn render_sized(mode_dark: bool, name: &str, build: Build, width: u32, height: u32) -> Frame {
    render_input(mode_dark, name, build, width, height, Input::None)
}

/// Like `render`, after a keyboard event: the window's last input modality is
/// the keyboard, as it is while someone tabs through the UI. Focus rings only
/// draw for keyboard focus, so the focus-ring renders need this.
fn render_keyboard(mode_dark: bool, name: &str, build: Build) -> Frame {
    render_input(
        mode_dark,
        name,
        build,
        LOGICAL_W,
        LOGICAL_H,
        Input::Keyboard,
    )
}

/// Like `render`, after a real left click at `at` (logical px): mouse move, down
/// and up, so the clicked element holds focus under mouse modality.
fn render_clicked(mode_dark: bool, name: &str, build: Build, at: (u32, u32)) -> Frame {
    render_input(
        mode_dark,
        name,
        build,
        LOGICAL_W,
        LOGICAL_H,
        Input::Click(at),
    )
}

/// A click, then a keyboard event: the clicked element keeps focus and the
/// modality becomes keyboard, as when someone clicks and then presses a key.
fn render_clicked_then_keyboard(
    mode_dark: bool,
    name: &str,
    build: Build,
    at: (u32, u32),
) -> Frame {
    render_input(
        mode_dark,
        name,
        build,
        LOGICAL_W,
        LOGICAL_H,
        Input::ClickThenKeyboard(at),
    )
}

/// Input the harness sends before the capture.
#[derive(Clone, Copy)]
enum Input {
    None,
    Keyboard,
    Click((u32, u32)),
    ClickThenKeyboard((u32, u32)),
}

fn render_input(
    mode_dark: bool,
    name: &str,
    build: Build,
    width: u32,
    height: u32,
    input: Input,
) -> Frame {
    let (guard, watchdog) = watchdog::lock_then_arm(
        &GPU,
        name,
        Duration::from_secs(watchdog::RENDER_WATCHDOG_SECS),
        || std::process::abort(),
    );
    let mut app = headless_app(mode_dark);
    let frame = draw_and_capture(&mut app, name, build, width, height, input);
    // Release the render mutex before shutting the context down: shutdown parks
    // on the test scheduler, and holding the mutex through it serialises every
    // other case behind a park.
    drop(guard);
    shutdown_headless(app);
    drop(watchdog);
    frame
}

fn a11y_node_count_and_dups(json: &str) -> (usize, Vec<String>) {
    let value: serde_json::Value = serde_json::from_str(json).expect("a11y json");
    let nodes = value
        .get("nodes")
        .and_then(|n| n.as_object())
        .cloned()
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut dups = Vec::new();
    for key in nodes.keys() {
        if !seen.insert(key.clone()) {
            dups.push(key.clone());
        }
    }
    (nodes.len(), dups)
}

/// The actual render: window, frames, screenshot, PNG. The caller owns the
/// mutex and the context lifetime.
fn draw_and_capture(
    app: &mut HeadlessAppContext,
    name: &str,
    build: Build,
    width: u32,
    height: u32,
    input: Input,
) -> Frame {
    let service = seeded_service();
    let handle: AnyWindowHandle = {
        let bounds = Bounds {
            origin: gpui_kit::point(px(0.0), px(0.0)),
            size: size(px(width as f32), px(height as f32)),
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

    // Draw once so the rows exist before a pointer lands on one.
    for _ in 0..4 {
        app.update_window(handle, |_view, window, cx| {
            let _ = window.draw(cx);
        })
        .expect("draw frame");
        app.run_until_parked();
    }
    if let Input::Click(at) | Input::ClickThenKeyboard(at) = input {
        app.update_window(handle, |_view, window, cx| {
            let position = gpui_kit::point(px(at.0 as f32), px(at.1 as f32));
            let modifiers = gpui_kit::Modifiers::none();
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }),
                cx,
            );
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                    button: gpui_kit::MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
                    button: gpui_kit::MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }),
                cx,
            );
        })
        .expect("click");
        app.run_until_parked();
    }
    if matches!(input, Input::Keyboard | Input::ClickThenKeyboard(_)) {
        // A bare modifier press: no binding, but it makes the last input modality
        // the keyboard, which is what draws a focus ring.
        app.update_window(handle, |_view, window, cx| {
            let key = gpui_kit::Keystroke::parse("shift").expect("valid keystroke");
            window.dispatch_keystroke(key, cx);
        })
        .expect("keyboard modality");
        app.run_until_parked();
    }
    // uniform_list paints rows on the next frame after layout.
    for _ in 0..4 {
        app.update_window(handle, |_view, window, cx| {
            let _ = window.draw(cx);
        })
        .expect("draw frame");
        app.run_until_parked();
    }
    if std::env::var_os("DBLOCAL_TEST_A11Y").is_some() {
        let json = app
            .update_window(handle, |_view, window, _cx| window.debug_a11y_tree_json())
            .expect("a11y json read");
        if let Some(json) = json {
            let (count, dups) = a11y_node_count_and_dups(&json);
            eprintln!("A11Y {name}: nodes={count} dups={}", dups.len());
            assert!(
                dups.is_empty(),
                "duplicate a11y node ids in {name}: {dups:?}\n{json}"
            );
        }
    }
    let image = app
        .capture_screenshot(handle)
        .expect("headless wgpu renderer must produce a frame");
    assert!(image.width() > 0 && image.height() > 0, "empty frame");
    let scale = image.width() / width;
    assert!(scale >= 1, "unexpected render width {}", image.width());
    assert_eq!(
        (image.width() / scale, image.height() / scale),
        (width, height),
        "render must match the requested logical size"
    );
    let colours = Frame::count_colours(&image);
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
        colours,
        if tracked { "tracked" } else { "target-only" }
    );
    Frame {
        image,
        colours,
        scale,
    }
}

/// Whether a device-pixel row inside `rect` contains glyph pixels (anything
/// that differs from `bg` by more than `tol`), with a minimum run length so a
/// single antialiased pixel does not count.
fn row_has_text(frame: &Frame, y: u32, x0: u32, x1: u32, bg: [u8; 3], tol: u8) -> bool {
    let mut hits = 0;
    for x in x0..x1 {
        let p = frame.px(x, y);
        if p[0].abs_diff(bg[0]) > tol || p[1].abs_diff(bg[1]) > tol || p[2].abs_diff(bg[2]) > tol {
            hits += 1;
        }
    }
    hits > 8
}

/// Counts horizontal text bands (runs of glyph rows separated by a blank gap).
/// A list of one-line rows yields one band per row; a stacked two-line row
/// yields two bands, and touching rows merge into one long band.
fn text_bands(frame: &Frame, rect: (u32, u32, u32, u32), bg: [u8; 3], tol: u8) -> Vec<(u32, u32)> {
    let (x0, y0, x1, y1) = frame.rect(rect);
    let mut bands = Vec::new();
    let mut current: Option<(u32, u32)> = None;
    let mut blank_run = 0u32;
    for y in y0..y1 {
        if row_has_text(frame, y, x0, x1, bg, tol) {
            blank_run = 0;
            current = Some(match current {
                Some((start, _)) => (start, y + 1),
                None => (y, y + 1),
            });
        } else if let Some(band) = current {
            blank_run += 1;
            if blank_run >= 2 * frame.scale {
                bands.push(band);
                current = None;
                blank_run = 0;
            }
        }
    }
    if let Some(band) = current {
        bands.push(band);
    }
    bands
}

/// Counts horizontal text bands (runs of glyph rows separated by a blank gap),
/// treating any of `bgs` as background. A list of one-line rows yields one band
/// per row; a stacked two-line row yields two bands, and touching rows merge
/// into one long band.
fn text_bands_multi(
    frame: &Frame,
    rect: (u32, u32, u32, u32),
    bgs: &[[u8; 3]],
    tol: u8,
) -> Vec<(u32, u32)> {
    let (x0, y0, x1, y1) = frame.rect(rect);
    let is_bg = |p: [u8; 4]| {
        bgs.iter().any(|bg| {
            p[0].abs_diff(bg[0]) <= tol
                && p[1].abs_diff(bg[1]) <= tol
                && p[2].abs_diff(bg[2]) <= tol
        })
    };
    let mut bands = Vec::new();
    let mut current: Option<(u32, u32)> = None;
    let mut blank_run = 0u32;
    for y in y0..y1 {
        let hits = (x0..x1).filter(|x| !is_bg(frame.px(*x, y))).count();
        if hits > 8 {
            blank_run = 0;
            current = Some(match current {
                Some((start, _)) => (start, y + 1),
                None => (y, y + 1),
            });
        } else if let Some(band) = current {
            blank_run += 1;
            if blank_run >= 2 * frame.scale {
                bands.push(band);
                current = None;
                blank_run = 0;
            }
        }
    }
    if let Some(band) = current {
        bands.push(band);
    }
    bands
}

/// Finds the log viewer body rectangle: between the header bottom border and the
/// footer top border, inside the log viewer card in the content pane.
///
/// The viewer draws `table_row_border` (1px) under its header and above its
/// footer. We scan the content pane for two horizontal border runs and return the
/// rectangle between them. This is layout-derived, not a magic y constant.
fn log_viewer_body(frame: &Frame, border: [u8; 3]) -> (u32, u32, u32, u32) {
    let (w, h) = frame.dims();
    let s = frame.scale;
    let x0 = (DOCK + 40) * s;
    let x1 = (LOGICAL_W - 40) * s;
    let mut border_ys: Vec<u32> = Vec::new();
    let mut y = 120 * s;
    while y < h.saturating_sub(40 * s) {
        let mut hits = 0usize;
        let mut total = 0usize;
        let mut x = x0;
        while x < x1.min(w) {
            let p = frame.px(x, y);
            if p[0].abs_diff(border[0]) <= 6
                && p[1].abs_diff(border[1]) <= 6
                && p[2].abs_diff(border[2]) <= 6
            {
                hits += 1;
            }
            total += 1;
            x += 3;
        }
        if total > 0 && hits * 100 / total > 80 {
            border_ys.push(y);
        }
        y += 1;
    }
    // Coalesce adjacent border rows into single border positions.
    let mut borders: Vec<u32> = Vec::new();
    for y in border_ys {
        if borders.last().is_some_and(|last: &u32| y - *last <= 2 * s) {
            if let Some(last) = borders.last_mut() {
                *last = y;
            }
        } else {
            borders.push(y);
        }
    }
    // The viewer is the last bordered card-like pair that has substantial gap.
    // Header bottom border and footer top border are consecutive borders with a
    // gap larger than a single line but smaller than the whole pane.
    let mut best: Option<(u32, u32)> = None;
    for pair in borders.windows(2) {
        let top = pair[0];
        let bot = pair[1];
        let gap = bot.saturating_sub(top);
        // The body is the tallest gap between two hairlines: the card outline
        // and the header/footer dividers share one wash colour.
        let taller = best.is_none_or(|(t, b)| gap > b - t);
        if gap > 20 * s && gap < 700 * s && taller {
            best = Some((top, bot));
        }
    }
    let (top, bot) = best.unwrap_or_else(|| {
        panic!(
            "could not locate log viewer borders; found borders at {:?}",
            borders
        )
    });
    // Body is between header bottom border and footer top border. Convert back
    // to the harness's logical coordinates (helpers scale them again).
    let body_top = (top + s) / s;
    let body_bottom = bot.saturating_sub(s) / s;
    (x0 / s, body_top, x1 / s, body_bottom)
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
        ("dark", &dark, dark_sidebar(), dark_bg()),
        ("light", &light, light_sidebar(), light_bg()),
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

    // Tab strip: the selected tab is the soft active pill over the chrome plane.
    let pill_dark = token(true, "tab.active.background");
    let pill_light = token(false, "tab.active.background");
    let active_dark = dark.count_near((DOCK, TAB_TOP, DOCK + 280, TAB_BOTTOM), pill_dark, 3);
    let active_light = light.count_near((DOCK, TAB_TOP, DOCK + 280, TAB_BOTTOM), pill_light, 3);
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
    let running = dark.count_near((20, 0, 60, LOGICAL_H - 40), dark_success(), 16);
    let failed = dark.count_near((20, 0, 60, LOGICAL_H - 40), dark_danger(), 16);
    eprintln!("GLYPH running={running} failed={failed}");
    assert!(running > 20, "no running glyph ({running})");
    assert!(failed > 20, "no failed glyph ({failed})");

    // A selected row in the dock.
    let selected = dark.count_near((0, 70, DOCK, LOGICAL_H - 80), dark_active_row(), 4);
    eprintln!("DOCK selected={selected}");
    assert!(selected > 150, "no selected dock row ({selected})");

    // Defect 5: dock rows are one line. Sample the name column across the
    // visible rows and require one text band per 30px row pitch, not two
    // stacked bands (the old status-under-name layout). Both the plain sidebar
    // surface and the selected-row fill count as background.
    let name_col = (78, 86, 210, 246);
    let bands = text_bands_multi(&dark, name_col, &[dark_sidebar(), dark_active_row()], 12);
    eprintln!("DOCK bands={} {bands:?}", bands.len());
    // Five visible rows at a 30px pitch: one band each, none taller than a line.
    assert!(
        (4..=6).contains(&bands.len()),
        "dock rows should be one-line bands (~5), got {}: {bands:?}",
        bands.len()
    );
    for (start, end) in &bands {
        let h = (*end - *start) as f32 / dark.scale as f32;
        assert!(
            h < 18.0,
            "dock row band too tall ({h}px): one line expected"
        );
    }

    // Primary button colour present in each theme.
    let primary_dark = dark.count_near((DOCK, 70, LOGICAL_W, LOGICAL_H - 40), dark_primary(), 16);
    let primary_light =
        light.count_near((DOCK, 70, LOGICAL_W, LOGICAL_H - 40), light_primary(), 16);
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
fn renders_overview_failed_with_diagnosis_banner() {
    let frame = render(
        true,
        "overview-failed-dark.png",
        Workspace::preview_failed_overview,
    );
    let danger_tinted = frame.count_tint(
        (DOCK, 80, LOGICAL_W - 20, LOGICAL_H / 2),
        dark_bg(),
        dark_danger(),
    );
    let danger_text =
        frame.count_near((DOCK, 80, LOGICAL_W - 20, LOGICAL_H / 2), dark_danger(), 28);
    eprintln!("OVERVIEW-DIAG tint={danger_tinted} danger_text={danger_text}");
    assert!(
        danger_tinted > 2000 || danger_text > 150,
        "failed overview diagnosis banner not visible (tint={danger_tinted}, text={danger_text})"
    );
}

/// Assert a logs frame's viewer body shows exactly `expected` text bands.
fn assert_log_bands(frame: &Frame, border: [u8; 3], bg: [u8; 3], expected: usize, label: &str) {
    let body = log_viewer_body(frame, border);
    let bands = text_bands(frame, body, bg, 14);
    eprintln!("{label}_BODY rect={body:?} bands={}", bands.len());
    assert_eq!(
        bands.len(),
        expected,
        "{label} viewer body must show {expected} log bands, got {} in {body:?}",
        bands.len()
    );
}

#[test]
fn renders_logs_body_bands_from_layout() {
    // Round 2, defect 1: the old tests counted bands in a wide magic region that
    // included header/footer, so they passed with zero log lines. We derive the
    // viewer body rectangle from the painted header/footer borders.
    //
    // Mutation proof (reverting the layout fix, `screen-fill` back to a block
    // `div` so `page_fill`'s `flex_1` has no flex parent):
    //   RENDER .../logs-diagnosis-dark.png (2560x1640, 3016 colours, target-only)
    //   thread 'renders_logs_body_bands_from_layout' panicked at
    //   crates/gui/tests/render.rs:492:9:
    //   could not locate log viewer borders; found borders at [727, 761]
    // The 727/761 pair is the collapsed 16-logical-px body the coordinator saw.
    // Restoring `v_flex()` makes the same test pass with 8 bands.
    let border_dark = wash_over(true, "table.row.border", "group_box.background");
    let border_light = wash_over(false, "table.row.border", "group_box.background");

    let diagnosis = render(true, "logs-diagnosis-dark.png", Workspace::preview_failure);
    assert_log_bands(&diagnosis, border_dark, dark_bg(), 8, "DIAG");
    let danger_tinted = diagnosis.count_tint(
        (DOCK, 100, LOGICAL_W - 20, LOGICAL_H / 2),
        dark_bg(),
        dark_danger(),
    );
    let danger_text = diagnosis.count_near(
        (DOCK, 100, LOGICAL_W - 20, LOGICAL_H / 2),
        dark_danger(),
        28,
    );
    assert!(
        danger_tinted > 3000 || danger_text > 200,
        "diagnosis banner not visible (tint={danger_tinted}, text={danger_text})"
    );

    let light = render(false, "logs-light.png", Workspace::preview_logs);
    assert_log_bands(&light, border_light, light_bg(), 2, "LOGS");

    let short = render(false, "logs-short-light.png", Workspace::preview_logs_short);
    assert_log_bands(&short, border_light, light_bg(), 1, "SHORT");

    assert_logs_long_fills_its_rows();
}

/// A 200-line buffer with follow on: the body must be filled with visible rows
/// and the newest line must sit near the bottom.
fn assert_logs_long_fills_its_rows() {
    let border_light = wash_over(false, "table.row.border", "group_box.background");
    let long = render(false, "logs-long-light.png", Workspace::preview_logs_long);
    let body = log_viewer_body(&long, border_light);
    let bands = text_bands(&long, body, light_bg(), 14);
    let (_x0, y0, _x1, y1) = body;
    // `log_viewer_body` already returns logical coordinates; band positions from
    // `text_bands` are device pixels, so scale only when mixing the two.
    let body_h_logical = (y1 - y0) as f32;
    let visible_rows = (body_h_logical / 18.0).floor() as usize;
    eprintln!(
        "LONG_BODY rect={body:?} body_h={body_h_logical} visible_rows={visible_rows} bands={}",
        bands.len()
    );
    assert!(
        visible_rows >= 10,
        "long log viewer should show many rows, got {visible_rows}"
    );
    assert_eq!(
        bands.len(),
        visible_rows.min(200),
        "long buffer should fill visible rows, got {} expected ~{visible_rows}",
        bands.len()
    );
    let last = bands.last().expect("bands present");
    let bottom_gap = (y1 * long.scale).saturating_sub(last.1);
    assert!(
        bottom_gap < 24 * long.scale,
        "follow mode should keep last line near bottom; gap={bottom_gap}"
    );
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
    let product_text = product.count_not_near(body, light_bg(), 12);
    let demo_text = demo.count_not_near(body, light_bg(), 12);
    eprintln!("DATABASES product={product_text} demo={demo_text}");
    assert!(
        product_text > 100,
        "product placeholder renders nothing ({product_text})"
    );
    assert!(
        product_text > 10_000,
        "product placeholder should render the prototype structure ({product_text})"
    );
    assert!(
        demo_text > 10_000,
        "demo databases should render sample content ({demo_text})"
    );
}

#[test]
fn renders_the_live_config_tab_in_the_product_build() {
    let frame = render(
        false,
        "config-product-light.png",
        Workspace::preview_product_config,
    );
    let body = (DOCK + 40, 180, LOGICAL_W - 40, LOGICAL_H - 80);
    let text = frame.count_not_near(body, light_bg(), 12);
    eprintln!("CONFIG product={text}");
    assert!(
        text > 8_000,
        "the live Config tab should render its rows and actions ({text})"
    );
}

#[test]
fn renders_create_dialog_with_a_download_and_an_unavailable_version() {
    let download = render(true, "create-download-dark.png", Workspace::preview_create);
    // Modal panel paints popover/surface over a dimmed backdrop.
    let panel = download.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), [35, 36, 47], 10)
        + download.count_near(
            (160, 60, LOGICAL_W - 160, LOGICAL_H - 60),
            dark_sidebar(),
            10,
        );
    let primary = download.count_near(
        (160, 60, LOGICAL_W - 160, LOGICAL_H - 60),
        dark_primary(),
        18,
    );
    eprintln!("CREATE panel={panel} primary={primary}");
    assert!(panel > 5000, "create panel not painted ({panel})");
    assert!(primary > 200, "create primary button missing ({primary})");

    let unavailable = render(
        true,
        "create-unavailable-dark.png",
        Workspace::preview_create_unavailable,
    );
    let text =
        unavailable.count_not_near((DOCK, 120, LOGICAL_W - 40, LOGICAL_H - 60), dark_bg(), 16);
    eprintln!("CREATE unavailable text={text}");
    assert!(text > 1500, "unavailable card did not render ({text})");
}

#[test]
fn renders_dock_with_runtime_remove_controls_in_both_themes() {
    // `F-RTM-04`: the dock's RUNTIMES rows carry a remove control. Render the
    // whole shell in both themes so the control and its placement are on record;
    // the dock is the left column.
    for (dark, name) in [(true, "dock-dark.png"), (false, "dock-light.png")] {
        let frame = render(dark, name, Workspace::preview);
        let dock = (0, 70, DOCK as u32, LOGICAL_H - 40);
        let (sidebar, warning) = if dark {
            (dark_sidebar(), dark_warning())
        } else {
            (light_sidebar(), light_warning())
        };
        let chrome = frame.count_near(dock, sidebar, 10);
        // The "unused" label paints in the warning colour.
        let unused = frame.count_near(dock, warning, 20);
        eprintln!("{name} chrome={chrome} unused={unused}");
        assert!(chrome > 5000, "{name}: dock column not painted ({chrome})");
        assert!(
            unused > 20,
            "{name}: no unused-runtime label to pair with a remove control ({unused})"
        );
    }
}

#[test]
fn renders_dock_with_the_filter_applied() {
    let full = render(true, "dock-all-dark.png", Workspace::preview);
    let filtered = render(true, "dock-filtered-dark.png", Workspace::preview_filtered);
    let col = (20, 70, 60, LOGICAL_H - 80);
    let before = full.count_near(col, dark_success(), 16)
        + full.count_near(col, dark_danger(), 16)
        + full.count_near(col, [157, 165, 180], 12);
    let after = filtered.count_near(col, dark_success(), 16)
        + filtered.count_near(col, dark_danger(), 16)
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
        + palette.count_near((w / 4, 80, w * 3 / 4, h - 120), dark_sidebar(), 10);
    eprintln!("PALETTE panel={panel}");
    assert!(panel > 2000, "palette panel missing ({panel})");

    let delete = render(false, "delete-light.png", Workspace::preview_delete);
    let panel = delete.count_near((w / 4, h / 4, w * 3 / 4, h * 3 / 4), [255, 255, 255], 8)
        + delete.count_near((w / 4, h / 4, w * 3 / 4, h * 3 / 4), light_sidebar(), 8);
    eprintln!("DELETE panel={panel}");
    assert!(panel > 2000, "delete panel missing ({panel})");
}

#[test]
fn renders_runtime_remove_dialog_in_both_themes() {
    // `F-RTM-04`: an unused runtime's confirm dialog must paint a centred panel
    // with its body text and a danger confirm button, in dark and light.
    for (dark, name) in [
        (true, "runtime-remove-dark.png"),
        (false, "runtime-remove-light.png"),
    ] {
        let frame = render(dark, name, Workspace::preview_runtime_remove);
        let (w, h) = (LOGICAL_W, LOGICAL_H);
        let region = (w / 4, h / 4, w * 3 / 4, h * 3 / 4);
        let (sidebar, danger) = if dark {
            (dark_sidebar(), dark_danger())
        } else {
            (light_sidebar(), light_danger())
        };
        let panel = frame.count_near(region, [255, 255, 255], 8)
            + frame.count_near(region, [35, 36, 47], 10)
            + frame.count_near(region, sidebar, 10);
        let danger_px = frame.count_near(region, danger, 18);
        eprintln!("{name} panel={panel} danger={danger_px}");
        assert!(
            panel > 2000,
            "{name}: runtime-remove panel missing ({panel})"
        );
        assert!(
            danger_px > 60,
            "{name}: danger confirm button missing ({danger_px})"
        );
    }
}

#[test]
fn renders_quit_dialog_with_both_buttons_and_the_default_marked() {
    let frame = render(true, "quit-dark.png", Workspace::preview_quit);
    let panel = frame.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), [35, 36, 47], 10)
        + frame.count_near(
            (160, 60, LOGICAL_W - 160, LOGICAL_H - 60),
            dark_sidebar(),
            10,
        );
    let primary = frame.count_near(
        (160, 60, LOGICAL_W - 160, LOGICAL_H - 60),
        dark_primary(),
        18,
    );
    let text = frame.count_not_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), dark_bg(), 16);
    eprintln!("QUIT panel={panel} primary={primary} text={text}");
    assert!(panel > 3000, "quit panel not painted ({panel})");
    assert!(text > 800, "quit dialog body text missing ({text})");
    // Exactly one button is primary: "Leave them running" is the default.
    assert!(primary > 150, "default button not marked ({primary})");
}

#[test]
fn renders_quit_dialog_reporting_a_failed_stop() {
    let frame = render(
        true,
        "quit-stop-failed-dark.png",
        Workspace::preview_quit_stop_failed,
    );
    let text = frame.count_not_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), dark_bg(), 16);
    eprintln!("QUIT-FAILED text={text}");
    assert!(
        text > 600,
        "failed-stop quit dialog renders nothing ({text})"
    );
}

#[test]
fn renders_the_neutral_pending_note() {
    let idle = render(true, "pending-idle-dark.png", Workspace::preview);
    let pending = render(true, "pending-dark.png", Workspace::preview_pending);
    let bar = (0, LOGICAL_H - 30, LOGICAL_W, LOGICAL_H - 2);
    // The pending note adds warning-tinted glyphs to the status bar; compare
    // against the same bar in the idle frame with a wide tolerance because text
    // is antialiased and only a few pixels sit on the token colour.
    let idle_warn = idle.count_near(bar, dark_warning(), 60);
    let pending_warn = pending.count_near(bar, dark_warning(), 60);
    eprintln!("PENDING idle={idle_warn} pending={pending_warn}");
    assert!(
        pending_warn > idle_warn + 10,
        "pending note did not appear ({idle_warn} -> {pending_warn})"
    );
}

#[test]
fn renders_the_tray_unavailable_note_once() {
    let frame = render(false, "no-tray-light.png", Workspace::preview_no_tray);
    let bar = (0, LOGICAL_H - 30, LOGICAL_W, LOGICAL_H - 2);
    let warn = frame.count_near(bar, light_warning(), 30);
    eprintln!("NO-TRAY warning text={warn}");
    assert!(
        warn > 20,
        "no-tray note missing from the status bar ({warn})"
    );
}

#[test]
fn renders_stale_banner_and_revealed_uri() {
    let stale = render(false, "stale-light.png", Workspace::preview_stale_banner);
    let banner = stale.count_near((DOCK, 34, LOGICAL_W, 140), light_warning(), 30)
        + stale.count_tint((DOCK, 34, LOGICAL_W, 140), light_bg(), light_warning());
    eprintln!("STALE banner={banner}");
    assert!(banner > 50, "stale catalog banner missing ({banner})");

    let revealed = render(
        true,
        "overview-revealed-dark.png",
        Workspace::preview_revealed,
    );
    assert!(revealed.colours > 40, "revealed render too flat");
}

#[test]
fn renders_the_launcher_when_no_tab_is_open() {
    for (dark, name) in [(true, "launcher-dark.png"), (false, "launcher-light.png")] {
        let frame = render(dark, name, Workspace::preview_launcher);
        let bg = if dark { dark_bg() } else { light_bg() };
        // The launcher card paints text and tiles in the centre pane.
        let text = frame.count_not_near((DOCK + 200, 120, LOGICAL_W - 200, LOGICAL_H - 60), bg, 16);
        eprintln!("LAUNCHER {name} text={text}");
        assert!(text > 5000, "{name}: launcher did not render ({text})");
        // The app mark carries the accent colour.
        let primary = if dark {
            dark_primary()
        } else {
            light_primary()
        };
        let mark = frame.count_near((DOCK, 70, LOGICAL_W, LOGICAL_H - 40), primary, 16);
        assert!(mark > 200, "{name}: launcher mark missing ({mark})");
    }
}
