//! Handler tests for the instance and form actions.
//!
//! They run on [`QuietService`](crate::app::test_service::QuietService), which
//! answers inline and never emits: `MockService` completes on tokio threads and
//! gpui's deterministic test scheduler rejects the cross-thread wake.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::model::{EngineKey, InstanceStatus};
use dblocal_core::service::{DblService, UriFormat};
use dblocal_core::strings;
use gpui_kit::TestAppContext;

use crate::app::test_service::{QuietService, SECRET};
use crate::app::{InnerTab, Workspace};

/// A workspace on the emit-free service, plus a handle to that service.
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

#[gpui_kit::test]
fn open_tab_loads_the_uri_masked(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();

    let text = view.read_with(cx, |this, _| this.uri_text(&id));
    assert!(
        !text.is_empty(),
        "open_tab must fetch the URI in the background"
    );
    assert!(
        text.contains(crate::format::SECRET_MASK),
        "the fetched URI must render masked: {text}"
    );
    assert!(!text.contains(SECRET), "the card leaked the secret: {text}");
    assert_eq!(view.read_with(cx, |this, _| this.tabs.len()), 1);
}

#[gpui_kit::test]
fn start_and_stop_report_the_status_toast(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);

    view.update(cx, |this, cx| this.start_instance(id.clone(), cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_START_REQUESTED.to_owned())
    );

    view.update(cx, |this, cx| this.stop_instance(id.clone(), cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_STOP_REQUESTED.to_owned())
    );
}

#[gpui_kit::test]
fn copy_uri_writes_the_plaintext_to_the_clipboard(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update_in(cx, |this, window, cx| {
        this.copy_uri(id.clone(), UriFormat::Uri, window, cx)
    });
    cx.run_until_parked();

    let text = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        text.contains("://"),
        "the clipboard must hold the URI: {text}"
    );
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_URI_COPIED.to_owned())
    );
}

#[gpui_kit::test]
fn reveal_shows_the_plaintext_and_hide_restores_the_mask(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();

    view.update(cx, |this, cx| this.toggle_reveal(&id, cx));
    cx.run_until_parked();
    let revealed = view.read_with(cx, |this, _| this.uri_revealed(&id));
    let text = view.read_with(cx, |this, _| this.uri_text(&id));
    // The URI carries the percent-encoded password, so the raw secret is
    // not a substring of it; the encoded form is what must appear.
    let encoded = dblocal_core::engine::uri::encode_userinfo(SECRET);
    assert!(revealed, "reveal did not flag the card");
    assert!(
        text.contains(&encoded) || text.contains(SECRET),
        "reveal hid the plaintext: {text}"
    );

    view.update(cx, |this, cx| this.toggle_reveal(&id, cx));
    cx.run_until_parked();
    let text = view.read_with(cx, |this, _| this.uri_text(&id));
    assert!(!view.read_with(cx, |this, _| this.uri_revealed(&id)));
    let encoded = dblocal_core::engine::uri::encode_userinfo(SECRET);
    assert!(
        text.contains(crate::format::SECRET_MASK),
        "hide left it revealed: {text}"
    );
    assert!(!text.contains(&encoded) && !text.contains(SECRET));
}

#[gpui_kit::test]
fn reveal_sync_uses_the_cached_secret_and_never_calls_the_service(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    let before = service.calls();

    view.update(cx, |this, _| this.toggle_reveal_sync(&id));
    assert!(
        view.read_with(cx, |this, _| this.uri_revealed(&id)),
        "toggle_reveal_sync must reveal from the cached URI"
    );
    view.update(cx, |this, _| this.toggle_reveal_sync(&id));
    assert!(!view.read_with(cx, |this, _| this.uri_revealed(&id)));
    assert_eq!(
        service.calls(),
        before,
        "toggle_reveal_sync must not call the service from the GPUI thread"
    );
}

#[gpui_kit::test]
fn select_and_inner_tab_load_the_log_tail(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();

    view.update(cx, |this, cx| this.select_tab(0, cx));
    view.update(cx, |this, cx| this.set_inner_tab(InnerTab::Logs, cx));
    cx.run_until_parked();
    let lines = view.read_with(cx, |this, _| this.log_lines(&id).len());
    assert!(lines > 0, "the Logs tab never loaded its tail");
}

#[gpui_kit::test]
fn close_tab_drops_the_buffer_and_uri(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.close_tab(0, cx));
    cx.run_until_parked();

    assert_eq!(view.read_with(cx, |this, _| this.tabs.len()), 0);
    assert!(view.read_with(cx, |this, _| this.log_lines(&id).is_empty()));
    assert!(view.read_with(cx, |this, _| this.uri_text(&id).is_empty()));
}

