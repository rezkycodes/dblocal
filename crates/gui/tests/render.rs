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
const DARK_WARNING: [u8; 3] = [0xe5, 0xc0, 0x7b];

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

/// Per-case hang guard. A deadlocked render held the GPU mutex for 54 minutes
/// in CI; aborting with the case name is the only diagnostic a wedged process
/// can emit. Arm it *after* acquiring the GPU mutex so a queued case is not
/// blamed for time spent waiting on an earlier case. Under heavy shared-host
/// load a healthy render can take well over 2 minutes; 10 minutes still catches
/// a real hang without killing slow-but-progressing cases on a loaded host.
const RENDER_WATCHDOG_SECS: u64 = 600;

struct RenderWatchdog {
    done: Arc<std::sync::atomic::AtomicBool>,
}

impl RenderWatchdog {
    /// Arms a timer that aborts the process when `limit` elapses before `Drop`.
    ///
    /// The callback is injectable so the ordering test can observe a firing
    /// instead of aborting the whole test binary. Production passes `abort`.
    fn arm_with(name: &str, limit: Duration, on_fire: impl Fn() + Send + 'static) -> Self {
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let label = name.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            if !flag.load(std::sync::atomic::Ordering::SeqCst) {
                let msg = format!(
                    "RENDER HANG: {label} did not finish within {}s; aborting",
                    limit.as_secs()
                );
                eprintln!("{msg}");
                use std::io::Write as _;
                let _ = std::io::stderr().write_all(msg.as_bytes());
                let _ = std::io::stderr().write_all(b"\n");
                on_fire();
            }
        });
        Self { done }
    }
}

impl Drop for RenderWatchdog {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Locks the render mutex, **then** arms the watchdog, returning both.
///
/// The single ordering every render case uses: a case that queued behind others
/// must not spend its watchdog budget while waiting. `render` keeps the guard
/// until it has drawn and releases it before the context shutdown; the guard and
/// watchdog both outlive the draw, so a hang in draw or shutdown still trips it.
fn lock_then_arm<'a>(
    mutex: &'a std::sync::Mutex<()>,
    name: &str,
    limit: Duration,
    on_fire: impl Fn() + Send + 'static,
) -> (std::sync::MutexGuard<'a, ()>, RenderWatchdog) {
    let guard = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let watchdog = RenderWatchdog::arm_with(name, limit, on_fire);
    (guard, watchdog)
}

/// A case that queued behind another must not be killed for the wait.
///
/// Case A holds the lock longer than B's limit, and B is already queued on it.
/// Because B arms only after acquiring, A's hold never counts against B's
/// budget. `fired` records a watchdog trip instead of aborting this binary.
/// Arming before the lock is the defect: it sets `fired`, and the assertion
/// fails with the same cause a healthy queued render used to die from.
#[test]
fn watchdog_does_not_fire_for_a_queued_case() {
    let mutex = Arc::new(std::sync::Mutex::new(()));
    let a_started = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let started = Arc::clone(&a_started);
    let a_mutex = Arc::clone(&mutex);
    let a = std::thread::spawn(move || {
        let _guard = a_mutex.lock().expect("lock a");
        started.store(true, std::sync::atomic::Ordering::SeqCst);
        // Longer than B's 80 ms limit.
        std::thread::sleep(Duration::from_millis(160));
    });

    while !a_started.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1));
    }

    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_t = Arc::clone(&fired);
    {
        // B queues on the lock while A holds it; its limit is shorter than the
        // remaining hold, so arming early would fire.
        let (_guard_b, watchdog) = lock_then_arm(
            &mutex,
            "queued-case",
            Duration::from_millis(80),
            move || fired_t.store(true, std::sync::atomic::Ordering::SeqCst),
        );
        std::thread::sleep(Duration::from_millis(20));
        drop(watchdog);
    }
    a.join().expect("a joined");
    assert!(
        !fired.load(std::sync::atomic::Ordering::SeqCst),
        "the watchdog fired for a case that was only queued"
    );
}

