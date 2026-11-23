//! Databases tab: handler tests over the scriptable service (`F-DBU-01..03`).
//!
//! Each test drives the `Workspace` the way the view does — through the action
//! handlers — and asserts on the pure state the cards render, so a state the
//! task names (empty, many, slow, failed, reset failure) is proven end to end
//! without a real engine.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use dblocal_core::admin::PrivilegeLevel;
use dblocal_core::model::InstanceStatus;
use dblocal_core::service::UserFacingError;
use gpui_kit::TestAppContext;

use super::databases_support::{
    fixture, open_databases, press_enter_in_drop, set_db_name, set_drop_text, set_user_name, settle,
};
use crate::app::InnerTab;
use crate::app::admin_state::{AdminAction, Loadable};

#[gpui_kit::test]
fn entering_the_tab_reads_databases_and_users(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    let (rows, users) = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin state");
        (admin.rows().len(), admin.users().len())
    });
    assert_eq!(rows, 2, "the seeded databases must be read");
    assert_eq!(users, 2, "the seeded users must be read");
    let counts = service.counts();
    assert_eq!(counts.list_databases, 1);
    assert_eq!(counts.list_users, 1);
}

#[gpui_kit::test]
fn an_empty_list_renders_no_rows_and_no_error(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.databases = Ok(Vec::new());
        script.users = Ok(Vec::new());
    });
    let id = open_databases(&view, &service, cx);
    let admin = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin state");
        (
            admin.rows().is_empty(),
            admin.users().is_empty(),
            admin.error.is_some(),
        )
    });
    assert_eq!(
        admin,
        (true, true, false),
        "empty is a ready, error-free state"
    );
}

#[gpui_kit::test]
fn a_failed_read_is_a_typed_error_not_a_panic(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let failure = UserFacingError::new(
        "DBL-8111",
        "Operasi ini belum didukung untuk engine tersebut.",
    );
    service.set(|script| script.databases = Err(failure.clone()));
    let id = open_databases(&view, &service, cx);
    let (error, is_failed) = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin state");
        (admin.error.clone(), admin.data.error().is_some())
    });
    assert_eq!(error.map(|e| e.code), Some("DBL-8111".to_owned()));
    assert!(is_failed, "the read must land as a typed failure");
}

#[gpui_kit::test]
fn a_stopped_instance_is_not_read_and_has_no_error(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Databases, cx));
    settle(cx);
    let (idle, rows, error) = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin state");
        (
            matches!(admin.data, Loadable::Idle),
            admin.rows().len(),
            admin.error.is_some(),
        )
    });
    assert!(
        idle,
        "a stopped instance is 'not read', never a fake empty answer (F1)"
    );
    assert_eq!(rows, 0);
    assert!(
        !error,
        "a stopped instance must not show a connection error"
    );
    assert_eq!(
        service.counts().list_databases,
        0,
        "a stopped instance has no server to read"
    );
}

#[gpui_kit::test]
fn create_database_rejects_a_bad_name_without_a_round_trip(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    let before = service.counts().create_database;
    set_db_name(&view, "app-dev", cx);
    view.update(cx, |this, cx| this.create_database(&id, cx));
    settle(cx);
    assert_eq!(
        service.counts().create_database,
        before,
        "an invalid name must not reach the service (F-DBU-01 AC4)"
    );
    let code = view.read_with(cx, |this, _| {
        this.admin_state_for(&id)
            .and_then(|admin| admin.error.as_ref().map(|e| e.code.clone()))
    });
    assert_eq!(code.as_deref(), Some("DBL-8101"));
}

#[gpui_kit::test]
fn create_database_sends_a_valid_name(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    set_db_name(&view, "shop_db", cx);
    view.update(cx, |this, cx| this.create_database(&id, cx));
    settle(cx);
    assert_eq!(service.counts().create_database, 1);
    let toast = view.read_with(cx, |this, _| this.toast_text().map(|t| t.to_string()));
    assert!(
        toast.as_deref().is_some_and(|t| t.contains("shop_db")),
        "a created database must confirm: {toast:?}"
    );
}

#[gpui_kit::test]
fn drop_confirmation_needs_the_exact_name(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.open_drop_dialog_for_test(&id, "app", cx);
    });
    // Wrong text: the confirm button is disabled and a confirm is a no-op.
    set_drop_text(&view, "app_dev", cx);
    view.update(cx, |this, cx| this.confirm_drop(cx));
    settle(cx);
    assert_eq!(service.counts().drop_database, 0, "mismatch must not drop");

    view.update(cx, |this, cx| {
        this.open_drop_dialog_for_test(&id, "app", cx)
    });
    set_drop_text(&view, "app", cx);
    view.update(cx, |this, cx| this.confirm_drop(cx));
    settle(cx);
    assert_eq!(service.counts().drop_database, 1, "the exact name drops");
}

#[gpui_kit::test]
fn a_created_user_password_is_shown_once_and_dropped_on_close(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    set_user_name(&view, "app_ro", cx);
    view.update(cx, |this, cx| {
        this.choose_user_database("app".to_owned(), cx);
        this.choose_user_level(PrivilegeLevel::ReadOnly, cx);
        this.create_user(&id, cx);
    });
    settle(cx);
    let shown = view.read_with(cx, |this, _| {
        this.secret_dialog().map(|d| d.value.expose().to_owned())
    });
    assert_eq!(shown.as_deref(), Some("pwd-user-0001"));
    view.update(cx, |this, cx| this.close_secret_dialog(cx));
    assert!(
        view.read_with(cx, |this, _| this.secret_dialog().is_none()),
        "the password must not survive the dialog (F-DBU-02 AC2)"
    );
}

