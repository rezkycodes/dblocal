//! The quit-flow glue tests (F-TRY-02, Q1, R2).
//!
//! Split from `quit_flow.rs` so the module describing the glue stays readable;
//! these drive the real `Workspace` through a deterministic service.

#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::model::InstanceStatus;
use dblocal_core::service::DblService;
use gpui_kit::TestAppContext;

use super::Workspace;
use dblocal_core::service::OperationId;

use crate::quit::QuitState;
use crate::tray::TrayAction;

/// A workspace on the emit-free service, with the tray forced unavailable so
/// window close is a quit rather than a hide.
fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<Workspace>,
    crate::app::test_service::QuietService,
    &mut gpui_kit::VisualTestContext,
) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let service = crate::app::test_service::QuietService::three();
    let svc: Arc<dyn DblService> = Arc::new(service.clone());
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::new_preview(svc, window, cx);
        workspace.tray_available = false;
        workspace.tray_note_shown = true;
        workspace.tray_note = Some(gpui_kit::SharedString::from(
            dblocal_core::strings::UI_TRAY_UNAVAILABLE_NOTE,
        ));
        workspace
    });
    cx.run_until_parked();
    (view, service, cx)
}

/// A quit with a running instance asks first: the dialog, not the exit.
#[gpui_kit::test]
fn quit_with_a_running_instance_opens_the_dialog(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Confirming { .. }
    ));
}

/// "Leave them running" marks the machine done without stopping anything.
#[gpui_kit::test]
fn keep_running_does_not_call_stop(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    let before = service.calls();
    view.update(cx, |this, cx| this.quit_keep_running(cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Done
    );
    assert_eq!(
        service.calls(),
        before,
        "keep-running must not touch engines"
    );
}

/// Cancel returns to Idle without a dialog and without a stop.
#[gpui_kit::test]
fn cancel_returns_to_idle(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    let before = service.calls();
    view.update(cx, |this, cx| this.quit_cancel(cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Idle
    );
    assert_eq!(service.calls(), before);
}

/// Q1: request acceptance must not quit; only OperationFinished does.
#[gpui_kit::test]
fn stop_all_does_not_quit_before_operation_finishes(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    let before = service.calls();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(service.calls() > before, "stop request must be issued");
    assert!(
        matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::StoppingAll { .. }
        ),
        "request acceptance must not transition to Done"
    );
    // Simulate time passing after acceptance, but no OperationFinished.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert!(
        matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::StoppingAll { .. }
        ),
        "still stopping before OperationFinished"
    );
    let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
    for op in ops {
        view.update(cx, |this, cx| {
            this.apply_event_with_cx(
                crate::service_bridge::UiEvent::OperationFinished {
                    id: op,
                    result: Ok(()),
                },
                cx,
            )
        });
        cx.run_until_parked();
    }
    assert_eq!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Done
    );
}

/// Q1: an operation finishing with an error leaves quit in StopFailed.
#[gpui_kit::test]
fn stop_all_operation_error_becomes_stop_failed(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
    assert!(!ops.is_empty());
    let err = dblocal_core::service::UserFacingError::new("DBL-9999", "boom");
    for op in ops {
        view.update(cx, |this, cx| {
            this.apply_event_with_cx(
                crate::service_bridge::UiEvent::OperationFinished {
                    id: op,
                    result: Err(err.clone()),
                },
                cx,
            )
        });
        cx.run_until_parked();
    }
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::StopFailed { .. }
    ));
}

/// R2: a stop that finishes before its registration is not lost; quit
/// completes immediately instead of waiting for `STOP_GRACE_SECS + 10`.
#[gpui_kit::test]
fn early_stop_finish_completes_quit_without_the_timeout(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
    let op = ops[0].clone();
    let instance = view.read_with(cx, |this, _| this.instances()[0].id.clone());

    // Simulate the event landing before the continuation registers the stop.
    view.update(cx, |this, _| this.quit_stops.clear());
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            crate::service_bridge::UiEvent::OperationFinished {
                id: op.clone(),
                result: Ok(()),
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |this, _| this.quit_stop_buffer_len()), 1);

    // Now the registration arrives.
    view.update(cx, |this, cx| this.register_quit_stop(op, instance, cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Done,
        "R2: a buffered success must complete quit, not wait for the timeout"
    );
}

