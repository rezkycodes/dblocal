//! DBLocal GPUI spike library.
//!
//! The window, tray, progress and Vulkan-preflight modules live here so that both the
//! `gpui-spike` binary and the off-screen screenshot test can use them.

pub mod app;
pub mod progress;
pub mod tray;
pub mod vulkan;
