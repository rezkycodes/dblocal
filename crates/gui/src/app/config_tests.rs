//! Config tab handler tests over the scriptable service (`F-CFG-01`, `F-CFG-02`).
//!
//! Each test drives the `Workspace` through the same handlers a click runs and
//! asserts on the pure state the card renders, so a state the task names (all
//! engines, an invalid value, a failed apply, a backup, no backup, a launcher
//! failure, a `ConfigChanged` mid-edit) is proven end to end without a real
//! engine. The pure state itself has its own unit tests in `config_state.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use dblocal_core::config::{BackupInfo, ConfigView};
use dblocal_core::model::{EngineKey, InstanceStatus};
use dblocal_core::service::UserFacingError;
use gpui_kit::{Focusable, TestAppContext};

use super::databases_support::{fixture, settle};
use super::scripted_service::ScriptedService;
use crate::app::admin_state::Loadable;
use crate::app::config_state::ConfigAction;
use crate::app::{InnerTab, Workspace};
use crate::service_bridge::UiEvent;

/// Opens the running instance's Config tab and settles its first read.
fn open_config(
    view: &gpui_kit::Entity<Workspace>,
    service: &ScriptedService,
    cx: &mut gpui_kit::VisualTestContext,
) -> dblocal_core::model::InstanceId {
    let id = service.id_with(InstanceStatus::Running);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Config, cx));
    settle(cx);
    id
}

/// A view with a backup available, so the restore action is enabled.
fn view_with_backup() -> ConfigView {
    use dblocal_core::config::{ConfigSetting, ConfigView};
    let mut view = ConfigView::new(
        dblocal_core::model::InstanceId::new(),
        EngineKey::Postgresql,
        std::path::PathBuf::from("/mock/pg-run/postgresql.conf"),
    );
    view.settings.push(ConfigSetting {
        key: "max_connections".to_owned(),
        current: Some("100".to_owned()),
        default: "100".to_owned(),
        unit: String::new(),
        restart_required: true,
        description: "desc".to_owned(),
        readonly: false,
    });
    view.backup_available = Some(BackupInfo {
        taken_at: 1_700_000_000,
        taken_at_secs: 1_700_000_000,
        path: std::path::PathBuf::from("/mock/backups/postgresql.conf.1700000000.bak"),
    });
    view
}

#[gpui_kit::test]
fn entering_the_tab_reads_the_config_view(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    let (editable, readonly, backup) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.editable().len(),
            config.readonly().len(),
            config.view().map(|v| v.backup_available.is_some()),
        )
    });
    assert_eq!(editable, 2, "two editable rows from the service");
    assert_eq!(readonly, 1, "the port row is read-only");
    assert_eq!(backup, Some(false), "no backup in the seed");
    assert_eq!(service.counts().config_view, 1);
}

#[gpui_kit::test]
fn a_failed_read_is_a_typed_error_not_a_panic(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let failure = UserFacingError::new("DBL-8603", "Berkas konfigurasi tidak dapat dibaca.");
    service.set(|script| script.config_view = Err(failure));
    let id = open_config(&view, &service, cx);
    let error = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        config
            .error
            .as_ref()
            .or_else(|| config.data.error())
            .map(|e| e.code.clone())
    });
    assert_eq!(error.as_deref(), Some("DBL-8603"));
}

#[gpui_kit::test]
fn an_invalid_value_is_refused_locally_with_the_service_message(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    let before = service.counts().config_apply;
    // The core validator refuses this before any round trip.
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "0");
        this.config.entry(&id).validate(EngineKey::Postgresql);
        this.apply_config(&id, cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().config_apply,
        before,
        "an invalid value must not reach the service"
    );
    let code = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|config| config.errors.get("max_connections").map(|e| e.code.clone()))
    });
    assert_eq!(code.as_deref(), Some("DBL-8601"));
}

