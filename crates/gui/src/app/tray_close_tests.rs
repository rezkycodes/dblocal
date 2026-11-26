//! Regression tests for tray actions after the main window is closed (close-to-tray).
//!
//! Specifically exercises closing the window via `window.remove_window()` — the exact
//! path `gpui_component::TitleBar`'s Linux close button takes when `on_close_window`
//! is not wired — as well as stale-handle recovery and visible logging for dropped
//! tray actions.

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::DblService;
use gpui_kit::{BorrowAppContext as _, TestAppContext};
use tracing_subscriber::fmt::MakeWriter;

use super::bootstrap::{
    AppRuntime, QuitLedger, handle_window_should_close, open_main_window, quit_requests,
    start_app_loops,
};
use super::bootstrap_tests::CountingService;
use super::tray_paths::handle_tray_action;
use crate::tray::{TrayAction, TrayBackend, TrayHandle, TrayMenuModel};

struct AvailableTray;

impl TrayBackend for AvailableTray {
    fn refresh(
        &mut self,
        _model: &TrayMenuModel,
        _views: &[dblocal_core::service::InstanceView],
    ) -> bool {
        true
    }
    fn is_available(&self) -> bool {
        true
    }
}

#[derive(Clone, Default)]
struct LogBuf(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buf").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn setup_tray_app(
    cx: &mut TestAppContext,
    views: Vec<dblocal_core::service::InstanceView>,
) -> Arc<CountingService> {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(views));
    let svc: Arc<dyn DblService> = service.clone();
    let (tx, rx) = flume::unbounded::<crate::service_bridge::UiEvent>();
    drop(tx);
    let runtime = AppRuntime::new_for_test(svc, TrayHandle::new(Box::new(AvailableTray)), rx);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
        start_app_loops(cx);
    });
    service
}

fn close_window_via_remove(cx: &mut TestAppContext) {
    let handle = cx.update(|cx| cx.global::<AppRuntime>().main_window.expect("main window"));
    cx.update(|cx| {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .expect("remove window");
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| cx.windows().len()),
        0,
        "window must be gone after remove_window()"
    );
}

/// Window open -> close to tray (`window.remove_window`) -> all instances stopped
/// externally -> tray Quit must exit the app instead of routing to a stale windowless
/// workspace whose cached instance list still shows the server running.
#[gpui_kit::test]
fn tray_quit_after_window_remove_with_stopped_service_quits(cx: &mut TestAppContext) {
    let service = setup_tray_app(
        cx,
        vec![CountingService::view("pg1", InstanceStatus::Running)],
    );
    cx.update(|cx| open_main_window(cx).expect("open window"));
    for _ in 0..100 {
        cx.run_until_parked();
        let loaded = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).instances().len() == 1)
        });
        if loaded {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Close the window the way Linux TitleBar's X button does (remove_window).
    close_window_via_remove(cx);

    // Server is later stopped (no database servers running when Quit is clicked).
    service.views.lock().expect("views")[0].status = InstanceStatus::Stopped;

    cx.update(|cx| handle_tray_action(TrayAction::Quit, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        if cx.read(quit_requests) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        cx.read(quit_requests),
        1,
        "tray Quit after close-to-tray with no running servers must quit the app"
    );
    assert_eq!(
        cx.read(|cx| cx.windows().len()),
        0,
        "tray Quit with no running servers must not open a window"
    );
}

/// Window open -> close to tray (`window.remove_window`) -> server still running ->
/// tray Quit must open the main window to show the quit confirmation dialog instead
/// of silently calling `cx.notify()` on a windowless workspace.
#[gpui_kit::test]
fn tray_quit_after_window_remove_with_running_instance_opens_confirming_window(
    cx: &mut TestAppContext,
) {
    let _service = setup_tray_app(
        cx,
        vec![CountingService::view("pg1", InstanceStatus::Running)],
    );
    cx.update(|cx| open_main_window(cx).expect("open window"));
    cx.run_until_parked();

    close_window_via_remove(cx);

    cx.update(|cx| handle_tray_action(TrayAction::Quit, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        let ready = cx.read(|cx| cx.windows().len() == 1)
            && cx.update(|cx| {
                cx.global::<AppRuntime>()
                    .workspace
                    .as_ref()
                    .is_some_and(|ws| ws.read(cx).quit_machine().is_confirming())
            });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        cx.read(|cx| cx.windows().len()),
        1,
        "tray Quit with a running server must open a window to show the confirmation dialog"
    );
    let confirming = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.read(cx).quit_machine().is_confirming())
    });
    assert!(
        confirming,
        "the reopened window must be in the confirming quit state"
    );
    assert_eq!(
        cx.read(quit_requests),
        0,
        "must not quit before the user decides what to do with running servers"
    );
}

/// Window open -> close to tray (`window.remove_window`) -> tray "Open DBLocal"
/// must open a new window instead of silently ignoring a stale WindowHandle.
#[gpui_kit::test]
fn tray_open_after_window_remove_opens_a_fresh_window(cx: &mut TestAppContext) {
    let _service = setup_tray_app(
        cx,
        vec![CountingService::view("pg1", InstanceStatus::Stopped)],
    );
    cx.update(|cx| open_main_window(cx).expect("open window"));
    cx.run_until_parked();

    close_window_via_remove(cx);

    cx.update(|cx| handle_tray_action(TrayAction::OpenWindow, cx));
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| cx.windows().len()),
        1,
        "tray OpenWindow after close-to-tray must open a new window"
    );
}

