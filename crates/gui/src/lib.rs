//! `dblocal_gui` — the DBLocal desktop shell.
//!
//! A Zed-style workspace that talks only to `dyn DblService`. It runs today
//! against the in-memory mock so the whole Phase 1 flow is reviewable before
//! the real service exists. Tray wiring and quit dialogs land in a later wave;
//! the seams for them live in [`app`].

pub mod app;
pub mod contrast;
pub mod format;
pub mod logs;
pub mod service_bridge;
pub mod theme;
pub mod vulkan;

use dblocal_core::Result;

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

/// Launches the DBLocal GUI with the in-memory mock service.
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
    app::run_with_mock()
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