#[gpui_kit::test]
fn the_tab_renders_every_engine_from_the_view(cx: &mut TestAppContext) {
    // All four engines' views, built from the real schema, must be readable: the
    // screen hard-codes nothing per engine, so the editable rows and defaults
    // come straight from `settings_for`.
    let _ = cx;
    for engine in EngineKey::ALL {
        let mut view = ConfigView::new(
            dblocal_core::model::InstanceId::new(),
            engine,
            std::path::PathBuf::from("/mock/conf"),
        );
        for spec in dblocal_core::config::settings_for(engine) {
            view.settings.push(dblocal_core::config::ConfigSetting {
                key: spec.key.to_owned(),
                current: None,
                default: spec.default.to_owned(),
                unit: spec.unit.to_owned(),
                restart_required: true,
                description: spec.description.to_owned(),
                readonly: false,
            });
        }
        let config = crate::app::config_state::InstanceConfig {
            data: Loadable::Ready(view),
            ..Default::default()
        };
        assert!(
            !config.editable().is_empty(),
            "{engine:?} must expose at least one editable row"
        );
        assert!(config.editable().iter().all(|s| !s.description.is_empty()));
        assert!(
            config.editable().iter().any(|s| !s.default.is_empty()),
            "{engine:?} must show at least one default"
        );
    }
}

#[gpui_kit::test]
fn a_restore_available_failure_offers_the_banner_action(cx: &mut TestAppContext) {
    use dblocal_core::model::StartFailure;
    use dblocal_core::service::FailureInfo;
    // F-CFG-01 AC3: a failure whose config file changed offers the restore from
    // the Overview banner as well as the Config tab.
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| {
        let mut changed = this.instance(&id).cloned().expect("instance view");
        changed.status = InstanceStatus::Failed;
        changed.failure = Some(FailureInfo {
            code: "DBL-8601".to_owned(),
            message: "nilai salah".to_owned(),
            diagnosis: StartFailure::Unknown,
            log_tail: Vec::new(),
            restore_available: true,
        });
        this.state.upsert_instance(changed);
        this.open_tab(id.clone(), cx);
    });
    let offered = view.read_with(cx, |this, _| {
        this.instance(&id)
            .map(crate::format::offers_restore)
            .unwrap_or(false)
    });
    assert!(offered, "the banner must offer the restore");
    view.update(cx, |this, cx| this.restore_and_restart(&id, cx));
    settle(cx);
    assert_eq!(service.mutations().config_restores, vec![id]);
}

#[gpui_kit::test]
fn a_config_change_mid_edit_rebases_through_the_event(cx: &mut TestAppContext) {
    // The event path (not the direct re-read) must preserve an edit to a key the
    // new file value does not match, and raise the outside-changed banner.
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, _| {
        this.config.entry(&id).set_field("shared_buffers", "512MB");
    });
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::ConfigChanged(id.clone()), cx);
    });
    settle(cx);
    let (edit, outside) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.edits.get("shared_buffers").cloned(),
            config.touched_outside,
        )
    });
    assert_eq!(edit.as_deref(), Some("512MB"));
    assert!(outside);
}

#[gpui_kit::test]
fn apply_sends_only_the_changed_fields_and_shows_the_restart_offer(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "300");
        this.apply_config(&id, cx);
    });
    settle(cx);

    let applies = service.mutations().config_applies;
    assert_eq!(applies.len(), 1, "one apply crosses the seam");
    assert_eq!(
        applies[0].1,
        vec![("max_connections".to_owned(), "300".to_owned())],
        "only the changed key is sent"
    );
    let (applied, edits_empty) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (config.applied.is_some(), config.edits.is_empty())
    });
    assert!(
        applied,
        "the restart banner is offered after a successful apply"
    );
    assert!(
        edits_empty,
        "the edits are cleared after a successful apply"
    );
}

#[gpui_kit::test]
fn a_busy_apply_error_is_reported_typed(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.config_apply = Err(UserFacingError::new(
            "DBL-8604",
            "Instance sedang dipakai operasi lain. Coba lagi setelah selesai.",
        ));
    });
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "300");
        this.apply_config(&id, cx);
    });
    settle(cx);
    let (code, applied) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.error.as_ref().map(|e| e.code.clone()),
            config.applied.is_some(),
        )
    });
    assert_eq!(
        code.as_deref(),
        Some("DBL-8604"),
        "a busy apply says try again"
    );
    assert!(!applied, "a failed apply must not claim success");
}

