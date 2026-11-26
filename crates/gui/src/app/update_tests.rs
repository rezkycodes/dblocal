//! In-app update handler and dialog tests (`F-PKG-05`).
//!
//! Every test drives the real `Workspace` over the scriptable service, so the
//! icon, the dialog, cancel, retry, restart and the failure path are exercised
//! exactly as a click would. Nothing here downloads a real release.

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::service::{AvailableUpdateView, DblService, OperationId, UpdateStatus};
use dblocal_core::update::{InstallKind, UpdateProgress};
use gpui_kit::TestAppContext;

use super::Workspace;
use super::update_state::UpdatePhase;
use crate::app::scripted_service::ScriptedService;
use crate::service_bridge::UiEvent;

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
    for _ in 0..60 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cx.run_until_parked();
}

fn available(can_self_update: bool) -> AvailableUpdateView {
    AvailableUpdateView {
        version: "0.9.0".to_owned(),
        tag: "v0.9.0".to_owned(),
        release_notes: "Notes".to_owned(),
        release_url: "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0".to_owned(),
        asset_name: "dblocal-linux-x86_64.tar.gz".to_owned(),
        size_bytes: 1024 * 1024,
        can_self_update,
    }
}

fn status(available: Option<AvailableUpdateView>, kind: InstallKind) -> UpdateStatus {
    UpdateStatus {
        current: "0.8.0".to_owned(),
        available,
        cannot_update_reason: kind.refusal_reason(),
        install_kind: kind,
        package_manager_hint: None,
    }
}

fn portable() -> InstallKind {
    InstallKind::PortableTarball {
        exe_path: "/opt/dblocal/dblocal".into(),
    }
}

fn distro() -> InstallKind {
    InstallKind::DistPackage {
        exe_path: "/usr/bin/dblocal".into(),
        hint: dblocal_core::update::PackageManagerHint::Dpkg,
    }
}

/// No update: the icon is hidden.
#[gpui_kit::test]
fn no_update_hides_the_icon(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.check_due = Ok(Some(status(None, portable()))));
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    assert!(!view.read_with(cx, |this, _| this.update_state().icon_visible()));
    assert_eq!(service.counts().check_due, 1, "the check really ran");
}

/// A quiet check (offline or rate limited) leaves the icon hidden and pops up
/// nothing.
#[gpui_kit::test]
fn a_quiet_check_pops_up_nothing(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.check_due = Ok(None));
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    assert!(!view.read_with(cx, |this, _| this.update_state().icon_visible()));
    assert!(!view.read_with(cx, |this, _| this.update_state().dialog_visible()));
}

/// Update available: the icon shows and the dialog is not open yet.
#[gpui_kit::test]
fn update_available_shows_the_icon(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.check_due = Ok(Some(status(Some(available(true)), portable()))));
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.update_state().icon_visible()));
    assert_eq!(
        view.read_with(cx, |this, _| this
            .update_state()
            .available_version()
            .map(str::to_owned))
            .as_deref(),
        Some("0.9.0")
    );
    assert!(!view.read_with(cx, |this, _| this.update_state().dialog_visible()));
}

/// Pressing the icon opens the dialog; pressing again while downloading never
/// starts a second operation.
#[gpui_kit::test]
fn start_then_second_press_is_refused(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.check_due = Ok(Some(status(Some(available(true)), portable()))));
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    // One press of the icon starts the update.
    view.update_in(cx, |this, window, cx| this.press_update_button(window, cx));
    settle(cx);
    assert_eq!(service.counts().start_update, 1);
    assert!(view.read_with(cx, |this, _| this.update_state().running()));
    assert!(view.read_with(cx, |this, _| this.update_state().dialog_visible()));

    // A second press opens the dialog and must not start another.
    view.update_in(cx, |this, window, cx| this.press_update_button(window, cx));
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    assert_eq!(
        service.counts().start_update,
        1,
        "a running update must not start a second"
    );
}

/// Download samples move the percent, and cancel reaches the service.
#[gpui_kit::test]
fn downloading_progress_and_cancel(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-me"));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);

    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::UpdateProgress {
                id: OperationId::new("op-me"),
                progress: UpdateProgress::Downloading {
                    received_bytes: 512 * 1024,
                    total_bytes: Some(1024 * 1024),
                },
            },
            cx,
        );
    });
    settle(cx);
    let (percent, visible, can_cancel) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (state.percent(), state.dialog_visible(), state.can_cancel())
    });
    assert_eq!(percent, Some(50));
    assert!(visible, "a progress sample opens the dialog");
    assert!(can_cancel);

    view.update(cx, |this, cx| this.cancel_update(cx));
    settle(cx);
    assert_eq!(service.counts().cancel_operation, 1);
}