#[gpui_kit::test]
fn delete_requires_the_name_before_purging(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, _| {
        this.delete_target = Some(id.clone());
        this.delete_mode = dblocal_core::model::DeleteMode::PurgeData;
        this.delete_typed_name = Some("wrong".to_owned());
    });
    assert!(!view.read_with(cx, |this, cx| this.delete_confirmation_ok(cx)));

    let name = view.read_with(cx, |this, _| {
        this.instance(&id).expect("instance").name.clone()
    });
    view.update(cx, |this, _| this.delete_typed_name = Some(name));
    assert!(view.read_with(cx, |this, cx| this.delete_confirmation_ok(cx)));
}

#[gpui_kit::test]
fn submit_create_pushes_a_download_and_closes_the_form(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_create_dialog(window, cx);
        // An empty catalog leaves the version blank; any version string is
        // accepted by InstanceSpec, and the service is the fake.
        if let Some(form) = this.create.as_mut() {
            form.version = "17.4".to_owned();
        }
        this.set_create_field_texts("pg-new", "5500", window, cx);
        this.submit_create(window, cx);
    });
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_none()),
        "a valid submit must close the form"
    );
    assert!(
        view.read_with(cx, |this, _| !this.downloads().is_empty()),
        "a valid submit must add a download row"
    );
}

#[gpui_kit::test]
fn submit_create_reopens_the_form_with_the_validation_message(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_create_dialog(window, cx);
        // An empty name is rejected by validate_form, before the service.
        this.set_create_field_texts("", "", window, cx);
        this.submit_create(window, cx);
    });
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_some()),
        "an invalid submit must keep the form open"
    );
    assert!(
        view.read_with(cx, |this, _| this.toast_text().is_some()),
        "an invalid submit must explain the problem"
    );
}

#[gpui_kit::test]
fn choose_auth_and_cancel_shape_the_form(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_create_dialog(window, cx);
        this.choose_auth(dblocal_core::model::AuthMode::None);
    });
    assert_eq!(
        view.read_with(cx, |this, _| this.create_form().map(|f| f.auth)),
        Some(dblocal_core::model::AuthMode::None)
    );
    view.update(cx, |this, _| this.cancel_create());
    assert!(view.read_with(cx, |this, _| this.create_form().is_none()));
}

#[gpui_kit::test]
fn open_delete_dialog_reads_the_size_in_the_background(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update_in(cx, |this, window, cx| {
        this.open_delete_dialog(id.clone(), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.delete_size()),
        4096,
        "the dialog must show the size the service reported"
    );
    assert!(view.read_with(cx, |this, _| this.delete_name_input().is_some()));
    assert_eq!(
        view.read_with(cx, |this, _| this.delete_mode()),
        dblocal_core::model::DeleteMode::ArchiveData
    );
}

#[gpui_kit::test]
fn autostart_reports_and_refreshes(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.set_autostart(&id, true, cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_AUTOSTART_ON.to_owned())
    );
    view.update(cx, |this, cx| this.set_autostart(&id, false, cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_AUTOSTART_OFF.to_owned())
    );
}

#[gpui_kit::test]
fn toggle_follow_touches_only_local_state(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| {
        this.open_tab(id.clone(), cx);
        this.set_inner_tab(InnerTab::Logs, cx);
        // The tail arrives from the background executor; wait for it so the
        // buffer exists before follow is toggled.
        this.load_logs_sync(&id);
    });
    cx.run_until_parked();
    // A fresh buffer follows, so the first toggle turns it off and the
    // second turns it back on.
    assert!(view.read_with(cx, |this, _| this.log_lines(&id).is_following()));
    view.update(cx, |this, _| this.toggle_follow());
    assert!(!view.read_with(cx, |this, _| this.log_lines(&id).is_following()));
    view.update(cx, |this, _| this.toggle_follow());
    assert!(view.read_with(cx, |this, _| this.log_lines(&id).is_following()));
}

#[gpui_kit::test]
fn toggle_run_selected_starts_a_stopped_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.toggle_run_selected(cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_START_REQUESTED.to_owned())
    );
}

/// The preview constructor's synchronous tab open fills every cache the
/// Overview card reads from, and it must not need a window tick.
#[gpui_kit::test]
fn open_tab_sync_fills_every_cache(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update(cx, |this, _| this.open_tab_sync(id.clone()));

    let (masked, size, dotenv, port) = view.read_with(cx, |this, _| {
        (
            this.uri_text(&id),
            this.data_size(&id),
            this.dotenv_rows(&id).len(),
            this.instance_port_proposal(&id),
        )
    });
    assert!(
        masked.contains(crate::format::SECRET_MASK),
        "URI not masked: {masked}"
    );
    assert_eq!(size, Some(4096));
    assert!(dotenv > 0, "the .env block was not cached");
    assert_eq!(port, 5433, "the port suggestion was not cached");
}

