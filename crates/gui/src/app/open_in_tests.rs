//! "Open in…" menu: handler tests (`F-OPN-01..02`).
//!
//! Detection and launch run over the scriptable service; the menu is the pure
//! model, tested in `open_in.rs`. Here the tests prove the handlers fill the
//! cache, fall back to copying the URI when nothing is detected, and report a
//! typed launch error.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::clients::{ClientError, ClientKind};
use dblocal_core::model::EngineKey;
use dblocal_core::service::{DblService, UserFacingError};
use gpui_kit::TestAppContext;

use crate::app::Workspace;
use crate::app::scripted_service::ScriptedService;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<Workspace>,
    ScriptedService,
    &mut gpui_kit::VisualTestContext,
) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = ScriptedService::new();
    let svc: Arc<dyn DblService> = Arc::new(service.clone());
    let (view, cx) = cx.add_window_view(|window, cx| Workspace::new_preview(svc, window, cx));
    cx.run_until_parked();
    (view, service, cx)
}

fn settle(cx: &mut gpui_kit::VisualTestContext) {
    for _ in 0..120 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cx.run_until_parked();
}

#[gpui_kit::test]
fn opening_an_instance_loads_the_client_menu_once(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(dblocal_core::model::InstanceStatus::Running);
    view.update(cx, |this, cx| this.open_tab(id, cx));
    settle(cx);
    // A second instance open must reuse the cache, not detect again.
    let second = service.id_with(dblocal_core::model::InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(second, cx));
    settle(cx);
    assert_eq!(
        service.counts().detect_clients,
        1,
        "detection is cached across instances"
    );
    let menu = view.read_with(cx, |this, _| this.open_in_menu(EngineKey::Postgresql));
    assert!(!menu.copy_uri_fallback, "clients were detected");
    assert!(menu.has_launchable());
}

#[gpui_kit::test]
fn no_detected_clients_falls_back_to_copy_uri(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.clients = Ok(Vec::new()));
    let id = service.id_with(dblocal_core::model::InstanceStatus::Running);
    view.update(cx, |this, cx| this.open_tab(id, cx));
    settle(cx);
    let menu = view.read_with(cx, |this, _| this.open_in_menu(EngineKey::Postgresql));
    assert!(
        menu.copy_uri_fallback,
        "nothing detected must offer Salin URI, not an empty list (F-OPN-01 AC2)"
    );
    assert!(!menu.has_launchable());
}

#[gpui_kit::test]
fn refresh_re_runs_detection(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(dblocal_core::model::InstanceStatus::Running);
    view.update(cx, |this, cx| this.open_tab(id, cx));
    settle(cx);
    assert_eq!(service.counts().detect_clients, 1);
    view.update(cx, |this, cx| this.detect_clients_refresh(cx));
    settle(cx);
    assert_eq!(
        service.counts().detect_clients,
        2,
        "the refresh item re-runs detection"
    );
}

#[gpui_kit::test]
fn launching_a_client_calls_open_in_and_confirms(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(dblocal_core::model::InstanceStatus::Running);
    view.update(cx, |this, cx| {
        this.open_instance_in(id.clone(), ClientKind::Psql, cx)
    });
    settle(cx);
    assert_eq!(service.counts().open_in_client, 1);
    let toast = view.read_with(cx, |this, _| this.toast_text().map(|t| t.to_string()));
    assert!(
        toast.as_deref().is_some_and(|t| t.contains("psql")),
        "a launch must confirm with the client name: {toast:?}"
    );
    assert!(
        view.read_with(cx, |this, _| this.opening_instance().is_none()),
        "the busy flag clears after the launch"
    );
}

#[gpui_kit::test]
fn a_launch_failure_is_a_typed_message(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.launch = Err(UserFacingError::new(
            ClientError::LaunchFailed {
                reason: "spawn".to_owned(),
            }
            .code(),
            "Klien gagal dibuka.",
        ));
    });
    let id = service.id_with(dblocal_core::model::InstanceStatus::Running);
    view.update(cx, |this, cx| {
        this.open_instance_in(id, ClientKind::TablePlus, cx)
    });
    settle(cx);
    let toast = view.read_with(cx, |this, _| this.toast_text().map(|t| t.to_string()));
    assert!(
        toast.as_deref().is_some_and(|t| t.contains("DBL-")),
        "a launch failure is a typed message, not a silent no-op: {toast:?}"
    );
}
