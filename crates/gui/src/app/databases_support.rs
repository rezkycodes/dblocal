//! Shared harness for the Databases-tab tests (`F-DBU-01..03`).
//!
//! Split out so `databases_tests.rs` and `admin_fix_tests.rs` both stay under the
//! 800-line cap: the fixture, the settle loop and the typers are the same in
//! both, and a test must drive the handlers exactly as a click does.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::DblService;
use gpui_kit::TestAppContext;

use crate::app::scripted_service::ScriptedService;
use crate::app::{InnerTab, Workspace};

/// A workspace on the scriptable service, parked after its first read.
pub(crate) fn fixture(
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

/// Drives the tab to a settled read: open the instance, select Databases, let
/// the background read land.
pub(crate) fn open_databases(
    view: &gpui_kit::Entity<Workspace>,
    service: &ScriptedService,
    cx: &mut gpui_kit::VisualTestContext,
) -> dblocal_core::model::InstanceId {
    let id = service.id_with(InstanceStatus::Running);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Databases, cx));
    settle(cx);
    id
}

/// Yields until every background task the view spawned has run.
pub(crate) fn settle(cx: &mut gpui_kit::VisualTestContext) {
    for _ in 0..200 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    cx.run_until_parked();
}

/// Types into the database-name field through the real editor state.
pub(crate) fn set_db_name(
    view: &gpui_kit::Entity<Workspace>,
    value: &str,
    cx: &mut gpui_kit::VisualTestContext,
) {
    cx.update(|window, app| {
        view.update(app, |this, cx| this.set_db_name_for_test(value, window, cx));
    });
}

/// Types into the new-user name field.
pub(crate) fn set_user_name(
    view: &gpui_kit::Entity<Workspace>,
    value: &str,
    cx: &mut gpui_kit::VisualTestContext,
) {
    cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.set_user_name_for_test(value, window, cx)
        });
    });
}

/// Types into the drop dialog's confirmation field.
pub(crate) fn set_drop_text(
    view: &gpui_kit::Entity<Workspace>,
    value: &str,
    cx: &mut gpui_kit::VisualTestContext,
) {
    cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.set_drop_text_for_test(value, window, cx)
        });
    });
}

/// Emits `InputEvent::PressEnter` through the drop dialog's field, exactly as
/// the widget does when the user presses Enter.
pub(crate) fn press_enter_in_drop(
    view: &gpui_kit::Entity<Workspace>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    let input = cx.update(|_window, app| {
        view.read_with(app, |this, _| this.drop_dialog().map(|d| d.input.clone()))
    });
    if let Some(input) = input {
        cx.update(|_window, app| {
            input.update(app, |_state, ctx| {
                ctx.emit(gpui_kit::component::input::InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                });
            });
        });
    }
}