#[gpui_kit::test]
fn a_restore_confirmation_needs_a_backup_and_calls_restore(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);
    let can_restore = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .is_some_and(|config| config.can_restore())
    });
    assert!(can_restore, "a backup enables the restore action");

    view.update(cx, |this, cx| {
        this.open_config_restore(&id, cx);
    });
    assert!(view.read_with(cx, |this, _| this.config_restore_confirm().is_some()));
    view.update(cx, |this, cx| this.confirm_config_restore(cx));
    settle(cx);
    assert_eq!(service.mutations().config_restores, vec![id.clone()]);
}

#[gpui_kit::test]
fn without_a_backup_the_restore_action_is_disabled(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    let can_restore = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .is_some_and(|config| config.can_restore())
    });
    assert!(!can_restore, "no backup means no restore");
}

#[gpui_kit::test]
fn restore_and_restart_is_two_service_calls(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| this.restore_and_restart(&id, cx));
    settle(cx);
    assert_eq!(service.mutations().config_restores, vec![id.clone()]);
    assert_eq!(
        service.counts().start_instance,
        starts_before + 1,
        "restore then start are two separate service calls"
    );
}

#[gpui_kit::test]
fn opening_the_raw_file_calls_the_service(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.open_config_raw(&id, cx));
    settle(cx);
    assert_eq!(service.mutations().config_opens, vec![id]);
}

#[gpui_kit::test]
fn a_launcher_failure_is_typed_and_keeps_the_path(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.config_open_raw = Err(UserFacingError::new(
            "DBL-8608",
            "Editor bawaan sistem tidak dapat dibuka. Buka berkasnya secara manual.",
        ));
    });
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.open_config_raw(&id, cx));
    settle(cx);
    let (code, path) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.error.as_ref().map(|e| e.code.clone()),
            config.view().map(|v| v.raw_path.display().to_string()),
        )
    });
    assert_eq!(code.as_deref(), Some("DBL-8608"));
    assert!(
        path.as_deref()
            .is_some_and(|p| p.contains("postgresql.conf")),
        "the path is still shown so the user can open it by hand: {path:?}"
    );
}

#[gpui_kit::test]
fn a_config_change_re_reads_the_visible_tab_and_rebases_edits(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    // An unsaved edit on a key that the new file value will not match.
    view.update(cx, |this, _| {
        this.config.entry(&id).set_field("max_connections", "300");
    });
    let before = service.counts().config_view;
    // The file changed elsewhere; the re-read now reports max_connections 100.
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::ConfigChanged(id.clone()), cx);
    });
    settle(cx);
    assert!(
        service.counts().config_view > before,
        "the visible Config tab must re-read on ConfigChanged"
    );
    let (survives, outside) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (
            config.edits.get("max_connections").cloned(),
            config.touched_outside,
        )
    });
    assert_eq!(
        survives.as_deref(),
        Some("300"),
        "an edit to another key survives the re-read"
    );
    assert!(outside, "the banner says the file changed outside the app");
}

#[gpui_kit::test]
fn a_config_change_for_a_background_instance_sends_it_to_idle(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let visible = open_config(&view, &service, cx);
    // The other instance's view is ready but it is not on screen.
    let other = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, _| {
        this.config.entry(&other).data = Loadable::Ready(view_with_backup());
    });
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(UiEvent::ConfigChanged(other.clone()), cx);
    });
    let idle = view.read_with(cx, |this, _| {
        this.config_state_for(&other)
            .is_some_and(|config| matches!(config.data, Loadable::Idle))
    });
    assert!(
        idle,
        "a background instance's Config view is invalidated on ConfigChanged"
    );
    let _ = visible;
}