/// R2: a buffered error lands as `StopFailed`.
#[gpui_kit::test]
fn early_stop_error_becomes_stop_failed(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
    let op = ops[0].clone();
    let instance = view.read_with(cx, |this, _| this.instances()[0].id.clone());
    view.update(cx, |this, _| this.quit_stops.clear());
    view.update(cx, |this, cx| {
        this.apply_event_with_cx(
            crate::service_bridge::UiEvent::OperationFinished {
                id: op.clone(),
                result: Err(dblocal_core::service::UserFacingError::new(
                    "DBL-9999", "boom",
                )),
            },
            cx,
        );
    });
    cx.run_until_parked();
    view.update(cx, |this, cx| this.register_quit_stop(op, instance, cx));
    cx.run_until_parked();
    assert!(
        matches!(
            view.read_with(cx, |this, _| this.quit_machine().state().clone()),
            QuitState::StopFailed { .. }
        ),
        "R2: a buffered error must surface as StopFailed"
    );
}

/// Q1: if an operation never finishes, the bounded wait moves to StopFailed.
#[gpui_kit::test]
fn stop_all_timeout_moves_to_stop_failed(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, _cx| {
        this.set_quit_stop_timeout(std::time::Duration::from_millis(20))
    });
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::StoppingAll { .. }
    ));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(30));
    cx.run_until_parked();
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::StopFailed { .. }
    ));
}

/// A tray Quit is the same entry point the window-close hook uses.
#[gpui_kit::test]
fn tray_quit_opens_the_same_dialog(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.apply_tray_action(TrayAction::Quit, cx));
    cx.run_until_parked();
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Confirming { .. }
    ));
}

/// Tray start/stop route to the same handlers as the window.
#[gpui_kit::test]
fn tray_start_routes_to_start_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    let before = service.calls();
    view.update(cx, |this, cx| {
        this.apply_tray_action(TrayAction::Start(id.to_string()), cx)
    });
    cx.run_until_parked();
    assert!(
        service.calls() > before,
        "a tray Start must call the service"
    );
}