#[gpui_kit::test]
fn a_reset_shows_the_new_uri_once_and_updates_the_card(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.open_reset_confirm(&id, cx);
        this.confirm_reset(cx);
    });
    settle(cx);
    let (masked, dialog_ok) = view.read_with(cx, |this, _| {
        (this.uri_text(&id), this.secret_dialog().is_some())
    });
    assert!(dialog_ok, "the reset URI must be shown once");
    assert!(
        masked.contains(crate::format::SECRET_MASK),
        "the Overview card must hold the (masked) new URI: {masked}"
    );
    // Revealing shows the rotated password, not a stale one.
    view.update(cx, |this, cx| this.toggle_reveal(&id, cx));
    settle(cx);
    let revealed = view.read_with(cx, |this, _| this.uri_text(&id));
    assert!(
        revealed.contains("pwd-reset-0001"),
        "the Overview card must carry the new URI: {revealed}"
    );
}

#[gpui_kit::test]
fn a_failed_reset_keeps_the_old_password_and_says_so(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let failure = UserFacingError::new(
        "DBL-8112",
        "Password baru tidak dapat dipakai untuk login; password lama dipulihkan.",
    );
    service.set(|script| script.reset_password = Err(failure));
    let id = open_databases(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.open_reset_confirm(&id, cx);
        this.confirm_reset(cx);
    });
    settle(cx);
    let (reset_failed, dialog) = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin");
        (admin.reset_failed, this.secret_dialog().is_some())
    });
    assert!(
        reset_failed,
        "the card must state the old password still works"
    );
    assert!(!dialog, "a failed reset must not show a URI");
}

#[gpui_kit::test]
fn every_action_is_busy_while_in_flight(cx: &mut TestAppContext) {
    // The buttons disable on the in-flight flag; setting it marks exactly that
    // action and leaves the others.
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    view.update(cx, |this, _| {
        this.admin.entry(&id).begin(AdminAction::CreateDatabase);
    });
    let busy = view.read_with(cx, |this, _| {
        let admin = this.admin_state_for(&id).expect("admin");
        (
            admin.is_busy(AdminAction::CreateDatabase),
            admin.is_busy(AdminAction::CreateUser),
        )
    });
    assert_eq!(busy, (true, false));
}

#[gpui_kit::test]
fn admin_changed_re_reads_the_visible_instance_only(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let visible = open_databases(&view, &service, cx);
    let before = service.counts().list_databases;

    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            crate::service_bridge::UiEvent::AdminChanged(visible.clone()),
            cx,
        );
    });
    settle(cx);
    assert!(
        service.counts().list_databases > before,
        "the visible instance is re-read on AdminChanged"
    );

    let other = service.id_with(InstanceStatus::Stopped);
    let before = service.counts().list_databases;
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            crate::service_bridge::UiEvent::AdminChanged(other.clone()),
            cx,
        );
    });
    settle(cx);
    assert_eq!(
        service.counts().list_databases,
        before,
        "a background instance must not be read eagerly"
    );
}

#[gpui_kit::test]
fn escape_closes_the_top_dialog_and_leaves_the_one_under_it(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    // A secret dialog over the Settings dialog: Escape closes the secret one and
    // leaves Settings up (F-PLH-01, the task's "Escape closes the top dialog").
    view.update(cx, |this, _| {
        this.settings.open();
        this.secret_dialog = Some(crate::app::admin_dialogs::SecretDialog {
            instance: id.clone(),
            kind: crate::app::admin_dialogs::SecretKind::ResetUri,
            value: dblocal_core::secret::SecretString::new("postgresql://x"),
        });
    });
    view.update(cx, |this, _| this.close_modals());
    let (secret, settings) = view.read_with(cx, |this, _| {
        (this.secret_dialog().is_some(), this.settings_state().open)
    });
    assert!(!secret, "Escape must close the top (secret) dialog");
    assert!(settings, "the dialog under it must stay open");
    view.update(cx, |this, _| this.close_modals());
    assert!(
        !view.read_with(cx, |this, _| this.settings_state().open),
        "a second Escape closes Settings"
    );
}

#[gpui_kit::test]
fn enter_confirms_the_drop_only_when_the_name_matches(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_databases(&view, &service, cx);
    // Use the real dialog opener so its Enter subscription is installed.
    cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.open_drop_dialog(&id, "app".to_owned(), window, cx);
        });
    });

    // Enter with the wrong text must not drop.
    set_drop_text(&view, "app_dev", cx);
    press_enter_in_drop(&view, cx);
    settle(cx);
    assert_eq!(
        service.counts().drop_database,
        0,
        "Enter on a mismatch is a no-op"
    );

    // Enter with the exact name drops.
    set_drop_text(&view, "app", cx);
    press_enter_in_drop(&view, cx);
    settle(cx);
    assert_eq!(service.counts().drop_database, 1, "Enter on a match drops");
}
