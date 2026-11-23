//! W4-B fix-1 regression tests: F1, F3, F5, F6, F7 (`F-DBU-01..03`).
//!
//! Split from `databases_tests.rs` so both files stay under the 800-line cap.
//! Every case drives the `Workspace` through the same handlers a click runs, and
//! asserts on the pure state the cards render.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::UserFacingError;
use gpui_kit::Focusable as _;
use gpui_kit::TestAppContext;

use super::databases_support::{
    fixture, open_databases, press_enter_in_drop, set_db_name, set_drop_text, set_user_name, settle,
};
use crate::app::admin_state::Loadable;
use crate::app::{InnerTab, Workspace};
use crate::service_bridge::UiEvent;

/// F1: an instance that starts while its Databases tab is open must re-read.
///
/// The reported defect: the tab was entered while the server was stopped, the
/// start flipped the status to `Running`, and the tab kept the empty list until
/// some unrelated `AdminChanged` arrived.
#[gpui_kit::test]
fn a_started_instance_re_reads_the_visible_tab(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Databases, cx));
    settle(cx);
    assert_eq!(
        service.counts().list_databases,
        0,
        "a stopped instance has no server to read"
    );

    // The status arrives as an event, the way a CLI start reaches the GUI.
    view.update(cx, |this, cx| {
        let mut changed = this.instance(&id).cloned().expect("instance view");
        changed.status = InstanceStatus::Running;
        this.apply_event_with_cx(UiEvent::InstanceChanged(changed), cx);
    });
    settle(cx);

    let rows = view.read_with(cx, |this, _| {
        this.admin_state_for(&id)
            .map(|admin| admin.rows().len())
            .unwrap_or(0)
    });
    assert_eq!(rows, 2, "the started instance must show the service's rows");
    assert_eq!(service.counts().list_databases, 1);
}

/// F1: the reverse transition must not keep showing rows that are now stale.
#[gpui_kit::test]
fn a_stopped_instance_drops_its_rows_again(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    assert!(!view.read_with(cx, |this, _| {
        this.admin_state_for(&id)
            .map(|admin| admin.rows().is_empty())
            .unwrap_or(true)
    }));

    view.update(cx, |this, cx| {
        let mut changed = this.instance(&id).cloned().expect("instance view");
        changed.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(changed), cx);
    });
    settle(cx);

    let (rows, idle) = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin state");
        (admin.rows().len(), matches!(admin.data, Loadable::Idle))
    });
    assert_eq!(rows, 0, "a stopped instance must not keep its rows");
    assert!(
        idle,
        "the stopped state must be 'not read', not a fake empty read"
    );
}

/// F3: the drop dialog takes the keyboard, and gives it back on close.
///
/// Before the fix the covered "Buat database" field kept focus, so typing `x`
/// appended there and the dialog's field stayed empty.
#[gpui_kit::test]
fn the_drop_dialog_takes_focus_and_returns_it_on_close(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    let field = view.read_with(cx, |this, cx| {
        this.db_name_input_entity().read(cx).focus_handle(cx)
    });
    cx.update(|window, cx| field.focus(window, cx));
    assert!(
        cx.update(|window, _| field.is_focused(window)),
        "the fixture focuses the covered field first"
    );

    cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.open_drop_dialog(&id, "app".to_owned(), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let dialog_focused = cx.update(|window, app| {
        let input = view
            .read_with(app, |this, _| this.drop_dialog().map(|d| d.input.clone()))
            .expect("the dialog is open");
        input.read(app).focus_handle(app).is_focused(window)
    });
    assert!(
        dialog_focused,
        "the dialog's field must take focus from the covered field (F3)"
    );
    assert!(
        !cx.update(|window, _| field.is_focused(window)),
        "the covered field must lose focus"
    );

    // Escape closes the top dialog and the covered field gets the keyboard back.
    cx.dispatch_action(crate::app::actions::CloseOverlay);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        view.read_with(cx, |this, _| this.drop_dialog().is_none()),
        "Escape must close the drop dialog"
    );
    assert!(
        cx.update(|window, _| field.is_focused(window)),
        "closing the dialog must return focus to the field it covered (F3)"
    );
}

