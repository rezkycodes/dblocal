//! Compiles the real Windows/macOS tray sources for a non-Linux target.
//!
//! `src/tray/{mod.rs,spec.rs,desktop.rs}` are byte-for-byte copies of
//! `crates/gui/src/tray/`; only `format` and `dblocal_core` are stubbed, because
//! the real core cannot be cross-compiled here (`aws-lc-sys` needs a mingw C
//! compiler). A successful `cargo check --target x86_64-pc-windows-gnu` here
//! therefore proves the backend's types and API calls against the real
//! `tray-icon` 0.26 / `muda` 0.21 for Windows.

pub mod format;
pub mod tray;

#[allow(dead_code)]
fn _assert_tray_shape() {
    // Keeps the module reachable if a future edit removes the only user.
    let _ = tray::TrayMenuModel::from_views(&[]);
}