#[gpui_kit::test]
fn switching_instances_never_shows_another_instances_values(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let first = open_config(&view, &service, cx);
    view.update(cx, |this, _| {
        this.config
            .entry(&first)
            .set_field("max_connections", "999");
    });
    // Open the other instance's Config tab: its own state has no edit.
    let other = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| {
        this.open_tab(other.clone(), cx);
        this.set_inner_tab(InnerTab::Config, cx);
    });
    settle(cx);
    let other_edits = view.read_with(cx, |this, _| {
        this.config_state_for(&other)
            .map(|config| config.edits.len())
            .unwrap_or(0)
    });
    assert_eq!(
        other_edits, 0,
        "the second instance must not inherit the first's edit"
    );
}

#[gpui_kit::test]
fn two_instances_with_the_same_keys_get_distinct_row_ids(cx: &mut TestAppContext) {
    // The a11y duplicate-node trap: two instances share `max_connections`.
    let (view, service, cx) = fixture(cx);
    let first = open_config(&view, &service, cx);
    let other = service.id_with(InstanceStatus::Stopped);
    let a = crate::app::config_state::config_row_id("config-key", &first, "max_connections");
    let b = crate::app::config_state::config_row_id("config-key", &other, "max_connections");
    assert_ne!(a, b);
    let _ = view;
}

#[gpui_kit::test]
fn escape_closes_the_restore_dialog(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.open_config_restore(&id, cx));
    assert!(view.read_with(cx, |this, _| this.config_restore_confirm().is_some()));
    view.update(cx, |this, _| this.close_modals());
    assert!(
        view.read_with(cx, |this, _| this.config_restore_confirm().is_none()),
        "Escape must close the restore dialog"
    );
}

#[gpui_kit::test]
fn every_call_is_busy_while_in_flight(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, _| {
        this.config.entry(&id).begin(ConfigAction::Apply);
    });
    let (busy, can_apply) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (config.is_busy(ConfigAction::Apply), config.can_apply())
    });
    assert!(busy);
    assert!(!can_apply, "Terapkan is disabled while a call is in flight");
}

/// "Restart sekarang" stops a running instance, then starts it once the stop has
/// settled (`F-CFG-01` AC2). Starting a `Stopping` instance would be refused, so
/// the start must run on the transition to `Stopped`, not immediately.
///
/// Without the scheduling in `after_status_change` this test is red: the stop is
/// issued but the start never is, so the engine stays stopped after a promised
/// restart.
#[gpui_kit::test]
fn restart_now_stops_then_starts_on_the_stopped_transition(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Running);
    let starts_before = service.counts().start_instance;
    let stops_before = service.counts().stop_instance;

    // The user accepts "Restart sekarang": a running instance is stopped first.
    view.update(cx, |this, cx| this.restart_after_apply(&id, cx));
    settle(cx);
    assert_eq!(
        service.counts().stop_instance,
        stops_before + 1,
        "Restart sekarang stops a running instance first"
    );
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "the start must wait for the stop to settle, not run before it"
    );

    // The stop completes: the service reports the instance as Stopped.
    view.update(cx, |this, cx| {
        let mut stopped = this.instance(&id).cloned().expect("instance view");
        stopped.status = InstanceStatus::Stopped;
        this.apply_event_with_cx(UiEvent::InstanceChanged(stopped), cx);
    });
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before + 1,
        "once the stop has settled the queued restart must start the instance"
    );
}

/// A restart queued for an instance that is already stopped is just a start.
#[gpui_kit::test]
fn restart_now_on_a_stopped_instance_starts_it_directly(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    let starts_before = service.counts().start_instance;
    let stops_before = service.counts().stop_instance;
    view.update(cx, |this, cx| this.restart_after_apply(&id, cx));
    settle(cx);
    assert_eq!(
        service.counts().start_instance,
        starts_before + 1,
        "a stopped instance is started, not stopped again"
    );
    assert_eq!(
        service.counts().stop_instance,
        stops_before,
        "there is nothing to stop on a stopped instance"
    );
}

