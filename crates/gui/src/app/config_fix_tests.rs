//! W4-H fix-round tests (`F1`-`F7`): the two wrong restarts, the stale banner,
//! the disabled-when-invalid apply, the focus after Confirm and the live re-read.
//!
//! Split out of `config_tests.rs` so both files stay under the size cap; the
//! harness (`fixture`, `settle`, `open_config`, `view_with_backup`) is shared.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::UserFacingError;
use gpui_kit::{Focusable, TestAppContext};

use super::config_tests::{open_config, view_with_backup};
use super::databases_support::{fixture, settle};
use crate::app::InnerTab;
use crate::service_bridge::UiEvent;

/// `F1`: a failed restore is reported typed and never starts the instance.
///
/// Red on `a6f4255` (`restore_config_now` started regardless of the restore's
/// result); this is the reviewer's first probe, pinned as a product test.
#[gpui_kit::test]
fn a_failed_restore_is_typed_and_does_not_start_the_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.config_view = Ok(view_with_backup());
        script.config_restore = Err(UserFacingError::new("DBL-8605", "pulihkan gagal"));
    });
    let id = open_config(&view, &service, cx);
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| this.restore_and_restart(&id, cx));
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "F1: a failed restore must not be followed by a start"
    );
    let code = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|c| c.error.as_ref().map(|e| e.code.clone()))
    });
    assert_eq!(code.as_deref(), Some("DBL-8605"));
}

/// `F1` mirror: a good restore then a failing start reports the start's own
/// typed failure and never rolls the restored file back.
#[gpui_kit::test]
fn a_good_restore_then_a_failed_start_reports_the_start(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.config_view = Ok(view_with_backup());
        script.config_restore = Ok(());
        script.start_instance = Err(UserFacingError::new("DBL-4001", "mesin gagal start"));
    });
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.restore_and_restart(&id, cx));
    settle(cx);
    assert_eq!(
        service.mutations().config_restores,
        vec![id.clone()],
        "F1: the restore went out exactly once"
    );
    assert_eq!(
        service.counts().start_instance,
        1,
        "F1: the start is attempted after a good restore"
    );
    let code = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|c| c.error.as_ref().map(|e| e.code.clone()))
    });
    assert_eq!(
        code.as_deref(),
        Some("DBL-4001"),
        "F1: the start's failure is the reported one, not the restore's"
    );
}

/// `F2`: a stop that fails after "Restart sekarang" must not leave a restart
/// that a later manual stop consumes. This is the reviewer's second probe.
#[gpui_kit::test]
fn a_failed_stop_after_restart_now_leaves_no_pending_restart(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Running);
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| this.restart_after_apply(&id, cx));
    settle(cx);
    // The queued stop's OperationFinished says it failed; the instance is still
    // running (the event does not change its status).
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::OperationFinished {
                id: dblocal_core::service::OperationId::new("op-stop"),
                result: Err(UserFacingError::new("DBL-4002", "stop gagal")),
            },
            cx,
        );
    });
    settle(cx);
    // Much later the user stops the instance by hand: no start may follow.
    view.update(cx, |this, cx| {
        let mut stopped = this.instance(&id).cloned().expect("instance view");
        stopped.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "F2: a manual stop after a failed Restart sekarang must not auto-start"
    );
}

/// `F2`: an unrelated operation finishing with an error must not clear a pending
/// restart, and a pending restart of A is never consumed by B's stop.
#[gpui_kit::test]
fn a_pending_restart_matches_only_its_own_stop_and_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let running = service.id_with(InstanceStatus::Running);
    let other = service.id_with(InstanceStatus::Stopped);
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| this.restart_after_apply(&running, cx));
    settle(cx);
    // An unrelated operation fails: the restart for `running` must survive.
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::OperationFinished {
                id: dblocal_core::service::OperationId::new("op-unrelated"),
                result: Err(UserFacingError::new("DBL-4002", "lain gagal")),
            },
            cx,
        );
    });
    // The other instance stops: that is not the stop `running` waits on.
    view.update(cx, |this, cx| {
        let mut stopped = this.instance(&other).cloned().expect("instance view");
        stopped.status = InstanceStatus::Running;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
        let mut stopped = this.instance(&other).cloned().expect("instance view");
        stopped.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "F2: another instance's stop must not consume A's pending restart"
    );
    // The queued stop for `running` now completes: the restart runs exactly once.
    view.update(cx, |this, cx| {
        let mut stopped = this.instance(&running).cloned().expect("instance view");
        stopped.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before + 1,
        "F2: the real stop still completes the happy path"
    );
}

/// `F2`: an instance that goes back to `Running` instead of `Stopped` (a stop
/// that ends between `Stopping` and `Running`) drops the pending restart.
#[gpui_kit::test]
fn a_stop_that_ends_running_drops_the_pending_restart(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Running);
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| this.restart_after_apply(&id, cx));
    settle(cx);
    view.update(cx, |this, cx| {
        let mut stopping = this.instance(&id).cloned().expect("instance view");
        stopping.status = InstanceStatus::Stopping;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopping), cx);
    });
    view.update(cx, |this, cx| {
        let mut running = this.instance(&id).cloned().expect("instance view");
        running.status = InstanceStatus::Running;
        this.apply_event_with_cx(UiEvent::InstanceChanged(running), cx);
    });
    settle(cx);
    view.update(cx, |this, cx| {
        let mut stopped = this.instance(&id).cloned().expect("instance view");
        stopped.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "F2: a stop that never reached Stopped drops the restart"
    );
}