/// F3: the secret dialog is reachable by keyboard, and Escape closes it.
#[gpui_kit::test]
fn the_secret_dialog_takes_focus_and_escape_closes_it(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    let field = view.read_with(cx, |this, cx| {
        this.db_name_input_entity().read(cx).focus_handle(cx)
    });
    cx.update(|window, cx| field.focus(window, cx));
    set_user_name(&view, "app_ro", cx);
    view.update(cx, |this, cx| {
        this.choose_user_database("app".to_owned(), cx);
        this.create_user(&id, cx);
    });
    settle(cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (open, still_on_field) = cx.update(|window, app| {
        (
            view.read_with(app, |this, _| this.secret_dialog().is_some()),
            field.is_focused(window),
        )
    });
    assert!(open, "the one-time dialog is up");
    assert!(
        !still_on_field,
        "the covered field must not keep the keyboard while a dialog is up"
    );

    cx.dispatch_action(crate::app::actions::CloseOverlay);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        view.read_with(cx, |this, _| this.secret_dialog().is_none()),
        "Escape must close the secret dialog"
    );
    assert!(
        cx.update(|window, _| field.is_focused(window)),
        "focus returns to the covered field after the dialog closes"
    );
}

/// F3: dialog keys stay registerable, and the drop path still confirms on Enter.
#[gpui_kit::test]
fn the_drop_dialog_keeps_its_enter_subscription_after_the_focus_change(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.open_drop_dialog(&id, "app".to_owned(), window, cx);
        });
    });
    set_drop_text(&view, "app", cx);
    press_enter_in_drop(&view, cx);
    settle(cx);
    assert_eq!(
        service.counts().drop_database,
        1,
        "Enter in the focused dialog field must still confirm"
    );
}

/// F6: a successful create empties the name field.
#[gpui_kit::test]
fn a_successful_create_clears_the_name_fields(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    set_db_name(&view, "shop_db", cx);
    view.update(cx, |this, cx| this.create_database(&id, cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, cx| this.db_name_text_for_test(cx)),
        "",
        "the database field must not keep the name it just created (F6)"
    );

    set_user_name(&view, "app_ro", cx);
    view.update(cx, |this, cx| {
        this.choose_user_database("app".to_owned(), cx);
        this.create_user(&id, cx);
    });
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, cx| this.user_name_text_for_test(cx)),
        "",
        "the user field must not keep the name it just created (F6)"
    );
}

/// A failed create keeps the text so the user can correct it.
#[gpui_kit::test]
fn a_failed_create_keeps_the_name(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.create_database = Err(UserFacingError::new("DBL-8104", "sudah ada"));
    });
    let id = open_databases(&view, &service, cx);
    set_db_name(&view, "shop_db", cx);
    view.update(cx, |this, cx| this.create_database(&id, cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, cx| this.db_name_text_for_test(cx)),
        "shop_db",
        "a failure must not throw away what the user typed"
    );
}

/// F6: a successful drop confirms itself instead of leaving the create line.
#[gpui_kit::test]
fn a_drop_reports_itself_in_the_status_bar(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    // Reproduce the report: a create leaves "Database shop_db dibuat (UTF-8)".
    set_db_name(&view, "shop_db", cx);
    view.update(cx, |this, cx| this.create_database(&id, cx));
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this.toast_text())
            .is_some_and(|t| t.contains("dibuat")),
        "the create toast is up first"
    );

    view.update(cx, |this, cx| {
        this.open_drop_dialog_for_test(&id, "app", cx);
    });
    set_drop_text(&view, "app", cx);
    view.update(cx, |this, cx| this.confirm_drop(cx));
    settle(cx);
    let toast = view
        .read_with(cx, |this, _| this.toast_text())
        .map(|t| t.to_string())
        .unwrap_or_default();
    assert!(
        toast.contains("app") && toast.contains("dihapus"),
        "a drop must replace the create line with its own confirmation: {toast}"
    );
}