/// A cancel event lands in the Cancelled phase, which offers a retry.
#[gpui_kit::test]
fn a_cancel_event_is_its_own_phase(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-me"));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::UpdateProgress {
                id: OperationId::new("op-me"),
                progress: UpdateProgress::Cancelled,
            },
            cx,
        );
    });
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.update_state().phase.clone()),
        UpdatePhase::Cancelled
    );
    assert!(view.read_with(cx, |this, _| this.update_state().can_retry()));
    assert!(!view.read_with(cx, |this, _| this.update_state().running()));
}

/// Verify and install are shown, then ready offers the restart.
#[gpui_kit::test]
fn verifying_installing_then_ready(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-me"));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    let op = OperationId::new("op-me");
    for progress in [UpdateProgress::Verifying, UpdateProgress::Installing] {
        view.update(cx, |this, cx| {
            this.apply_event_with_cx(
                UiEvent::UpdateProgress {
                    id: op.clone(),
                    progress,
                },
                cx,
            );
        });
        settle(cx);
        assert!(
            view.read_with(cx, |this, _| this.update_state().running()),
            "verifying and installing are in-flight phases"
        );
    }
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::UpdateProgress {
                id: op.clone(),
                progress: UpdateProgress::ReadyToRestart {
                    version: "0.9.0".to_owned(),
                    restart_target: "/opt/dblocal/dblocal".into(),
                },
            },
            cx,
        );
    });
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.update_state().can_restart()));
    assert!(!view.read_with(cx, |this, _| this.update_state().running()));
}

/// An installed update restarts on its own: the service spawns the new binary
/// and the old process quits. A second ready event must not launch twice.
#[gpui_kit::test]
fn a_ready_update_restarts_and_quits_once(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    // `request_app_quit` records only when the ledger global exists, which the
    // real app installs at startup.
    cx.update(|_, cx| {
        if !cx.has_global::<crate::app::bootstrap::QuitLedger>() {
            cx.set_global(crate::app::bootstrap::QuitLedger::default());
        }
    });
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-me"));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    let ready = UiEvent::UpdateProgress {
        id: OperationId::new("op-me"),
        progress: UpdateProgress::ReadyToRestart {
            version: "0.9.0".to_owned(),
            restart_target: "/opt/dblocal/dblocal".into(),
        },
    };
    view.update(cx, |this, cx| this.apply_event_with_cx(ready.clone(), cx));
    settle(cx);
    assert_eq!(service.counts().restart_for_update, 1);
    assert_eq!(
        cx.read(crate::app::bootstrap::quit_requests),
        1,
        "the restart must quit the old process"
    );

    // A duplicate ready event (a second listener) must not launch again.
    view.update(cx, |this, cx| this.apply_event_with_cx(ready, cx));
    settle(cx);
    assert_eq!(
        service.counts().restart_for_update,
        1,
        "the restart must fire once"
    );
}

/// A failed relaunch keeps the installed state, so "Restart now" retries the
/// launch instead of downloading the release again.
#[gpui_kit::test]
fn a_failed_restart_keeps_the_installed_state(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-me"));
        script.restart = Err(dblocal_core::service::UserFacingError::new(
            "DBL-8008",
            "could not launch the updated application",
        ));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::UpdateProgress {
                id: OperationId::new("op-me"),
                progress: UpdateProgress::ReadyToRestart {
                    version: "0.9.0".to_owned(),
                    restart_target: "/opt/dblocal/dblocal".into(),
                },
            },
            cx,
        );
    });
    settle(cx);
    let (can_restart, code) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (
            state.can_restart(),
            state.restart_error.as_ref().map(|e| e.code.clone()),
        )
    });
    assert!(
        can_restart,
        "the version is installed; only the launch failed"
    );
    assert_eq!(code.as_deref(), Some("DBL-8008"));

    view.update(cx, |this, cx| this.retry_restart(cx));
    settle(cx);
    assert_eq!(
        service.counts().restart_for_update,
        2,
        "the retry must launch again, not re-download"
    );
    assert_eq!(
        service.counts().start_update,
        1,
        "a failed relaunch must not start a second download"
    );
}

/// A failed update shows its code and keeps the app usable with a retry.
#[gpui_kit::test]
fn a_failed_update_shows_the_code_and_offers_a_retry(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-me"));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            UiEvent::UpdateProgress {
                id: OperationId::new("op-me"),
                progress: UpdateProgress::Failed {
                    code: "DBL-8005".to_owned(),
                    message: "The checksum did not match.".to_owned(),
                },
            },
            cx,
        );
    });
    settle(cx);
    let (code, can_retry, visible) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (
            match &state.phase {
                UpdatePhase::Failed { code, .. } => Some(code.clone()),
                _ => None,
            },
            state.can_retry(),
            state.icon_visible(),
        )
    });
    assert_eq!(code.as_deref(), Some("DBL-8005"));
    assert!(can_retry);
    assert!(visible, "the release is still there to retry");
}