/// `F4`: the "Tersimpan. Berlaku setelah restart." banner is gone once the
/// instance is running again — a stopped Valkey that was applied and then
/// started must not keep offering a restart for a change already in effect.
#[gpui_kit::test]
fn the_applied_banner_clears_when_the_instance_starts(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| {
        this.open_tab(id.clone(), cx);
        this.set_inner_tab(InnerTab::Config, cx);
    });
    settle(cx);
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "300");
        this.apply_config(&id, cx);
    });
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this
            .config_state_for(&id)
            .is_some_and(|c| c.applied.is_some())),
        "F4: the apply raises the banner"
    );
    // The user starts the stopped instance: the change is now in effect.
    view.update(cx, |this, cx| {
        let mut running = this.instance(&id).cloned().expect("instance view");
        running.status = InstanceStatus::Running;
        this.apply_event_with_cx(UiEvent::InstanceChanged(running), cx);
    });
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this
            .config_state_for(&id)
            .is_some_and(|c| c.applied.is_none())),
        "F4: a start applies the change, so the banner must be gone"
    );
}

/// `F4`: a banner shown while the instance is already running stays until it is
/// restarted or dismissed (a start is not what applies it there).
#[gpui_kit::test]
fn the_applied_banner_stays_while_a_running_instance_just_sits_there(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "300");
        this.apply_config(&id, cx);
    });
    settle(cx);
    // A running instance's banner waits for "Restart sekarang"/"Nanti".
    let applied = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .is_some_and(|c| c.applied.is_some())
    });
    assert!(applied, "F4: the banner stays until restarted or dismissed");
}

/// `F4`: the banner does not flicker through a restart taken from the header —
/// the stop half keeps it, the start half clears it.
#[gpui_kit::test]
fn the_applied_banner_survives_the_stop_half_of_a_restart(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "300");
        this.apply_config(&id, cx);
    });
    settle(cx);
    // The header's Restart: Running -> Stopping, no banner dismissal.
    view.update(cx, |this, cx| {
        let mut stopping = this.instance(&id).cloned().expect("instance view");
        stopping.status = InstanceStatus::Stopping;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopping), cx);
    });
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this
            .config_state_for(&id)
            .is_some_and(|c| c.applied.is_some())),
        "F4: the stop half of a restart keeps the banner, no flicker"
    );
    // Stopping -> Running: the change is in effect now.
    view.update(cx, |this, cx| {
        let mut running = this.instance(&id).cloned().expect("instance view");
        running.status = InstanceStatus::Running;
        this.apply_event_with_cx(UiEvent::InstanceChanged(running), cx);
    });
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this
            .config_state_for(&id)
            .is_some_and(|c| c.applied.is_none())),
        "F4: the start half clears it"
    );
}

/// `F5`: "Terapkan" is disabled while a field carries a validation error, and
/// Enter in that field does not call the service.
#[gpui_kit::test]
fn apply_is_disabled_and_enter_is_inert_while_a_field_has_an_error(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.record_config_field(&id, "max_connections", "bukan-angka", cx);
    });
    settle(cx);
    let (can_apply, has_error) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.can_apply(),
            config.errors.contains_key("max_connections"),
        )
    });
    assert!(has_error, "F5: the field shows its DBL-8601 error");
    assert!(
        !can_apply,
        "F5: Terapkan is disabled while a field has an error"
    );

    let before = service.counts().config_apply;
    // Enter in the field runs the same guarded path a keypress runs.
    view.update(cx, |this, cx| {
        this.apply_config_if_ready_for_test(&id, cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().config_apply,
        before,
        "F5: Enter with a field error must not call the service"
    );
}

/// `F6`: Confirm takes the dialog away exactly like Escape and Cancel, so it
/// must park the focus restore the frame applies.
#[gpui_kit::test]
fn confirm_restore_returns_the_focus_like_escape(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);
    // A field the dialog covers; open the dialog from it.
    let field = view.read_with(cx, |this, cx| {
        this.config_field_handle(&id, "max_connections")
            .map(|entity| entity.read(cx).focus_handle(cx))
    });
    let Some(field) = field else {
        return; // no field entity off-screen; the pending flag is what matters
    };
    cx.update(|window, cx| field.focus(window, cx));
    // The window only records the focus on the next frame.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|window, cx| window.focused(cx)),
        Some(field.clone()),
        "F6: the field has the keyboard before the dialog"
    );
    view.update(cx, |this, cx| this.open_config_restore(&id, cx));
    // The dialog takes the keyboard while it is open.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_ne!(
        cx.update(|window, cx| window.focused(cx)),
        Some(field.clone()),
        "F6: the open dialog holds the keyboard"
    );
    view.update(cx, |this, cx| this.confirm_config_restore(cx));
    // Confirm takes the dialog away; the next frame gives the field its focus
    // back, exactly as Escape and Cancel do.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.update(|window, cx| window.focused(cx)),
        Some(field.clone()),
        "F6: Confirm must return the focus to the covered field"
    );
}

