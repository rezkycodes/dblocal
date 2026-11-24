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
//!
//! The harness wraps `gpui_platform::current_platform(true)`, i.e. the *headless*
//! platform. The Linux headless backend implements `open_window` (it has a
//! `HeadlessClient`), and macOS creates real `NSWindow`s, but the Windows one
//! refuses it ("cannot open windows while headless": a headless `WindowsPlatform`
//! never attaches the DirectX devices that `is_windowed()` checks) and its
//! `Platform::run` pumps `GetMessageW` forever, so `run_embedded` never returns.
//! GPUI's own cross-platform test platform (`TestAppContext`) does not build the
//! real a11y tree, so there is no way to reach the `Duplicate a11y node id`
//! assertion on Windows. The support module still compiles there (and is checked
//! by `cargo clippy --all-targets`); each test skips with a stated reason.

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

/// The colliding `accesskit` node ids in one tree, sorted for a stable message.
fn duplicates(tree: &TreeUpdate) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (id, _) in &tree.nodes {
        *counts.entry(format!("{id:?}")).or_insert(0) += 1;
    }
    let mut dups: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n > 1).collect();
    dups.sort();
    dups
}

/// Builds one `Workspace` case in a window whose a11y tree is active, renders
/// a frame, and returns the duplicate node ids the tree reported.
///
/// Returns `None` on Windows, where the headless platform cannot open a window,
/// so the caller skips instead of hanging or failing on a platform limit.
fn duplicates_for(name: &str, build: Build) -> Option<(usize, Vec<(String, usize)>)> {
    if cfg!(target_os = "windows") {
        eprintln!(
            "{name}: skipped — a headless GPUI platform cannot open a window on Windows, \
             so the a11y tree cannot be built there"
        );
        return None;
    }
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
        // proves the tree stays consistent across a redraw. Draw through the app
        // (`AppContext::update_window`), not `WindowHandle::update`, which would
        // borrow the Root entity and re-enter it.
        for _ in 0..2 {
            cx.update_window(handle.into(), |_, window, cx| {
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
    Some((captured.len(), worst))
}

/// One `#[test]` per case: gpui's `debug_assert` panics inside the frame, so a
/// single all-cases test would stop at the first collision. Split, libtest
/// reports every colliding screen in one run.
macro_rules! case_tests {
    ($( $fn_name:ident => $name:literal, $build:path; )*) => {
        $(
            #[test]
            fn $fn_name() {
                let Some((trees, dups)) = duplicates_for($name, $build) else {
                    return;
                };
                let detail: Vec<String> = dups
                    .iter()
                    .map(|(id, n)| format!("{id} x{n}"))
                    .collect();
                assert!(
                    dups.is_empty(),
                    "{}: {} duplicate a11y node id(s) across {trees} tree(s): {}",
                    $name,
                    dups.len(),
                    detail.join(", ")
                );
            }
        )*
    };
}

case_tests! {
    shell_is_unique => "shell", Workspace::preview;
    quit_dialog_is_unique => "quit dialog", Workspace::preview_quit;
    quit_stop_failed_is_unique => "quit stop failed", Workspace::preview_quit_stop_failed;
    new_instance_dialog_is_unique => "new instance dialog", Workspace::preview_create;
    create_unavailable_is_unique => "create unavailable", Workspace::preview_create_unavailable;
    delete_confirmation_is_unique => "delete confirmation", Workspace::preview_delete;
    command_palette_is_unique => "command palette", Workspace::preview_palette;
    status_bar_downloads_is_unique => "status bar downloads", Workspace::preview_pending;
    logs_are_unique => "logs", Workspace::preview_logs;
    logs_long_is_unique => "logs long", Workspace::preview_logs_long;
    failure_is_unique => "failure", Workspace::preview_failure;
    failed_overview_is_unique => "failed overview", Workspace::preview_failed_overview;
    databases_is_unique => "databases", Workspace::preview_databases;
    databases_product_is_unique => "databases product", Workspace::preview_product_databases;
    databases_loaded_is_unique => "databases loaded", Workspace::preview_databases_loaded;
    drop_dialog_is_unique => "drop dialog", Workspace::preview_drop_dialog;
    reset_dialog_is_unique => "reset dialog", Workspace::preview_reset_dialog;
    secret_dialog_is_unique => "secret dialog", Workspace::preview_secret_dialog;
    settings_dialog_is_unique => "settings dialog", Workspace::preview_settings;
    same_database_name_two_instances_is_unique => "two instances, same database name", Workspace::preview_two_instances_same_db;
    config_product_is_unique => "config product", Workspace::preview_product_config;
    config_demo_is_unique => "config demo", Workspace::preview_config_demo;
    two_configs_are_unique => "two configs", Workspace::preview_two_configs;
    filtered_dock_is_unique => "filtered dock", Workspace::preview_filtered;
    stale_banner_is_unique => "stale banner", Workspace::preview_stale_banner;
    revealed_uris_is_unique => "revealed uris", Workspace::preview_revealed;
    no_tray_is_unique => "no tray", Workspace::preview_no_tray;
}
