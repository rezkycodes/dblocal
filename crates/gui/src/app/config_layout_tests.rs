//! Render-level Config tests: the fixed grid (`F3`), "Restart sekarang" only
//! for a running instance (`M8`) and the Overview restore banner (`M12`).
//!
//! These read element bounds through `VisualTestContext::debug_bounds` (the
//! evidence `F3` asks for: never an image). A selector is recorded only during
//! a real paint, so every assertion here draws a frame first.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::model::{InstanceId, InstanceStatus, StartFailure};
use dblocal_core::service::{DblService, FailureInfo, mock::MockService};
use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext, px, size};

use super::config_state::{config_action_id, config_row_id};
use super::InnerTab;
use super::Workspace;

/// A workspace on the demo service, showing one instance's Config tab.
fn config_window<'a>(
    cx: &'a mut TestAppContext,
) -> (gpui_kit::Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service: Arc<dyn DblService> = Arc::new(MockService::demo());
    let (view, mut cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::preview_product_config(service, window, cx);
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Config);
        workspace
    });
    // The prototype's default window; the minimum is 900x600.
    cx.simulate_resize(size(px(1280.), px(820.)));
    settle(cx);
    (view, cx)
}

/// Draws one frame and returns nothing; `debug_bounds` reads the last frame.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The bounds one debug selector recorded in the last frame.
///
/// `debug_bounds` takes a `&'static str`; the selectors carry the runtime
/// instance id, so the test leaks its own labels (a bounded, tiny allocation)
/// instead of widening a product API for a test.
fn bounds_of(cx: &mut VisualTestContext, selector: &str) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(selector)
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    draw(cx);
    cx.run_until_parked();
}

/// The demo Config view's instance id (the selected tab).
fn selected(view: &gpui_kit::Entity<Workspace>, cx: &VisualTestContext) -> InstanceId {
    view.read_with(cx, |this, _| {
        this.tabs
            .selected_instance()
            .cloned()
            .expect("a selected instance")
    })
}