/// F6: a failed drop states the failure and never claims the drop happened.
#[gpui_kit::test]
fn a_failed_drop_does_not_confirm(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.drop_database = Err(UserFacingError::new("DBL-8106", "masih dipakai"));
    });
    let id = open_databases(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.open_drop_dialog_for_test(&id, "app", cx);
    });
    set_drop_text(&view, "app", cx);
    view.update(cx, |this, cx| this.confirm_drop(cx));
    settle(cx);
    let (toast, error) = view.read_with(cx, |this, _| {
        (
            this.toast_text().map(|t| t.to_string()).unwrap_or_default(),
            this.admin_state_for(&id)
                .and_then(|a| a.error.as_ref().map(|e| e.code.clone())),
        )
    });
    assert!(
        !toast.contains("dihapus"),
        "a failed drop must not claim it happened: {toast}"
    );
    assert_eq!(error.as_deref(), Some("DBL-8106"));
}

/// F5: no code from the core registry is invented on the dead no-database path.
///
/// `create_user` used to build `DBL-8106` ("database in use") with the label
/// "Database" when nothing was chosen. The button is disabled in that state, so
/// the branch is unreachable from the UI — but a stray code must not exist.
#[gpui_kit::test]
fn create_user_without_a_database_sends_nothing_and_claims_nothing(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    set_user_name(&view, "app_ro", cx);
    // No `choose_user_database`: the same state as the disabled button.
    view.update(cx, |this, cx| this.create_user(&id, cx));
    settle(cx);

    assert_eq!(
        service.counts().create_user,
        0,
        "no database means no round trip"
    );
    let (error, dialog) = view.read_with(cx, |this, _| {
        (
            this.admin_state_for(&id).and_then(|a| a.error.clone()),
            this.secret_dialog().is_some(),
        )
    });
    assert!(
        error.is_none(),
        "the dead path must not invent a code: {error:?}"
    );
    assert!(!dialog, "nothing was created, so no password dialog");
}

/// F7 mutation 1: the GUI must ask for a non-forced drop.
///
/// `confirm_drop` passing `true` would silently kill other sessions; no test
/// pinned the argument, so the mutation stayed green.
#[gpui_kit::test]
fn a_drop_is_never_forced(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.open_drop_dialog_for_test(&id, "app", cx);
    });
    set_drop_text(&view, "app", cx);
    view.update(cx, |this, cx| this.confirm_drop(cx));
    settle(cx);

    let drops = service.mutations().drops;
    assert_eq!(drops.len(), 1, "the drop must cross the seam");
    let (dropped_instance, dropped_name, force) = &drops[0];
    assert_eq!(dropped_instance, &id, "the drop must name its own instance");
    assert_eq!(dropped_name, "app");
    assert!(
        !force,
        "F-DBU-01 AC3: the GUI must never force a drop and kill a live session"
    );
}

/// F7 mutation 2: an `AdminChanged` behind the visible tab must reset its rows.
///
/// With the reset removed the non-visible instance kept `Ready` rows, so
/// entering its tab issued no read and showed rows that had changed meanwhile.
#[gpui_kit::test]
fn admin_changed_invalidates_a_non_visible_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let visible = open_databases(&view, &service, cx);
    // The other instance runs too, so entering its tab can read.
    let other = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, _cx| {
        let mut changed = this.instance(&other).cloned().expect("instance view");
        changed.status = InstanceStatus::Running;
        this.state.upsert_instance(changed);
        // Its rows are stale-but-ready, as a tab read before it went background.
        this.admin.entry(&other).data = Loadable::Ready(crate::app::admin_state::AdminData {
            databases: vec![dblocal_core::service::DatabaseInfo::new(
                "stale", "postgres", 4096,
            )],
            users: Vec::new(),
        });
    });
    assert!(
        view.read_with(cx, |this, _| this
            .admin_state_for(&other)
            .is_some_and(|a| a.rows().len() == 1)),
        "the fixture plants a stale ready row"
    );

    // `visible` is the tab on screen, so `other` is a background instance.
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::AdminChanged(other.clone()), cx);
    });
    let reset = view.read_with(cx, |this, _| {
        this.admin_state_for(&other)
            .is_some_and(|admin| matches!(admin.data, Loadable::Idle))
    });
    assert!(
        reset,
        "AdminChanged must send a background instance's rows back to Idle"
    );

    // Entering its tab must issue a fresh read rather than trust the stale rows.
    let before = service.counts().list_databases;
    view.update(cx, |this, cx| {
        this.open_tab(other.clone(), cx);
        this.set_inner_tab(InnerTab::Databases, cx);
    });
    settle(cx);
    assert!(
        service.counts().list_databases > before,
        "entering the invalidated tab must re-read it"
    );
    assert!(
        view.read_with(cx, |this, _| this
            .admin_state_for(&other)
            .is_some_and(|a| a.rows().len() == 2)),
        "the tab must show the service's rows, not the stale one"
    );
    let _ = visible;
}