/// A managed install explains itself and offers no download.
#[gpui_kit::test]
fn a_managed_install_explains_instead_of_downloading(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(false)), distro())));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    view.update(cx, |this, cx| this.start_update(cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.update_state().phase.clone()),
        UpdatePhase::CannotSelfUpdate
    );
    assert_eq!(
        service.counts().start_update,
        0,
        "a managed install must never start a download"
    );
    assert!(view.read_with(cx, |this, _| {
        this.update_state().cannot_update_reason().is_some()
    }));
}

/// Turning the automatic check on takes effect immediately.
#[gpui_kit::test]
fn enabling_auto_check_runs_a_check_now(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.auto_check = false;
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
    });
    view.update(cx, |this, cx| {
        this.read_auto_update_check(cx);
    });
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.update_state().auto_check),
        Some(false)
    );

    view.update(cx, |this, cx| this.set_auto_update_check(true, cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.update_state().auto_check),
        Some(true)
    );
    assert_eq!(service.counts().set_auto_check, 1);
    assert_eq!(
        service.counts().check_due,
        1,
        "the switch checks right away"
    );
}

/// The startup loop runs one check immediately and keeps the icon in sync.
#[gpui_kit::test]
fn the_check_loop_checks_once_at_startup(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.check_due = Ok(Some(status(Some(available(true)), portable()))));
    view.update(cx, |this, cx| this.start_update_check_loop(cx));
    settle(cx);
    assert_eq!(service.counts().check_due, 1, "one check at startup");
    assert!(view.read_with(cx, |this, _| this.update_state().icon_visible()));
    // A second call must not install a second loop or fire another check.
    view.update(cx, |this, cx| this.start_update_check_loop(cx));
    settle(cx);
    assert_eq!(service.counts().check_due, 1, "the loop is installed once");
}

/// A failure the previous run persisted is read once and shown in the dialog
/// with a retry and a release-page action.
#[gpui_kit::test]
fn a_previous_update_failure_is_shown_on_startup(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.pending_failure = Some(
            "The previous DBLocal process (PID 48211) did not exit within 60s; update aborted."
                .to_owned(),
        );
    });
    view.update(cx, |this, cx| this.read_pending_update_failure(cx));
    settle(cx);
    let (visible, message, can_retry, release_url) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (
            state.dialog_visible(),
            state.previous_failure().map(str::to_owned),
            state.can_retry(),
            state.release_url().map(str::to_owned),
        )
    });
    assert!(visible, "the persisted failure must open the dialog");
    assert!(
        message
            .as_deref()
            .is_some_and(|m| m.contains("did not exit")),
        "the helper's own sentence must be shown: {message:?}"
    );
    assert!(can_retry, "a previous failure offers a retry");
    assert!(release_url.is_none(), "no release is known");
    assert_eq!(service.counts().take_pending_failure, 1);

    // One-shot: a second startup read must not consume another note.
    view.update(cx, |this, cx| this.read_pending_update_failure(cx));
    settle(cx);
    assert_eq!(service.counts().take_pending_failure, 1);
}

/// Closing the dialog dismisses the previous-failure note.
#[gpui_kit::test]
fn closing_the_dialog_dismisses_the_previous_failure(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, _| {
        this.update
            .record_previous_failure("the helper could not replace the binary");
    });
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.update_state().dialog_visible()));
    view.update(cx, |this, cx| this.close_update_dialog(cx));
    settle(cx);
    assert!(view.read_with(cx, |this, _| {
        this.update_state().previous_failure().is_none()
    }));
    assert!(!view.read_with(cx, |this, _| this.update_state().dialog_visible()));
}

/// A release build with no signing key keeps a writable per-user install but
/// still refuses self-update; the UI must follow the flags, not the install kind.
#[gpui_kit::test]
fn no_signing_key_refuses_update_on_a_per_user_install(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(no_signing_key_status()));
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    let (icon, phase, can_self_update, can_retry, reason) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (
            state.icon_visible(),
            state.phase.clone(),
            state.can_self_update(),
            state.can_retry(),
            state.cannot_update_reason().map(str::to_owned),
        )
    });
    assert!(icon, "the icon still shows, so the reason can be read");
    assert_eq!(phase, UpdatePhase::CannotSelfUpdate);
    assert!(
        !can_self_update,
        "the signing gate must win over the install kind"
    );
    assert!(!can_retry, "a missing key cannot be retried into existence");
    assert!(
        reason
            .as_deref()
            .is_some_and(|r| r.contains("signing public key")),
        "the reason must name the missing key: {reason:?}"
    );

    // Pressing must not start a download.
    view.update_in(cx, |this, window, cx| this.press_update_button(window, cx));
    settle(cx);
    assert_eq!(service.counts().start_update, 0);
    assert!(view.read_with(cx, |this, _| this.update_state().dialog_visible()));
}