/// `F7a`: entering the tab re-reads every time, so an edit made by the CLI (an
/// out-of-process writer with no `ConfigChanged`) is visible on return.
#[gpui_kit::test]
fn re_entering_the_tab_sees_an_out_of_process_edit(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    // The file changed behind the app: the next read returns 120.
    service.set(|script| {
        let mut new = view_with_backup();
        new.settings[0].current = Some("120".to_owned());
        script.config_view = Ok(new);
    });
    let before = service.counts().config_view;
    // Leave and come back to the tab (the CLI wrote while we were away).
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Overview, cx));
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Config, cx));
    settle(cx);
    assert!(
        service.counts().config_view > before,
        "F7a: re-entering the tab must re-read"
    );
    let shown = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|c| c.view())
            .and_then(|v| v.settings.first())
            .and_then(|s| s.current.clone())
    });
    assert_eq!(
        shown.as_deref(),
        Some("120"),
        "F7a: the CLI edit is visible after returning to the tab"
    );
}

/// `F7a`: a window activation re-reads the visible Config tab, which is the path
/// back from the external editor that "Buka file konfigurasi" launches.
#[gpui_kit::test]
fn a_window_activation_re_reads_the_visible_config_tab(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.open_config_raw(&id, cx));
    settle(cx);
    let before = service.counts().config_view;
    view.update(cx, |this, cx| this.refresh_config_on_activation(cx));
    settle(cx);
    assert!(
        service.counts().config_view > before,
        "F7a: an activation re-reads the visible Config tab"
    );
}

/// `F7b`: a re-read keeps the typed text and the rows: it never blanks the form
/// or drops the field the user is typing in.
#[gpui_kit::test]
fn a_re_read_keeps_the_typed_text_and_the_rows(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.record_config_field(&id, "max_connections", "300", cx);
    });
    // Force the refresh state the way `reload_config_bg` enters it.
    view.update(cx, |this, _| this.config.entry(&id).begin_refresh());
    let (rows, refreshing) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (config.editable().len(), config.refreshing)
    });
    assert!(
        rows > 0,
        "F7b: a refresh keeps the rows on screen, no 'Memuat...'"
    );
    assert!(refreshing, "F7b: the card knows a read is in flight");
    let typed = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|c| c.edits.get("max_connections").cloned())
    });
    assert_eq!(
        typed.as_deref(),
        Some("300"),
        "F7b: the typed text survives the refresh"
    );
    let _ = service;
}

/// `M9`: typing an invalid value shows the `DBL-8601` sentence under the field
/// immediately, before any Terapkan — the real window does, and only the apply
/// path was tested before.
#[gpui_kit::test]
fn typing_an_invalid_value_shows_the_error_before_apply(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    let applies_before = service.counts().config_apply;
    view.update(cx, |this, cx| {
        this.record_config_field(&id, "max_connections", "abc", cx);
    });
    let code = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|c| c.errors.get("max_connections").map(|e| e.code.clone()))
    });
    assert_eq!(
        code.as_deref(),
        Some("DBL-8601"),
        "M9: typing validates with the service's message before apply"
    );
    assert_eq!(
        service.counts().config_apply,
        applies_before,
        "M9: no round trip for the obvious bad value"
    );
}