#[gpui_kit::test]
fn choose_engine_switches_the_form(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_create_dialog(window, cx);
        this.choose_engine(EngineKey::Mysql, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.create_form().map(|f| f.engine)),
        Some(EngineKey::Mysql)
    );
}

#[gpui_kit::test]
fn purge_refuses_without_the_typed_name(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    view.update_in(cx, |this, window, cx| {
        this.open_delete_dialog(id.clone(), window, cx);
        this.set_delete_mode(dblocal_core::model::DeleteMode::PurgeData);
        this.delete_typed_name = Some("wrong".to_owned());
        this.confirm_delete(cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_DELETE_TYPE_NAME.to_owned())
    );
    assert!(
        view.read_with(cx, |this, _| this.delete_target.is_some()),
        "the dialog stays open after a refusal"
    );
}

#[gpui_kit::test]
fn purge_with_the_right_name_deletes_and_closes(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    let name = view.read_with(cx, |this, _| {
        this.instance(&id).expect("instance").name.clone()
    });
    view.update_in(cx, |this, window, cx| {
        this.open_delete_dialog(id.clone(), window, cx);
        this.set_delete_mode(dblocal_core::model::DeleteMode::PurgeData);
        // The dialog owns an InputState; it takes precedence over the mirror.
        if let Some(input) = this.delete_name_input.clone() {
            input.update(cx, |state, cx| state.set_value(name.clone(), window, cx));
        }
        this.delete_typed_name = Some(name.clone());
        this.confirm_delete(cx);
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(strings::UI_TOAST_DELETED.to_owned())
    );
    assert!(
        view.read_with(cx, |this, _| this.delete_target.is_none()),
        "a completed delete must close the dialog"
    );
}

#[gpui_kit::test]
fn choose_version_updates_the_form_and_the_suggested_name(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_create_dialog(window, cx);
        this.choose_version("17.5".to_owned(), window, cx);
    });
    assert_eq!(
        view.read_with(cx, |this, _| {
            this.create_form().map(|f| f.version.clone())
        }),
        Some("17.5".to_owned())
    );
}

/// The command palette: opening, filtering, and running every action kind.
#[gpui_kit::test]
fn palette_actions_reach_the_same_handlers(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    assert!(!view.read_with(cx, |this, _| this.palette_is_open()));
    view.update(cx, |this, _| this.toggle_palette());
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    let labels = view.read_with(cx, |this, _| this.palette_labels());
    assert!(
        labels.iter().any(|l| l.contains("New instance")),
        "{labels:?}"
    );

    let actions = view.read_with(cx, |this, _| this.palette_actions());
    assert!(!actions.is_empty());
    // Run the New-instance entry (index 0 in the unfiltered list).
    let new_index = actions
        .iter()
        .position(|a| matches!(a, Some(crate::format::PaletteAction::NewInstance)))
        .expect("new-instance entry");
    view.update_in(cx, |this, window, cx| {
        this.run_palette_action(new_index, window, cx)
    });
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_some()),
        "the palette must open the create dialog"
    );
    assert!(!view.read_with(cx, |this, _| this.palette_is_open()));
    view.update(cx, |this, _| this.cancel_create());

    // Start, stop, open and copy each route to their handler. The list now
    // carries disabled rows too (`None`), so only the runnable ones run. The
    // delete row opens a modal, which another test drives; skip it here so this
    // walk covers the direct-service actions.
    let id = service.id_with(InstanceStatus::Stopped);
    let entries = view.read_with(cx, |this, _| this.palette_actions());
    for (index, action) in entries.iter().enumerate() {
        match action {
            Some(
                crate::format::PaletteAction::Start(value)
                | crate::format::PaletteAction::Stop(value)
                | crate::format::PaletteAction::Open(value)
                | crate::format::PaletteAction::CopyUri(value),
            ) => {
                if value == &id.to_string() {
                    view.update_in(cx, |this, window, cx| {
                        this.run_palette_action(index, window, cx)
                    });
                    cx.run_until_parked();
                }
            }
            Some(
                crate::format::PaletteAction::NewInstance | crate::format::PaletteAction::Delete(_),
            )
            | None => {}
        }
    }
    assert!(
        service.calls() > 0,
        "the palette actions must reach the service"
    );
    // An out-of-range index closes the palette without a panic.
    view.update(cx, |this, _| this.toggle_palette());
    view.update_in(cx, |this, window, cx| {
        this.run_palette_action(999, window, cx)
    });
    assert!(!view.read_with(cx, |this, _| this.palette_is_open()));
}