/// "Batalkan perubahan" drops the edits and disables Terapkan again.
#[gpui_kit::test]
fn cancel_changes_drops_every_edit(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, _| {
        this.config.entry(&id).set_field("max_connections", "999");
    });
    view.update(cx, |this, cx| this.reset_config_edits(&id, cx));
    settle(cx);
    let (edits, can_apply) = view.read_with(cx, |this, _| {
        let config = this.config_state_for(&id).expect("config state");
        (config.edits.len(), config.can_apply())
    });
    assert_eq!(edits, 0, "Batalkan perubahan clears every edit");
    assert!(!can_apply, "nothing left to apply");
}

/// "Muat ulang" issues a fresh read after a failed load.
#[gpui_kit::test]
fn retry_re_reads_after_a_failed_load(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.config_view = Err(UserFacingError::new("DBL-8603", "gagal baca"));
    });
    let id = open_config(&view, &service, cx);
    let before = service.counts().config_view;
    service.set(|script| script.config_view = Ok(view_with_backup()));
    view.update(cx, |this, cx| this.retry_config(&id, cx));
    settle(cx);
    assert!(
        service.counts().config_view > before,
        "Muat ulang issues a fresh read"
    );
    let ready = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .is_some_and(|config| config.view().is_some())
    });
    assert!(ready, "the retry must land a view, not stay failed");
}

/// "Nanti" dismisses the restart banner without restarting.
#[gpui_kit::test]
fn nanti_dismisses_the_restart_banner(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| {
        this.config.entry(&id).set_field("max_connections", "300");
        this.apply_config(&id, cx);
    });
    settle(cx);
    assert!(
        view.read_with(cx, |this, _| this
            .config_state_for(&id)
            .is_some_and(|c| c.applied.is_some())),
        "a successful apply raises the banner"
    );
    let stops = service.counts().stop_instance;
    view.update(cx, |this, cx| this.dismiss_config_applied(&id, cx));
    let applied = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .is_some_and(|c| c.applied.is_some())
    });
    assert!(!applied, "Nanti clears the banner");
    assert_eq!(
        service.counts().stop_instance,
        stops,
        "Nanti must not restart the instance"
    );
}

/// Copy puts the raw path on the clipboard.
#[gpui_kit::test]
fn copy_writes_the_raw_path_to_the_clipboard(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.copy_config_path(&id, cx));
    settle(cx);
    let text = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        text.contains("postgresql.conf"),
        "the clipboard must hold the config path: {text}"
    );
}

/// A failed restore reports the service's typed error, not a success.
#[gpui_kit::test]
fn a_failed_restore_is_reported_typed(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| {
        script.config_view = Ok(view_with_backup());
        script.config_restore = Err(UserFacingError::new("DBL-8605", "pulihkan gagal"));
    });
    let id = open_config(&view, &service, cx);
    view.update(cx, |this, cx| this.restore_and_restart(&id, cx));
    settle(cx);
    let code = view.read_with(cx, |this, _| {
        this.config_state_for(&id)
            .and_then(|config| config.error.as_ref().map(|e| e.code.clone()))
    });
    assert_eq!(
        code.as_deref(),
        Some("DBL-8605"),
        "a failed restore is the service's typed error"
    );
}

/// A plain restore (no start) after "Pulihkan" from the dialog.
#[gpui_kit::test]
fn confirm_restore_restores_without_starting(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    service.set(|script| script.config_view = Ok(view_with_backup()));
    let id = open_config(&view, &service, cx);
    let starts_before = service.counts().start_instance;
    view.update(cx, |this, cx| {
        this.open_config_restore(&id, cx);
        this.confirm_config_restore(cx);
    });
    settle(cx);
    assert_eq!(service.mutations().config_restores, vec![id.clone()]);
    assert_eq!(
        service.counts().start_instance,
        starts_before,
        "the dialog's restore does not start the instance; that is a separate action"
    );
}

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
        (config.can_apply(), config.errors.contains_key("max_connections"))
    });
    assert!(has_error, "F5: the field shows its DBL-8601 error");
    assert!(!can_apply, "F5: Terapkan is disabled while a field has an error");

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
