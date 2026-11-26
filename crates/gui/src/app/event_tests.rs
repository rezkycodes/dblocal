//! Event-handling tests for the workspace shell.
//!
//! Split out of `app.rs` so the entity definition stays under the 800-line cap.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::{DblService, OperationId, UserFacingError};
use gpui_kit::TestAppContext;

use super::Workspace;
use super::test_service::QuietService;
use crate::service_bridge::UiEvent;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<Workspace>,
    QuietService,
    &mut gpui_kit::VisualTestContext,
) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = QuietService::three();
    let svc: Arc<dyn DblService> = Arc::new(service.clone());
    let (view, cx) = cx.add_window_view(|window, cx| Workspace::new_preview(svc, window, cx));
    cx.run_until_parked();
    (view, service, cx)
}

/// `InstanceChanged` upserts the row and refreshes the tray.
#[gpui_kit::test]
fn instance_changed_upserts_the_row(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let mut changed = view.read_with(cx, |this, _| {
        this.instances()
            .iter()
            .find(|v| v.status == InstanceStatus::Stopped)
            .expect("stopped instance")
            .clone()
    });
    changed.status = InstanceStatus::Running;
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::InstanceChanged(changed.clone()), cx)
    });
    cx.run_until_parked();
    let status = view.read_with(cx, |this, _| this.instance(&changed.id).map(|v| v.status));
    assert_eq!(status, Some(InstanceStatus::Running));
    assert!(
        service.calls() > 0,
        "the tray refresh must re-read instances"
    );
}

/// `InstanceRemoved` drops the row, its tab, its logs and every cache, then
/// asks the service to stop watching it.
#[gpui_kit::test]
fn instance_removed_drops_tab_logs_and_caches(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();

    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::InstanceRemoved(id.clone()), cx)
    });
    cx.run_until_parked();

    let (row, tabs, logs) = view.read_with(cx, |this, _| {
        (
            this.instance(&id).is_some(),
            this.tabs.len(),
            this.log_lines(&id).is_empty(),
        )
    });
    assert!(!row, "the removed row is still in the dock");
    assert_eq!(tabs, 0, "the removed instance kept its tab");
    assert!(logs, "the removed instance kept its log buffer");
}

/// `StopForced` raises the forced-stop toast.
#[gpui_kit::test]
fn stop_forced_raises_the_toast(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Running);
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::StopForced(id.clone()), cx)
    });
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(dblocal_core::strings::UI_TOAST_STOP_FORCED.to_owned())
    );
}

/// A failed operation lands as `<DBL code> — <message>` and is mirrored for
/// the failure panel.
#[gpui_kit::test]
fn a_failed_operation_shows_the_code_and_message(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    let err = dblocal_core::DblError::InvalidInput {
        field: "port",
        reason: "invalid value".to_owned(),
    };
    let facing = UserFacingError::from(&err);
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::OperationFinished {
                id: OperationId::new("op-fail"),
                result: Err(facing.clone()),
            },
            cx,
        )
    });
    cx.run_until_parked();
    let toast = view
        .read_with(cx, |this, _| this.toast_text())
        .map(|t| t.to_string())
        .unwrap_or_default();
    assert!(
        toast.contains(&facing.code),
        "toast lacks the code: {toast}"
    );
    assert!(
        toast.contains(&facing.message),
        "toast lacks the message: {toast}"
    );
}

/// `close_tab_by_instance` is what a delete calls; it withdraws the tab.
#[gpui_kit::test]
fn close_tab_by_instance_withdraws_the_tab(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.close_tab_by_instance(&id, cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |this, _| this.tabs.len()), 0);
}

/// The toast setter, the focus handle and the count accessors are what the
/// element tree reads every frame.
#[gpui_kit::test]
fn small_accessors_answer_from_state(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, _| {
        this.set_toast(gpui_kit::SharedString::from("halo"));
    });
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .map(|t| t.to_string()),
        Some("halo".to_owned())
    );
    let counts = view.read_with(cx, |this, _| this.counts());
    assert_eq!(
        (counts.running, counts.stopped, counts.failed),
        (1, 1, 1),
        "the counts must see the seeded instances"
    );
    assert_eq!(
        view.read_with(cx, |this, _| this.version()),
        env!("CARGO_PKG_VERSION")
    );
    view.update(cx, |this, _| this.close_modals());
    assert!(view.read_with(cx, |this, _| this.create_form().is_none()));
    let _ = view.read_with(cx, |this, _| this.focus_handle.clone());
    let _ = view.read_with(cx, |this, _| this.command_state().clone());
}