/// R2 through the production ordering path: `stop_instance` emits
/// `OperationFinished` on the real broadcast channel before it returns the
/// id, the real `spawn_bridge` forwards it, and the ledger must still make
/// the quit complete (no timeout wait).
#[test]
fn early_finish_through_the_real_bridge_completes_the_ledger() {
    struct FastStop {
        tx: tokio::sync::broadcast::Sender<dblocal_core::service::ServiceEvent>,
    }
    impl FastStop {
        fn view() -> dblocal_core::service::InstanceView {
            use dblocal_core::model::{
                AuthMode, EngineKey, InstanceId, InstanceName, InstanceSpec, Port,
            };
            let spec = InstanceSpec::new(
                InstanceName::new("pg-fast").expect("name"),
                EngineKey::Postgresql,
                "17.4",
                Some(Port::new(5432).expect("port")),
                AuthMode::Password,
            )
            .expect("spec");
            let instance = dblocal_core::model::Instance::new(
                &spec,
                InstanceId::new(),
                InstanceStatus::Running,
                std::time::SystemTime::UNIX_EPOCH,
            );
            dblocal_core::service::InstanceView::from_instance(&instance)
        }
    }
    impl DblService for FastStop {
        fn stop_instance(
            &self,
            _id: &dblocal_core::model::InstanceId,
        ) -> dblocal_core::Result<OperationId> {
            let op = OperationId::new("op-fast");
            self.tx
                .send(dblocal_core::service::ServiceEvent::OperationFinished {
                    id: op.clone(),
                    result: Ok(()),
                })
                .expect("broadcast");
            Ok(op)
        }
        fn list_instances(&self) -> dblocal_core::Result<Vec<dblocal_core::service::InstanceView>> {
            Ok(vec![Self::view()])
        }
        fn catalog(&self) -> dblocal_core::Result<dblocal_core::service::CatalogView> {
            Ok(dblocal_core::service::CatalogView {
                status: dblocal_core::model::CatalogStatus::Fresh,
                banner: None,
                engines: Vec::new(),
            })
        }
        fn refresh_catalog(&self, _force: bool) -> dblocal_core::Result<OperationId> {
            Ok(OperationId::new("op"))
        }
        fn validate_spec(
            &self,
            _spec: &dblocal_core::model::InstanceSpec,
        ) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn suggest_port(
            &self,
            engine: dblocal_core::model::EngineKey,
        ) -> dblocal_core::Result<dblocal_core::model::Port> {
            dblocal_core::model::Port::new(engine.canonical_port())
        }
        fn create_instance(
            &self,
            _spec: dblocal_core::model::InstanceSpec,
        ) -> dblocal_core::Result<OperationId> {
            Ok(OperationId::new("op"))
        }
        fn start_instance(
            &self,
            _id: &dblocal_core::model::InstanceId,
        ) -> dblocal_core::Result<OperationId> {
            Ok(OperationId::new("op"))
        }
        fn delete_instance(
            &self,
            _id: &dblocal_core::model::InstanceId,
            _mode: dblocal_core::model::DeleteMode,
        ) -> dblocal_core::Result<OperationId> {
            Ok(OperationId::new("op"))
        }
        fn change_port(
            &self,
            _id: &dblocal_core::model::InstanceId,
            _port: dblocal_core::model::Port,
        ) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn data_size(&self, _id: &dblocal_core::model::InstanceId) -> dblocal_core::Result<u64> {
            Ok(0)
        }
        fn set_autostart(
            &self,
            _id: &dblocal_core::model::InstanceId,
            _enabled: bool,
        ) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn cancel_operation(&self, _id: &OperationId) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn connection_uri(
            &self,
            _id: &dblocal_core::model::InstanceId,
            _format: dblocal_core::service::UriFormat,
        ) -> dblocal_core::Result<dblocal_core::secret::SecretString> {
            Ok(dblocal_core::secret::SecretString::new("postgres://x"))
        }
        fn tail_logs(
            &self,
            _id: &dblocal_core::model::InstanceId,
            _lines: usize,
        ) -> dblocal_core::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn watch_logs(
            &self,
            _id: &dblocal_core::model::InstanceId,
            _enabled: bool,
        ) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn list_runtimes(&self) -> dblocal_core::Result<Vec<dblocal_core::model::Runtime>> {
            Ok(Vec::new())
        }
        fn remove_runtime(&self, _runtime: &str) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn list_archives(&self) -> dblocal_core::Result<Vec<dblocal_core::model::ArchivedData>> {
            Ok(Vec::new())
        }
        fn purge_archive(&self, _archive: &str) -> dblocal_core::Result<()> {
            Ok(())
        }
        fn subscribe(
            &self,
        ) -> tokio::sync::broadcast::Receiver<dblocal_core::service::ServiceEvent> {
            self.tx.subscribe()
        }
    }

    let (tx, _) = tokio::sync::broadcast::channel(16);
    let service = Arc::new(FastStop { tx });
    let shared: Arc<dyn DblService> = service.clone();
    let rx = crate::service_bridge::spawn_bridge(shared);
    let instance = FastStop::view().id;

    // The production order: the request emits, then returns its id.
    let op = service.stop_instance(&instance).expect("stop accepted");

    // The bridge delivers the finish before the registration is applied.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut buffered = None;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(crate::service_bridge::UiEvent::OperationFinished { id, result }) => {
                buffered = Some((id, result.is_ok()));
                break;
            }
            Ok(_) => continue,
            Err(flume::RecvTimeoutError::Timeout) => continue,
            Err(flume::RecvTimeoutError::Disconnected) => break,
        }
    }
    let (event_op, ok) = buffered.expect("the bridge must deliver OperationFinished");
    assert_eq!(event_op, op, "the event names the returned operation");

    let mut ledger = crate::quit::StopLedger::new();
    assert_eq!(ledger.finished(&event_op, ok), None, "not registered yet");
    assert_eq!(ledger.early_len(), 1);
    assert_eq!(
        ledger.register(op, instance.clone()),
        Some((instance.clone(), true)),
        "R2: the real ordering path must resolve to a completion"
    );
    assert!(ledger.is_empty());
}

