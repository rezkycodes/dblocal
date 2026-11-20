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
pub fn run() -> Result<()> {
    if let Err(report) = vulkan::preflight() {
        eprintln!("{report}");
        std::process::exit(vulkan::EXIT_VULKAN as i32);
    }
    app::run_with_mock()
}