/// A case that really hangs must trip the watchdog, promptly and with no
/// queueing involved. `on_fire` stands in for `abort`, so this stays in-process.
#[test]
fn watchdog_fires_for_a_stuck_case() {
    let mutex = std::sync::Mutex::new(());
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_t = Arc::clone(&fired);
    {
        let (_guard, _watchdog) =
            lock_then_arm(&mutex, "stuck-case", Duration::from_millis(40), move || {
                fired_t.store(true, std::sync::atomic::Ordering::SeqCst)
            });
        // Do not drop the watchdog: a stuck case never leaves its body.
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            fired.load(std::sync::atomic::Ordering::SeqCst),
            "the watchdog did not fire for a stuck case"
        );
        std::mem::forget(_watchdog);
    }
}

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
    let (guard, watchdog) = lock_then_arm(
        &GPU,
        name,
        Duration::from_secs(RENDER_WATCHDOG_SECS),
        || std::process::abort(),
    );
    let mut app = headless_app(mode_dark);
    let frame = draw_and_capture(&mut app, name, build);
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
fn draw_and_capture(app: &mut HeadlessAppContext, name: &str, build: Build) -> Frame {
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
        if gap > 20 * s && gap < 700 * s {
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
    assert!(selected > 150, "no selected dock row ({selected})");

    // Defect 5: dock rows are one line. Sample the name column across the
    // visible rows and require one text band per 30px row pitch, not two
    // stacked bands (the old status-under-name layout). Both the plain sidebar
    // surface and the selected-row fill count as background.
    let name_col = (78, 86, 210, 246);
    let bands = text_bands_multi(&dark, name_col, &[DARK_SIDEBAR, DARK_ACTIVE_ROW], 12);
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
fn renders_overview_failed_with_diagnosis_banner() {
    let frame = render(
        true,
        "overview-failed-dark.png",
        Workspace::preview_failed_overview,
    );
    let danger_tinted = frame.count_tint(
        (DOCK, 80, LOGICAL_W - 20, LOGICAL_H / 2),
        DARK_BG,
        DARK_DANGER,
    );
    let danger_text = frame.count_near((DOCK, 80, LOGICAL_W - 20, LOGICAL_H / 2), DARK_DANGER, 28);
    eprintln!("OVERVIEW-DIAG tint={danger_tinted} danger_text={danger_text}");
    assert!(
        danger_tinted > 2000 || danger_text > 150,
        "failed overview diagnosis banner not visible (tint={danger_tinted}, text={danger_text})"
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
    let border_dark = [38, 40, 51];
    let border_light = [223, 224, 231];

    // Diagnosis case: 8 lines in the buffer, all visible.
    let diagnosis = render(true, "logs-diagnosis-dark.png", Workspace::preview_failure);
    let body = log_viewer_body(&diagnosis, border_dark);
    let bands = text_bands(&diagnosis, body, DARK_BG, 14);
    eprintln!("DIAG_BODY rect={body:?} bands={}", bands.len());
    assert_eq!(
        bands.len(),
        8,
        "diagnosis viewer body must show 8 log bands, got {} in {body:?}",
        bands.len()
    );
    let danger_tinted = diagnosis.count_tint(
        (DOCK, 100, LOGICAL_W - 20, LOGICAL_H / 2),
        DARK_BG,
        DARK_DANGER,
    );
    let danger_text =
        diagnosis.count_near((DOCK, 100, LOGICAL_W - 20, LOGICAL_H / 2), DARK_DANGER, 28);
    assert!(
        danger_tinted > 3000 || danger_text > 200,
        "diagnosis banner not visible (tint={danger_tinted}, text={danger_text})"
    );

    // Light case: 2 demo lines.
    let light = render(false, "logs-light.png", Workspace::preview_logs);
    let body = log_viewer_body(&light, border_light);
    let bands = text_bands(&light, body, LIGHT_BG, 14);
    eprintln!("LOGS_BODY rect={body:?} bands={}", bands.len());
    assert_eq!(
        bands.len(),
        2,
        "light viewer body must show 2 log bands, got {} in {body:?}",
        bands.len()
    );

    // Short buffer: 1 line.
    let short = render(false, "logs-short-light.png", Workspace::preview_logs_short);
    let body = log_viewer_body(&short, border_light);
    let bands = text_bands(&short, body, LIGHT_BG, 14);
    eprintln!("SHORT_BODY rect={body:?} bands={}", bands.len());
    assert_eq!(
        bands.len(),
        1,
        "short buffer should show 1 band, got {} in {body:?}",
        bands.len()
    );

    // Long buffer: 200 lines, follow on → last lines visible. Band count must
    // equal the number of visible rows that fit the body.
    let long = render(false, "logs-long-light.png", Workspace::preview_logs_long);
    let body = log_viewer_body(&long, border_light);
    let bands = text_bands(&long, body, LIGHT_BG, 14);
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
    // Follow is on, so the last line must be near the bottom of the body.
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
    let product_text = product.count_not_near(body, LIGHT_BG, 12);
    let demo_text = demo.count_not_near(body, LIGHT_BG, 12);
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
fn renders_config_placeholder_in_the_product_build() {
    let frame = render(
        false,
        "config-product-light.png",
        Workspace::preview_product_config,
    );
    let body = (DOCK + 40, 180, LOGICAL_W - 40, LOGICAL_H - 80);
    let text = frame.count_not_near(body, LIGHT_BG, 12);
    eprintln!("CONFIG product={text}");
    assert!(
        text > 8_000,
        "config placeholder should render the prototype structure ({text})"
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
fn renders_quit_dialog_with_both_buttons_and_the_default_marked() {
    let frame = render(true, "quit-dark.png", Workspace::preview_quit);
    let panel = frame.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), [35, 36, 47], 10)
        + frame.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), DARK_SIDEBAR, 10);
    let primary = frame.count_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), DARK_PRIMARY, 18);
    let text = frame.count_not_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), DARK_BG, 16);
    eprintln!("QUIT panel={panel} primary={primary} text={text}");
    assert!(panel > 3000, "quit panel not painted ({panel})");
    assert!(text > 800, "quit dialog body text missing ({text})");
    // Exactly one button is primary: "Biarkan tetap berjalan" is the default.
    assert!(primary > 150, "default button not marked ({primary})");
}