/// The quit dialog's four states render without a panic and pick the right
/// button row (confirmation, stopping, failure, hidden).
#[gpui_kit::test]
fn quit_dialog_renders_every_state(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    // Hidden while idle.
    assert!(view.read_with(cx, |this, _| !this.quit.is_confirming()));
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |this, _| this.quit.is_confirming()));
    // Render the element tree through the window.
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(view.read_with(cx, |this, _| this.quit.is_confirming()));
    cx.run_until_parked();
    let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
    for op in ops {
        view.update(cx, |this, cx| {
            this.apply_event_with_cx(
                crate::service_bridge::UiEvent::OperationFinished {
                    id: op,
                    result: Err(dblocal_core::service::UserFacingError::new(
                        "DBL-9999", "boom",
                    )),
                },
                cx,
            )
        });
        cx.run_until_parked();
    }
    assert!(view.read_with(cx, |this, _| this.quit.is_confirming()));
    cx.run_until_parked();
}

/// The glue's action surface: bad tray ids, force exit, cancel after a
/// failed stop, a second stop-all, and detaching from the runtime.
#[gpui_kit::test]
fn quit_glue_handles_every_entry_point(cx: &mut TestAppContext) {
    let (view, service, cx) = fixture(cx);

    // Unparsable ids never panic and never touch the service.
    let before = service.calls();
    view.update(cx, |this, cx| {
        this.apply_tray_action(TrayAction::Start("not-an-id".to_owned()), cx);
        this.apply_tray_action(TrayAction::Stop("not-an-id".to_owned()), cx);
    });
    cx.run_until_parked();
    assert_eq!(service.calls(), before, "bad ids must not call the service");

    // Stop-all while idle is a no-op transition.
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Idle
    ));

    // A failed stop can be cancelled back to idle from the glue.
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    for _ in 0..40 {
        cx.run_until_parked();
        if view.read_with(cx, |this, _| this.quit_stop_pending_count()) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let ops = view.read_with(cx, |this, _| this.quit_stop_pending_ops());
    for op in ops {
        view.update(cx, |this, cx| {
            this.apply_event_with_cx(
                crate::service_bridge::UiEvent::OperationFinished {
                    id: op,
                    result: Err(dblocal_core::service::UserFacingError::new(
                        "DBL-9999", "boom",
                    )),
                },
                cx,
            )
        });
        cx.run_until_parked();
    }
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::StopFailed { .. }
    ));
    view.update(cx, |this, cx| this.quit_cancel(cx));
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Idle
    ));

    // Force exit reaches `Exit` from a failed state.
    view.update(cx, |this, cx| this.request_quit(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| this.quit_stop_all(cx));
    view.update(cx, |this, cx| this.quit_force_exit(cx));
    assert_eq!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Done
    );

    // Detaching drops the workspace's own tasks; the app pump owns actions.
    view.update(cx, |this, cx| this.detach_from_app_runtime(cx));
    cx.run_until_parked();
}

/// With no tray, closing the window is a quit; with one, it hides.
#[gpui_kit::test]
fn close_without_a_tray_asks_to_quit(cx: &mut TestAppContext) {
    let (view, _service, cx) = fixture(cx);
    view.update(cx, |this, cx| this.handle_window_close_request(cx));
    cx.run_until_parked();
    assert!(matches!(
        view.read_with(cx, |this, _| this.quit_machine().state().clone()),
        QuitState::Confirming { .. }
    ));
}