/// The thin action wrappers the dialogs and docks call.
#[gpui_kit::test]
fn dialog_action_wrappers_route_to_their_handlers(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);

    // demo_content / set_palette_open / close_modals_action.
    assert!(!view.read_with(cx, |this, _| this.demo_content()));
    view.update(cx, |this, _| this.set_palette_open(true));
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    view.update(cx, |this, _| this.close_modals_action());
    assert!(!view.read_with(cx, |this, _| this.palette_is_open()));

    // Create dialog wrappers: open, choose engine/version/auth, cancel.
    view.update_in(cx, |this, window, cx| {
        this.open_new_instance(window, cx);
        this.choose_engine_action(EngineKey::Mysql, window, cx);
        this.choose_version_action("8.0".to_owned(), window, cx);
        this.choose_auth_action(dblocal_core::model::AuthMode::None);
    });
    cx.run_until_parked();
    let form = view
        .read_with(cx, |this, _| this.create_form().cloned())
        .expect("the create dialog is open");
    assert_eq!(form.engine, EngineKey::Mysql);
    assert_eq!(form.version, "8.0");
    assert_eq!(form.auth, dblocal_core::model::AuthMode::None);
    view.update(cx, |this, _| this.cancel_create_action());
    assert!(view.read_with(cx, |this, _| this.create_form().is_none()));

    // submit_create_action with an empty name is rejected and re-opens the form.
    view.update_in(cx, |this, window, cx| {
        this.open_new_instance(window, cx);
        if let Some(form) = this.create.as_mut() {
            form.version = "17.4".to_owned();
        }
        this.set_create_field_texts("", "", window, cx);
        this.submit_create_action(window, cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |this, _| this.create_form().is_some()));
    assert!(view.read_with(cx, |this, _| this.toast_text().is_some()));
    view.update(cx, |this, _| this.cancel_create_action());

    // The delete dialog wrappers: cancel by closing every modal.
    view.update_in(cx, |this, window, cx| {
        this.open_delete_dialog(id.clone(), window, cx)
    });
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, cx| this.delete_target_name(cx))
            .is_some()
    );
    view.update(cx, |this, _| this.close_modals_action());
    assert!(
        view.read_with(cx, |this, cx| this.delete_target_name(cx))
            .is_none()
    );
}

/// `move_to_port_and_start` validates the port and then changes and starts.
#[gpui_kit::test]
fn move_to_port_and_start_rejects_a_privileged_port(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Failed);
    view.update(cx, |this, cx| this.move_to_port_and_start(&id, 80, cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.toast_text())
            .as_deref()
            .map(ToString::to_string),
        Some(dblocal_core::strings::UI_VALIDATE_PORT_LOW.to_owned())
    );
    let before = service.calls();
    view.update(cx, |this, cx| this.move_to_port_and_start(&id, 5500, cx));
    cx.run_until_parked();
    assert!(
        service.calls() > before,
        "a valid port must reach the service"
    );
}

/// The dock's engine groups collapse and expand independently.
#[gpui_kit::test]
fn engine_groups_collapse_and_expand(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    assert!(!view.read_with(cx, |this, _| this.is_collapsed(EngineKey::Postgresql)));
    view.update(cx, |this, _| this.toggle_collapsed(EngineKey::Postgresql));
    assert!(view.read_with(cx, |this, _| this.is_collapsed(EngineKey::Postgresql)));
    assert!(!view.read_with(cx, |this, _| this.is_collapsed(EngineKey::Mysql)));
    view.update(cx, |this, _| this.toggle_collapsed(EngineKey::Postgresql));
    assert!(!view.read_with(cx, |this, _| this.is_collapsed(EngineKey::Postgresql)));
}

/// Unparsable palette values are ignored rather than panicking.
#[gpui_kit::test]
fn palette_actions_with_bad_ids_are_ignored(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let before = service.calls();
    // The local query filter lives in `filter_palette` (the palette's
    // `CommandState` filters locally); a query that matches nothing leaves no
    // rows to confirm.
    let items = view.read_with(cx, |this, _| this.palette_items());
    assert!(
        crate::format::filter_palette(&items, "zzz-does-not-exist").is_empty(),
        "no instance matches the query"
    );
    view.update(cx, |this, _| this.palette_open = true);
    // An out-of-range confirm runs nothing and closes the palette.
    view.update_in(cx, |this, window, cx| {
        this.run_palette_action(9_999, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(service.calls(), before, "a bad index must reach no service");
    assert!(!view.read_with(cx, |this, _| this.palette_is_open()));
}
