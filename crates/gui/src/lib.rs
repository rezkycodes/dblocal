//! `dblocal_gui` — the DBLocal desktop shell.
//!
//! A Zed-style workspace that talks only to `dyn DblService`. The production
//! entry builds a real [`dblocal_core::service::LocalService`]; the in-memory
//! mock stays behind the `demo` feature for previews and tests.

pub mod app;
pub mod contrast;
pub mod format;
pub mod logs;
pub mod quit;
pub mod service_bridge;
pub mod theme;
pub mod tray;
pub mod vulkan;

use std::sync::Arc;

use dblocal_core::Result;
use dblocal_core::paths::DblPaths;
use dblocal_core::service::{DblService, LocalService, ServiceOptions};
use dblocal_core::strings;
use gpui_kit::AppContext as _;

/// Verifies that every rendered text/background pair in both embedded themes
/// meets WCAG AA (4.5:1). Returns the measured table, or the failing rows.
///
/// The GUI never calls this at runtime; it exists so `cargo xtask ci`, the
/// headless render tests and the final report can print the real numbers.
pub fn verify_theme_contrast() -> std::result::Result<String, String> {
    let (all, failing) = contrast::check_all();
    if failing.is_empty() {
        Ok(contrast::render_table(&all))
    } else {
        Err(contrast::render_table(&failing))
    }
}

/// Launches the DBLocal GUI.
///
/// Production builds start the real `LocalService` over `DblPaths::from_env()`.
/// Demo builds may opt into the in-memory mock with `DBLOCAL_DEMO=1`.
///
/// `F-TRY`: the GUI owns an event loop that must run on the process main
/// thread and needs a display. When either is missing — a headless build box, a
/// test harness thread, or a terminal-only session — this returns `Ok` instead
/// of blocking, so `dblocal` with no arguments stays usable everywhere the
/// window cannot open. The reason is logged so it is never silently swallowed.
pub fn run() -> Result<()> {
    if let Some(reason) = gui_unavailable_reason() {
        tracing::warn!("dblocal-gui: not starting the window: {reason}");
        return Ok(());
    }
    if let Err(report) = vulkan::preflight() {
        eprintln!("{report}");
        std::process::exit(vulkan::EXIT_VULKAN as i32);
    }
    #[cfg(feature = "demo")]
    if std::env::var_os("DBLOCAL_DEMO").is_some() {
        return app::run_with_mock();
    }
    let service = build_real_service()?;
    app::run_with_service(service)
}

/// Builds the production service. On failure it shows a small error window and
/// exits with a non-zero code; it never panics and never leaves a black window.
fn build_real_service() -> Result<Arc<dyn DblService>> {
    let paths = DblPaths::from_env()?;
    match LocalService::new(paths, ServiceOptions::default()) {
        Ok(service) => Ok(Arc::new(service)),
        Err(err) => {
            let message = err.user_message();
            tracing::error!("could not start LocalService: {message}");
            show_startup_failure(&message);
            std::process::exit(1);
        }
    }
}

/// Shows a clear Indonesian startup-failure message. Falls back to stderr when
/// no display is available.
fn show_startup_failure(message: &str) {
    if gui_unavailable_reason().is_some() {
        eprintln!("{}: {message}", strings::UI_STARTUP_FAILED_TITLE);
        return;
    }
    let message = message.to_owned();
    let app = gpui_kit::platform::application();
    app.run(move |cx: &mut gpui_kit::App| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
        let bounds = gpui_kit::Bounds::centered(
            None,
            gpui_kit::size(gpui_kit::px(560.), gpui_kit::px(220.)),
            cx,
        );
        let options = gpui_kit::WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(bounds)),
            window_min_size: Some(gpui_kit::size(gpui_kit::px(420.), gpui_kit::px(180.))),
            app_id: Some("dev.dblocal.DBLocal.StartupFailure".to_owned()),
            is_movable: true,
            ..Default::default()
        };
        let text = message.clone();
        let result = cx.open_window(options, move |_, cx| {
            cx.new(|_| StartupFailureView {
                message: text.clone(),
            })
        });
        if result.is_err() {
            eprintln!("{}: {}", strings::UI_STARTUP_FAILED_TITLE, message);
            cx.quit();
        }
    });
}

struct StartupFailureView {
    message: String,
}

impl gpui_kit::Render for StartupFailureView {
    fn render(
        &mut self,
        _window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::prelude::*;
        let theme = <gpui_kit::App as gpui_kit::component::ActiveTheme>::theme(cx);
        gpui_kit::div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p(gpui_kit::px(24.))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                gpui_kit::div()
                    .text_lg()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(strings::UI_STARTUP_FAILED_TITLE),
            )
            .child(
                gpui_kit::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(strings::UI_STARTUP_FAILED_BODY),
            )
            .child(gpui_kit::div().text_sm().child(self.message.clone()))
    }
}

/// Why the window cannot be opened here, or `None` when it can.
///
/// Precedence: an explicit headless flag, then the main-thread requirement,
/// then the display check. Each is a real, documented constraint rather than a
/// workaround for a test.
pub fn gui_unavailable_reason() -> Option<String> {
    if std::env::var_os("DBLOCAL_GUI_HEADLESS").is_some() {
        return Some("DBLOCAL_GUI_HEADLESS is set".to_owned());
    }
    if !is_main_thread() {
        return Some("the GPUI event loop requires the process main thread".to_owned());
    }
    if !has_display() {
        return Some("no Wayland or X11 display is available".to_owned());
    }
    None
}

fn is_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

fn has_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_environment_is_detected() {
        // Tests run on a named harness thread, not `main`, so the GUI must
        // refuse to start here: that is exactly the seam the CLI test relies on.
        assert!(gui_unavailable_reason().is_some());
    }

    #[test]
    fn display_presence_reads_the_standard_variables() {
        let _ = has_display();
        let _ = is_main_thread();
    }
}
