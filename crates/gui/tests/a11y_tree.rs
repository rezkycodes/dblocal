//! Accessibility-tree regression tests for every overlay we own (`F-PLH-01`).
//!
//! A debug build of the real GUI warned, then aborted, with
//! `Duplicate a11y node id: #4375755302221953578` on the first frame it built
//! the tree: two siblings hashed to the same `GlobalElementId`. A release
//! build drops the second node silently, so a screen reader loses it.
//!
//! GPUI only builds the tree while an assistive technology is attached, which
//! is why no other test saw it: the stock test platform never calls the
//! activation callback. `support/a11y_platform.rs` wraps the headless platform
//! and activates the tree, so these tests render real frames with it built and
//! fail on any duplicate — the same assertion gpui makes in a debug build.
//!
//! Run: `cargo test -p dblocal-gui --test a11y_tree`

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "support/a11y_platform.rs"]
mod a11y_platform;

use std::collections::BTreeMap;
use std::sync::Arc;

use dblocal_core::service::{DblService, mock::MockService};
use dblocal_gui::app::Workspace;
use gpui_kit::accesskit::TreeUpdate;
use gpui_kit::{App, AppContext as _, Application, Context, Window, WindowOptions, px, size};

use a11y_platform::A11yPlatform;

type Build = fn(Arc<dyn DblService>, &mut Window, &mut Context<Workspace>) -> Workspace;

/// Every overlay/screen we own, as a `Workspace` constructor.
const CASES: &[(&str, Build)] = &[
    ("shell", Workspace::preview),
    ("quit dialog", Workspace::preview_quit),
    ("quit stop failed", Workspace::preview_quit_stop_failed),
    ("new instance dialog", Workspace::preview_create),
    ("create unavailable", Workspace::preview_create_unavailable),
    ("delete confirmation", Workspace::preview_delete),
    ("command palette", Workspace::preview_palette),
    ("status bar downloads", Workspace::preview_pending),
    ("logs", Workspace::preview_logs),
    ("logs long", Workspace::preview_logs_long),
    ("failure", Workspace::preview_failure),
    ("failed overview", Workspace::preview_failed_overview),
    ("databases", Workspace::preview_databases),
    ("databases product", Workspace::preview_product_databases),
    ("config product", Workspace::preview_product_config),
    ("filtered dock", Workspace::preview_filtered),
    ("stale banner", Workspace::preview_stale_banner),
    ("revealed uris", Workspace::preview_revealed),
    ("no tray", Workspace::preview_no_tray),
];

/// The colliding `accesskit` node ids in one tree, sorted for a stable message.
fn duplicates(tree: &TreeUpdate) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (id, _) in &tree.nodes {
        *counts.entry(format!("{id:?}")).or_insert(0) += 1;
    }
    let mut dups: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .collect();
    dups.sort();
    dups
}

/// Builds one `Workspace` case in a window whose a11y tree is active, renders
/// a frame, and returns the duplicate node ids the tree reported.
fn duplicates_for(name: &str, build: Build) -> (usize, Vec<(String, usize)>) {
    let platform = A11yPlatform::new();
    let trees = platform.clone();
    let build: Build = build;
    let app = Application::with_platform(platform as _).run_embedded(move |cx: &mut App| {
        gpui_kit::init(cx);
        dblocal_gui::theme::init(cx);
        let service: Arc<dyn DblService> = Arc::new(MockService::demo());
        let bounds = gpui_kit::Bounds {
            origin: gpui_kit::point(px(0.0), px(0.0)),
            size: size(px(1280.0), px(820.0)),
        };
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(gpui_kit::WindowBounds::Windowed(bounds)),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                |window, cx| {
                    let workspace = cx.new(|cx| build(service, window, cx));
                    cx.new(|cx| gpui_kit::component::Root::new(workspace, window, cx))
                },
            )
            .expect("open the a11y window");
        // Two frames: the first builds the tree once a11y is active, the second
        // proves the tree stays consistent across a redraw.
        for _ in 0..2 {
            handle
                .update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                })
                .expect("draw a frame with a11y active");
        }
    });

    let captured = trees.trees();
    assert!(
        !captured.is_empty(),
        "{name}: the a11y tree was never built — the harness did not activate it"
    );
    // Every frame's tree must be clean, not just the last.
    let mut worst: Vec<(String, usize)> = Vec::new();
    for tree in &captured {
        let dups = duplicates(tree);
        if dups.len() > worst.len() {
            worst = dups;
        }
    }
    drop(app);
    (captured.len(), worst)
}

#[test]
fn every_workspace_screen_and_overlay_has_a_unique_a11y_tree() {
    let mut failures: Vec<String> = Vec::new();
    for (name, build) in CASES {
        let (trees, dups) = duplicates_for(name, *build);
        if !dups.is_empty() {
            let detail: Vec<String> = dups
                .iter()
                .map(|(id, n)| format!("{id} x{n}"))
                .collect();
            failures.push(format!(
                "{name}: {} duplicate a11y node id(s) across {trees} tree(s): {}",
                dups.len(),
                detail.join(", ")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "duplicate a11y node ids (a release build would silently drop these nodes):\n{}",
        failures.join("\n")
    );
}
