//! Layout regression tests for the Toolchains GUI at the 900px minimum
//! window width (`title_bar_layout_tests.rs` style).
//!
//! Proves that the sidebar TOOLCHAINS section, toolchain header, version rows,
//! PHP settings card, and PATH panel all stay strictly inside the 900px
//! viewport with zero horizontal overflow.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::service::{DblService, mock::MockService};
use gpui_kit::component::Root;
use gpui_kit::{AppContext as _, TestAppContext, px, size};

use super::Workspace;

const MIN_WINDOW_WIDTH: f32 = 900.0;
const WINDOW_HEIGHT: f32 = 820.0;

#[gpui_kit::test]
fn toolchain_php_page_fits_at_900px_minimum_width(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service: Arc<dyn DblService> = Arc::new(MockService::demo());

    let (_view, cx) = cx.add_window_view(move |window, cx| {
        let workspace = cx.new(|cx| Workspace::preview_toolchain_php(service, window, cx));
        Root::new(workspace, window, cx)
    });

    cx.simulate_resize(size(px(MIN_WINDOW_WIDTH), px(WINDOW_HEIGHT)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });

    for selector in [
        "sidebar-toolchains",
        "tc-nav-php",
        "tc-nav-node",
        "tc-nav-composer",
        "toolchain-header",
        "tc-versions-card-php",
        "tc-row-0",
        "tc-row-1",
        "tc-php-settings-card",
        "tc-php-open-folder",
        "tc-php-reset",
        "tc-php-save",
        "tc-php-conf-dir",
        "tc-path-card",
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing debug bounds for {selector}"));
        assert!(
            bounds.right().as_f32() <= MIN_WINDOW_WIDTH + 0.5,
            "{selector} right edge {:.1} exceeds minimum window width {MIN_WINDOW_WIDTH}",
            bounds.right().as_f32()
        );
        assert!(
            bounds.left().as_f32() >= -0.5,
            "{selector} left edge {:.1} is negative",
            bounds.left().as_f32()
        );
    }

    // Save must sit on the same row as Reset to defaults, to its right (G2).
    let reset = cx.debug_bounds("tc-php-reset").expect("tc-php-reset");
    let save = cx.debug_bounds("tc-php-save").expect("tc-php-save");
    assert!(
        (reset.top().as_f32() - save.top().as_f32()).abs() < 1.0,
        "Save (y={:.1}) must sit on the same row as Reset to defaults (y={:.1})",
        save.top().as_f32(),
        reset.top().as_f32()
    );
    assert!(
        save.left().as_f32() >= reset.right().as_f32(),
        "Save (left={:.1}) must be to the right of Reset (right={:.1})",
        save.left().as_f32(),
        reset.right().as_f32()
    );
}

#[gpui_kit::test]
fn toolchain_composer_needs_php_page_fits_at_900px(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service: Arc<dyn DblService> = Arc::new(MockService::demo());

    let (_view, cx) = cx.add_window_view(move |window, cx| {
        let workspace =
            cx.new(|cx| Workspace::preview_toolchain_composer_needs_php(service, window, cx));
        Root::new(workspace, window, cx)
    });

    cx.simulate_resize(size(px(MIN_WINDOW_WIDTH), px(WINDOW_HEIGHT)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });

    for selector in [
        "toolchain-header",
        "tc-composer-needs-php",
        "tc-versions-card-composer",
        "tc-path-card",
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing debug bounds for {selector}"));
        assert!(
            bounds.right().as_f32() <= MIN_WINDOW_WIDTH + 0.5,
            "{selector} right edge {:.1} exceeds {MIN_WINDOW_WIDTH}",
            bounds.right().as_f32()
        );
    }
}