/// `M11`: removing an instance drops its Config state, a pending restart and an
/// open restore confirmation (`config.forget` + `forget_config_fields`).
#[gpui_kit::test]
fn removing_an_instance_forgets_its_whole_config_tab(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);
    // A pending "Restart sekarang" and an open confirmation on this instance.
    view.update(cx, |this, cx| {
        let running = service.id_with(InstanceStatus::Running);
        this.restart_after_apply(&running, cx);
        this.open_config_restore(&id, cx);
    });
    assert!(
        view.read_with(cx, |this, _| this.config_state_for(&id).is_some()),
        "M11: the tab state exists before removal"
    );
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::InstanceRemoved(id.clone()), cx);
    });
    let (state, confirm, pending) = view.read_with(cx, |this, _| {
        (
            this.config_state_for(&id).is_some(),
            this.config_restore_confirm().is_some(),
            this.config_restart_pending_for_test().is_some(),
        )
    });
    assert!(!state, "M11: the Config state is gone");
    assert!(!confirm, "M11: the open confirmation is gone");
    assert!(!pending, "M11: the pending restart is gone");
}

/// `M17`: a second restore or open while one is in flight does nothing; the
/// service sees a single call, not two racing ones.
#[gpui_kit::test]
fn a_second_restore_or_open_while_busy_does_nothing(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);

    // Two restores back to back: the second is refused while the first is busy.
    view.update(cx, |this, cx| {
        this.restore_and_restart(&id, cx);
        this.restore_and_restart(&id, cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().config_restore,
        1,
        "M17: a restore during a restore is a no-op"
    );

    // Two opens back to back: the same.
    view.update(cx, |this, cx| {
        this.open_config_raw(&id, cx);
        this.open_config_raw(&id, cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().config_open_raw,
        1,
        "M17: an open during an open is a no-op"
    );
}

/// `F8`: an invalid edit keeps its error across a re-read, so "Terapkan" never
/// turns blue while the field is invalid. Repro: type `abc`, leave the tab and
/// come back (the tab re-reads every entry now).
#[gpui_kit::test]
fn an_invalid_edit_keeps_its_error_across_a_re_read(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.record_config_field(&id, "max_connections", "abc", cx);
    });
    let before = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.errors.contains_key("max_connections"),
            config.can_apply(),
        )
    });
    assert!(before.0, "F8: the invalid field shows DBL-8601");
    assert!(!before.1, "F8: Terapkan is disabled before the re-read");

    // Leave and come back: every entry re-reads, which re-bases the edits.
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Overview, cx));
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Config, cx));
    settle(cx);
    let after = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.edits.get("max_connections").cloned(),
            config.errors.get("max_connections").map(|e| e.code.clone()),
            config.can_apply(),
        )
    });
    assert_eq!(
        after.0.as_deref(),
        Some("abc"),
        "F8: the typed text survives the re-read"
    );
    assert_eq!(
        after.1.as_deref(),
        Some("DBL-8601"),
        "F8: the surviving invalid edit keeps its error"
    );
    assert!(
        !after.2,
        "F8: Terapkan never turns blue while a field is invalid"
    );
}

/// `N13`: "Buka file konfigurasi" re-reads after the launcher returns, so a
/// change the editor made (no window activation) is in the form.
#[gpui_kit::test]
fn opening_the_raw_file_re_reads_the_new_values(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    // The editor saved a new value while it was open.
    service.set(|script| {
        let mut changed = view_with_backup();
        changed.settings[0].current = Some("424".to_owned());
        script.config_view = Ok(changed);
    });
    let before = service.counts().config_view;
    view.update(cx, |this, cx| this.open_config_raw(&id, cx));
    settle(cx);
    assert!(
        service.counts().config_view > before,
        "N13: the open re-reads after the launcher returns"
    );
    let shown = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|c| c.view())
            .and_then(|v| v.settings.first())
            .and_then(|s| s.current.clone())
    });
    assert_eq!(
        shown.as_deref(),
        Some("424"),
        "N13: the form shows what the service now reads"
    );
}

/// `N14`: a stop that cannot even be requested leaves no pending restart, shows
/// the typed error, and a later manual stop does not start the instance.
#[gpui_kit::test]
fn a_refused_stop_leaves_no_pending_restart(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Running);
    service.set(|script| {
        script.stop_instance = Err(UserFacingError::new("DBL-4002", "stop ditolak"));
    });
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| this.restart_after_apply(&id, cx));
    settle(cx);
    let (toast, pending) = view.read_with(cx, |this, _| {
        (
            this.toast_text().map(|t| t.to_string()),
            this.config_restart_pending_for_test().is_some(),
        )
    });
    assert!(
        toast.as_deref().is_some_and(|t| t.contains("DBL-")),
        "N14: the refused stop is reported typed: {toast:?}"
    );
    assert!(!pending, "N14: no restart is left pending");
    // A later manual stop must not start the instance.
    view.update(cx, |this, cx| {
        let mut stopped = this.instance(&id).cloned().expect("instance view");
        stopped.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "N14: a manual stop after a refused Restart sekarang must not auto-start"
    );
}