/// F7 mutation 3: the one-time secret never reaches a toast, the bar or a
/// `Debug` of the view state.
///
/// With the canary in place, a toast built as `format!("{user} {password}")`
/// fails the test instead of shipping a password to the status bar.
#[gpui_kit::test]
fn the_one_time_secret_never_leaves_the_dialog(cx: &mut TestAppContext) {
    const CANARY: &str = "canary-pwd-8f3a";
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.create_user = Ok(dblocal_core::secret::SecretString::new(CANARY));
        script.reset_password = Ok(dblocal_core::secret::SecretString::new(CANARY));
        script.uri = Ok(dblocal_core::secret::SecretString::new(format!(
            "postgresql://postgres:{CANARY}@127.0.0.1:5432/postgres"
        )));
    });
    let id = open_databases(&view, &service, cx);

    set_user_name(&view, "app_ro", cx);
    view.update(cx, |this, cx| {
        this.choose_user_database("app".to_owned(), cx);
        this.create_user(&id, cx);
    });
    settle(cx);
    assert_no_canary_outside_the_dialog(&view, CANARY, cx, "create_user");

    view.update(cx, |this, cx| this.close_secret_dialog(cx));
    settle(cx);
    assert_no_canary_outside_the_dialog(&view, CANARY, cx, "after the user dialog closed");

    view.update(cx, |this, cx| {
        this.open_reset_confirm(&id, cx);
        this.confirm_reset(cx);
    });
    settle(cx);
    assert_no_canary_outside_the_dialog(&view, CANARY, cx, "confirm_reset");

    view.update(cx, |this, cx| this.close_secret_dialog(cx));
    settle(cx);
    assert_no_canary_outside_the_dialog(&view, CANARY, cx, "after the reset dialog closed");
}

/// Fails when `canary` appears anywhere the user can read without the dialog.
///
/// The surfaces are the ones the mutation built the leak into: a toast, the
/// status bar (which renders the toast), a card error, and the whole view state
/// through `Debug`.
fn assert_no_canary_outside_the_dialog(
    view: &gpui_kit::Entity<Workspace>,
    canary: &str,
    cx: &mut gpui_kit::VisualTestContext,
    stage: &str,
) {
    view.read_with(cx, |this, _| {
        let toast = this.toast_text().map(|t| t.to_string()).unwrap_or_default();
        assert!(
            !toast.contains(canary),
            "{stage}: the one-time secret reached a toast/status bar: {toast}"
        );
        let errors: Vec<String> = this
            .instances()
            .iter()
            .filter_map(|instance| this.admin_state_for(&instance.id))
            .filter_map(|admin| admin.error.as_ref())
            .map(|err| format!("{} — {} {err:?}", err.code, err.message))
            .collect();
        for text in errors {
            assert!(
                !text.contains(canary),
                "{stage}: the one-time secret reached a card error: {text}"
            );
        }
        let state = view_state_dump(this);
        assert!(
            !state.contains(canary),
            "{stage}: the one-time secret is in the view state's Debug output"
        );
    });
}

/// The `Debug` of every field that could carry a secret.
///
/// `Workspace` holds `Arc<dyn DblService>`, which is not `Debug`, so the whole
/// entity cannot derive it. These are the fields that state or error text lives
/// in — the toast already covered above, plus every per-instance view.
fn view_state_dump(workspace: &Workspace) -> String {
    let admins: Vec<&crate::app::admin_state::InstanceAdmin> = workspace
        .instances()
        .iter()
        .filter_map(|instance| workspace.admin_state_for(&instance.id))
        .collect();
    let uris: Vec<String> = workspace
        .instances()
        .iter()
        .map(|instance| workspace.uri_text(&instance.id))
        .collect();
    format!(
        "toast={:?} admins={admins:?} uris={uris:?} settings={:?} dialog={:?}",
        workspace.toast_text(),
        workspace.settings_state(),
        workspace.secret_dialog().map(|d| d.kind.clone()),
    )
}
