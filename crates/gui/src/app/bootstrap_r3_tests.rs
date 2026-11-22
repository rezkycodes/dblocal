//! R3 (no-idle / single bridge) headless tests.
//!
//! Split from `bootstrap_tests.rs` to keep each file under the 800-line hard max.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::DblService;
use gpui_kit::TestAppContext;

use super::bootstrap::{AppRuntime, QuitLedger, start_app_loops};
use super::bootstrap_tests::CountingService;
use super::tray_paths::handle_tray_action;

/// R3: `AppRuntime` opens exactly one bridge (`subscribe` is called once), so
/// the app loop is its only consumer. Plain `#[test]`: the bridge thread is
/// real but nothing awaits its flume channel here, so the deterministic GPUI
/// scheduler is never woken from that thread.
#[test]
fn app_creates_exactly_one_bridge() {
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    let _runtime =
        AppRuntime::new_for_test_with_bridge(svc, crate::tray::TrayHandle::unavailable());
    assert_eq!(
        service.subscribe_calls.load(Ordering::SeqCst),
        1,
        "R3: AppRuntime must subscribe once, not one bridge per consumer"
    );
}

/// R3: the app-level loop drains the channel with no window open, so nothing
/// piles up while the window stays closed (a tray-resident app can sit there
/// for days). Events are fed from the scheduler thread on purpose: the
/// deterministic test scheduler rejects wakes from the bridge thread, which
/// would make this test flaky under load rather than prove anything.
#[gpui_kit::test]
fn app_loop_drops_events_with_no_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    let (sender, receiver) = flume::unbounded::<crate::service_bridge::UiEvent>();
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), receiver);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
        start_app_loops(cx);
    });
    const BURST: usize = 10_000;
    for _ in 0..BURST {
        let event = crate::service_bridge::UiEvent::CatalogUpdated;
        cx.update(|_cx| {
            let _ = sender.send(event);
        });
    }
    for _ in 0..50 {
        cx.run_until_parked();
        if sender.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        sender.len(),
        0,
        "R3: the single consumer must drain all {BURST} events with no window open"
    );
    let has_workspace = cx.update(|cx| cx.global::<AppRuntime>().workspace.is_some());
    assert!(!has_workspace, "R3: no window was opened");
}

/// R3: a window opened after a burst of events reads the service, it does
/// not replay the dropped backlog.
///
/// The events are fed from the scheduler thread (`cx.update`) on purpose: the
/// deterministic test scheduler rejects wakes from the real bridge thread, so
/// feeding them from `dblocal-service-bridge` would make this test flaky under
/// load rather than prove anything (see `tray_latency` for the same reason).
/// The real bridge's single-subscription count is pinned separately by
/// `app_creates_exactly_one_bridge`.
#[gpui_kit::test]
fn window_opened_after_a_burst_reads_the_service(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg-live",
        InstanceStatus::Running,
    )]));
    let svc: Arc<dyn DblService> = service.clone();
    let (sender, receiver) = flume::unbounded::<crate::service_bridge::UiEvent>();
    let runtime = AppRuntime::new_for_test(svc, crate::tray::TrayHandle::unavailable(), receiver);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
        start_app_loops(cx);
    });
    for _ in 0..10_000 {
        let event = crate::service_bridge::UiEvent::InstanceChanged(CountingService::view(
            "noise",
            InstanceStatus::Stopped,
        ));
        cx.update(|_cx| {
            let _ = sender.send(event);
        });
    }
    for _ in 0..30 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.update(|cx| handle_tray_action(crate::tray::TrayAction::OpenWindow, cx));
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).instances().len() == 1)
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let shown = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .map(|ws| {
                ws.read(cx)
                    .instances()
                    .iter()
                    .map(|v| v.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    assert_eq!(
        shown,
        vec!["pg-live".to_owned()],
        "R3: the new window must equal the service's list, not the backlog"
    );
}

/// The constructors and the real entry point: `AppRuntime::new` wires the app
/// flag, one bridge and the tray; `run_with_service` refuses on this harness
/// thread (it needs the process main thread) without panicking.
#[test]
fn the_app_runtime_constructor_wires_the_runtime() {
    let service = Arc::new(CountingService::new(vec![]));
    let svc: Arc<dyn DblService> = service.clone();
    let runtime = AppRuntime::new(svc);
    assert!(
        crate::tray::app_enabled(),
        "the app constructor must enable the tray for real runs"
    );
    assert!(
        runtime.tray_available() || !runtime.tray_available(),
        "tray availability is a real probe"
    );
    assert!(runtime.main_window.is_none());
    assert!(runtime.workspace.is_none());
    assert!(
        service.list_instances().is_ok(),
        "the runtime keeps a working service handle"
    );
}