#[test]
fn renders_quit_dialog_reporting_a_failed_stop() {
    let frame = render(
        true,
        "quit-stop-failed-dark.png",
        Workspace::preview_quit_stop_failed,
    );
    let text = frame.count_not_near((160, 60, LOGICAL_W - 160, LOGICAL_H - 60), DARK_BG, 16);
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
    let idle_warn = idle.count_near(bar, DARK_WARNING, 60);
    let pending_warn = pending.count_near(bar, DARK_WARNING, 60);
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
    let warn = frame.count_near(bar, LIGHT_WARNING, 30);
    eprintln!("NO-TRAY warning text={warn}");
    assert!(
        warn > 20,
        "no-tray note missing from the status bar ({warn})"
    );
}

#[test]
fn renders_stale_banner_and_revealed_uri() {
    let stale = render(false, "stale-light.png", Workspace::preview_stale_banner);
    let banner = stale.count_near((DOCK, 34, LOGICAL_W, 140), LIGHT_WARNING, 30)
        + stale.count_tint((DOCK, 34, LOGICAL_W, 140), LIGHT_BG, LIGHT_WARNING);
    eprintln!("STALE banner={banner}");
    assert!(banner > 50, "stale catalog banner missing ({banner})");

    let revealed = render(
        true,
        "overview-revealed-dark.png",
        Workspace::preview_revealed,
    );
    assert!(revealed.colours > 40, "revealed render too flat");
}