/// `F3`: at 1280x820 every field cell of the card shares one x and one width,
/// and the rightmost element of every row stays inside the card's right edge.
#[gpui_kit::test]
fn every_config_field_shares_one_x_and_stays_inside_the_card(cx: &mut TestAppContext) {
    let (view, mut cx) = config_window(cx);
    let id = selected(&view, &cx);
    draw(cx);
    let card = bounds_of(cx, &config_action_id("config-card", &id)).expect("the card records its bounds");
    let keys = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .map(|config| {
                config
                    .view()
                    .map(|view| {
                        view.settings
                            .iter()
                            .map(|s| s.key.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    });
    assert!(!keys.is_empty(), "F3: the card drew rows");

    let mut xs: Vec<f32> = Vec::new();
    for key in &keys {
        let selector = config_row_id("config-field-cell", &id, key);
        let bounds = bounds_of(cx, &selector)
            .unwrap_or_else(|| panic!("F3: no bounds for {key}"));
        xs.push(bounds.origin.x.as_f32());
        // Nothing in the row may cross the card's right edge.
        let side = bounds_of(cx, &config_row_id("config-side-cell", &id, key))
            .unwrap_or_else(|| panic!("F3: no side-cell bounds for {key}"));
        let right = side.origin.x.as_f32() + side.size.width.as_f32();
        let card_right = card.origin.x.as_f32() + card.size.width.as_f32();
        assert!(
            right <= card_right + 1.0 && bounds.origin.x.as_f32() + bounds.size.width.as_f32() <= card_right + 1.0,
            "F3: row {key} runs past the card border (right {right} > card {card_right})"
        );
    }
    let first = xs[0];
    for (key, x) in keys.iter().zip(&xs) {
        assert!(
            (*x - first).abs() < 1.0,
            "F3: {key} starts its field at x={x}, not the shared {first}"
        );
    }
}

/// `F3`: the same holds at the minimum 900x600 window, where the old layout was
/// far worse.
#[gpui_kit::test]
fn the_grid_holds_at_the_minimum_window_size(cx: &mut TestAppContext) {
    let (view, mut cx) = config_window(cx);
    cx.simulate_resize(size(px(900.), px(600.)));
    settle(cx);
    let id = selected(&view, &cx);
    draw(cx);
    let card = bounds_of(cx, &config_action_id("config-card", &id)).expect("the card records its bounds");
    let card_right = card.origin.x.as_f32() + card.size.width.as_f32();
    let keys = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|config| {
                config.view().map(|view| {
                    view.settings
                        .iter()
                        .map(|s| s.key.clone())
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default()
    });
    for key in &keys {
        let side = bounds_of(cx, &config_row_id("config-side-cell", &id, key))
            .unwrap_or_else(|| panic!("F3: no side-cell bounds for {key} at 900 wide"));
        let right = side.origin.x.as_f32() + side.size.width.as_f32();
        assert!(
            right <= card_right + 1.0,
            "F3: {key} overflows at 900 wide ({right} > {card_right})"
        );
    }
}

/// `M8`: "Restart sekarang" exists only for a running instance; a stopped one
/// draws "Nanti" alone.
#[gpui_kit::test]
fn restart_now_is_drawn_only_for_a_running_instance(cx: &mut TestAppContext) {
    let (view, mut cx) = config_window(cx);
    let running = selected(&view, &cx);
    // Raise the banner with a successful apply while the instance runs.
    view.update(cx, |this, cx| {
        this.config.entry(&running).applied = Some(applied());
        cx.notify();
    });
    settle(cx);
    let restart_sel = config_action_id("config-restart", &running);
    assert!(
        bounds_of(cx, &restart_sel).is_some(),
        "M8: a running instance draws 'Restart sekarang'"
    );

    // A stopped instance keeps the banner (Nanti) but no restart button.
    let stopped = view.read_with(cx, |this, _| {
        this.state
            .instances
            .iter()
            .find(|v| v.status == InstanceStatus::Stopped)
            .map(|v| v.id.clone())
    });
    let Some(stopped) = stopped else {
        return;
    };
    view.update(cx, |this, cx| {
        this.tabs.open_or_select(stopped.clone());
        this.tabs
            .set_inner(this.tabs.selected_index(), InnerTab::Config);
        this.config.entry(&stopped).applied = Some(applied());
        this.load_config_sync(cx);
        cx.notify();
    });
    settle(cx);
    let stopped_restart = config_action_id("config-restart", &stopped);
    assert!(
        bounds_of(cx, &stopped_restart).is_none(),
        "M8: a stopped instance must not draw 'Restart sekarang'"
    );
    assert!(
        bounds_of(cx, &config_action_id("config-later", &stopped)).is_some(),
        "M8: the banner is still there, with 'Nanti'"
    );
}

/// `M12`: the Overview of a Failed instance with `restore_available` draws
/// "Pulihkan konfigurasi dan mulai ulang", and clicking it calls `config_restore`.
#[gpui_kit::test]
fn the_failed_overview_draws_the_restore_action_and_it_restores(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = super::scripted_service::ScriptedService::new();
    let svc: Arc<dyn DblService> = Arc::new(service.clone());
    let (_view, mut cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_preview(svc, window, cx);
        let id = service.id_with(InstanceStatus::Running);
        let mut failed = workspace.instance(&id).cloned().expect("instance view");
        failed.status = InstanceStatus::Failed;
        failed.failure = Some(FailureInfo {
            code: "DBL-4001".to_owned(),
            message: "mesin gagal start".to_owned(),
            diagnosis: StartFailure::Unknown,
            log_tail: Vec::new(),
            restore_available: true,
        });
        workspace.state.upsert_instance(failed);
        workspace.open_tab_sync(id);
        workspace
            .tabs
            .set_inner(workspace.tabs.selected_index(), InnerTab::Overview);
        workspace
    });
    settle(cx);
    let selector = "overview-restore-config";
    let bounds = bounds_of(cx, selector).expect("M12: the Overview draws the restore action");
    // Click it: the action must restore the configuration.
    let center = gpui_kit::point(
        bounds.origin.x + bounds.size.width * 0.5,
        bounds.origin.y + bounds.size.height * 0.5,
    );
    cx.simulate_click(center, gpui_kit::Modifiers::none());
    settle(cx);
    assert_eq!(
        service.mutations().config_restores,
        vec![service.id_with(InstanceStatus::Running)],
        "M12: clicking the banner restores the configuration"
    );
}

/// A successful apply result, for the banner's `applied` flag.
fn applied() -> dblocal_core::config::ConfigApplied {
    dblocal_core::config::ConfigApplied {
        restart_required: true,
        backup: dblocal_core::config::BackupInfo {
            taken_at: 1_700_000_000,
            taken_at_secs: 1_700_000_000,
            path: std::path::PathBuf::from("/mock/backups/postgresql.conf.1700000000.bak"),
        },
        changed: 1,
    }
}