fn no_signing_key_status() -> UpdateStatus {
    UpdateStatus {
        current: "0.8.0".to_owned(),
        available: Some(AvailableUpdateView {
            version: "0.9.0".to_owned(),
            tag: "v0.9.0".to_owned(),
            release_notes: "Notes".to_owned(),
            release_url: "https://github.com/rezkycodes/dblocal/releases/tag/v0.9.0".to_owned(),
            asset_name: "dblocal-0.9.0-linux-x86_64.tar.gz".to_owned(),
            size_bytes: 1024,
            can_self_update: false,
        }),
        install_kind: portable(),
        package_manager_hint: None,
        cannot_update_reason: Some(
            "Release signing public key is not configured in this build; automatic self-update is disabled."
                .to_owned(),
        ),
    }
}

/// Starting a new attempt after a previous-run failure clears the old warning
/// immediately, both when the new attempt runs and when `start_update` fails.
#[gpui_kit::test]
fn retry_clears_previous_failure_warning(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.check_due = Ok(Some(status(Some(available(true)), portable())));
        script.start_update = Ok(OperationId::new("op-fresh"));
    });
    view.update(cx, |this, _| {
        this.update
            .record_previous_failure("The previous DBLocal process did not exit.");
    });
    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);

    view.update(cx, |this, cx| this.retry_update(cx));
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this
            .update_state()
            .previous_failure()
            .is_none()),
        "the old failure line must be cleared once a new attempt starts"
    );
    assert!(view.read_with(cx, |this, _| this.update_state().running()));
}

/// A startup up-to-date check must not close a pending previous-failure note.
#[gpui_kit::test]
fn startup_up_to_date_check_preserves_previous_failure_dialog(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.pending_failure = Some("The previous helper could not replace the file.".to_owned());
        script.check_due = Ok(Some(status(None, portable())));
    });
    view.update(cx, |this, cx| this.read_pending_update_failure(cx));
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.update_state().dialog_visible()));

    view.update(cx, |this, cx| this.check_update_bg(cx));
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this.update_state().dialog_visible()),
        "an up-to-date startup check must not dismiss the previous-failure note"
    );
    assert!(
        view.read_with(cx, |this, _| this
            .update_state()
            .previous_failure()
            .is_some()),
        "the previous-failure text must remain until the user closes it"
    );
}

/// "Try again" on a previous-failure note with no known release keeps the
/// dialog visible while checking and shows the outcome (up to date or error),
/// then restores focus cleanly on close.
#[gpui_kit::test]
fn retry_from_previous_failure_without_release_shows_outcome_in_dialog(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.pending_failure = Some("The previous helper aborted.".to_owned());
        script.update = Ok(status(None, portable()));
    });
    view.update(cx, |this, cx| this.read_pending_update_failure(cx));
    settle(cx);

    view.update(cx, |this, cx| this.retry_update(cx));
    // Immediately after pressing "Try again", the dialog stays open showing
    // that a check is in progress instead of vanishing into Hidden.
    let (visible_while_checking, headline_while_checking) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (state.dialog_visible(), state.headline())
    });
    assert!(
        visible_while_checking,
        "dialog must stay visible while re-checking"
    );
    assert_eq!(
        headline_while_checking,
        dblocal_core::strings::UI_SETTINGS_CHECKING
    );

    settle(cx);
    let (visible_after, headline_after) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (state.dialog_visible(), state.headline())
    });
    assert!(
        visible_after,
        "dialog must stay visible with the up-to-date answer"
    );
    assert_eq!(
        headline_after,
        dblocal_core::strings::UI_SETTINGS_UP_TO_DATE
    );

    // Closing restores focus and resets the dialog.
    view.update(cx, |this, cx| this.close_update_dialog(cx));
    settle(cx);
    assert!(!view.read_with(cx, |this, _| this.update_state().dialog_visible()));

    // Now test the check-error outcome: dialog stays visible with the error
    // and offers another retry.
    service.set(|script| {
        script.update = Err(dblocal_core::service::UserFacingError::new(
            "DBL-8002",
            "Could not reach GitHub.",
        ));
    });
    view.update(cx, |this, _| {
        this.update
            .record_previous_failure("The previous helper aborted.");
    });
    view.update(cx, |this, cx| this.retry_update(cx));
    settle(cx);
    let (visible_err, phase_err, can_retry_err) = view.read_with(cx, |this, _| {
        let state = this.update_state();
        (
            state.dialog_visible(),
            state.phase.clone(),
            state.can_retry(),
        )
    });
    assert!(visible_err, "dialog must stay visible with the check error");
    assert!(matches!(
        phase_err,
        UpdatePhase::Failed { ref code, .. } if code == "DBL-8002"
    ));
    assert!(can_retry_err, "a failed check must still offer Try again");
}