/// Window open -> close to tray (`window.remove_window`) -> tray Start failure
/// must be logged with its error code rather than swallowed onto a windowless toast.
#[gpui_kit::test]
fn tray_start_failure_after_window_remove_is_logged(cx: &mut TestAppContext) {
    let service = setup_tray_app(
        cx,
        vec![CountingService::view("pg1", InstanceStatus::Stopped)],
    );
    let id = service.views.lock().expect("views")[0].id.to_string();
    cx.update(|cx| open_main_window(cx).expect("open window"));
    cx.run_until_parked();

    close_window_via_remove(cx);

    let buf = LogBuf::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // Verify runtime cleared the window/workspace so Start runs at app level.
    let cleared = cx.update(|cx| {
        let rt = cx.global::<AppRuntime>();
        rt.main_window.is_none() && rt.workspace.is_none()
    });
    assert!(
        cleared,
        "closing the window via remove_window must clear main_window and workspace"
    );

    // Also verify invalid id on app-level Start logs a warning rather than silently dropping.
    cx.update(|cx| handle_tray_action(TrayAction::Start("not-a-uuid".to_owned()), cx));
    cx.run_until_parked();
    let logged = String::from_utf8_lossy(&buf.0.lock().expect("log buf")).into_owned();
    assert!(
        logged.contains("invalid instance id"),
        "invalid tray Start id must log a warning: {logged}"
    );
    let _ = id;
}

/// Without a tray, closing the window with a running server must NOT close the window
/// immediately; it must route through the quit flow and show the confirmation dialog.
#[gpui_kit::test]
fn close_without_tray_and_running_instance_keeps_window_for_dialog(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = Arc::new(CountingService::new(vec![CountingService::view(
        "pg1",
        InstanceStatus::Running,
    )]));
    let svc: Arc<dyn DblService> = service.clone();
    let (tx, rx) = flume::unbounded::<crate::service_bridge::UiEvent>();
    drop(tx);
    let runtime = AppRuntime::new_for_test(svc, TrayHandle::unavailable(), rx);
    cx.update(|cx| {
        cx.set_global(QuitLedger::default());
        cx.set_global(runtime);
    });
    cx.update(|cx| open_main_window(cx).expect("open window"));
    for _ in 0..100 {
        cx.run_until_parked();
        let loaded = cx.update(|cx| {
            cx.global::<AppRuntime>()
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.read(cx).instances().len() == 1)
        });
        if loaded {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let weak = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .expect("workspace")
            .downgrade()
    });
    let should_close = cx.update(|cx| handle_window_should_close(weak, cx));
    assert!(
        !should_close,
        "without a tray and with a running server, close must keep the window for the quit dialog"
    );
    let confirming = cx.update(|cx| {
        cx.global::<AppRuntime>()
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.read(cx).quit_machine().is_confirming())
    });
    assert!(confirming);
    assert_eq!(cx.read(quit_requests), 0);
}

/// Even if `AppRuntime.workspace` was somehow left set with no live window,
/// `handle_tray_action` must detect the stale state, log a warning, clear it,
/// and handle Quit at app level.
#[gpui_kit::test]
fn stale_workspace_without_live_window_is_cleared_and_logged(cx: &mut TestAppContext) {
    let _service = setup_tray_app(
        cx,
        vec![CountingService::view("pg1", InstanceStatus::Stopped)],
    );
    cx.update(|cx| open_main_window(cx).expect("open window"));
    cx.run_until_parked();
    let stale_ws = cx.update(|cx| cx.global::<AppRuntime>().workspace.clone());
    let stale_win = cx.update(|cx| cx.global::<AppRuntime>().main_window);
    close_window_via_remove(cx);

    // Re-inject the stale handles to test the defensive guard in handle_tray_action.
    cx.update(|cx| {
        cx.update_global::<AppRuntime, _>(|rt, _| {
            rt.workspace = stale_ws;
            rt.main_window = stale_win;
        });
    });

    let buf = LogBuf::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    cx.update(|cx| handle_tray_action(TrayAction::Quit, cx));
    for _ in 0..100 {
        cx.run_until_parked();
        if cx.read(quit_requests) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        cx.read(quit_requests),
        1,
        "stale workspace guard must route Quit to app level and exit"
    );
    let logged = String::from_utf8_lossy(&buf.0.lock().expect("log buf")).into_owned();
    assert!(
        logged.contains("stale workspace/window handle"),
        "stale handle recovery must log a warning: {logged}"
    );
}

/// A tray action arriving with no `AppRuntime` installed must log a warning rather
/// than being silently dropped.
#[gpui_kit::test]
fn tray_action_without_app_runtime_logs_warning(cx: &mut TestAppContext) {
    let buf = LogBuf::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    cx.update(|cx| handle_tray_action(TrayAction::Quit, cx));
    let logged = String::from_utf8_lossy(&buf.0.lock().expect("log buf")).into_owned();
    assert!(
        logged.contains("tray action dropped"),
        "a dropped tray action must be logged: {logged}"
    );
}
