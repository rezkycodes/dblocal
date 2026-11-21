//! Lifecycle tests: create, start, stop, delete, failure and retry.

use crate::model::{AuthMode, DeleteMode, EngineKey, InstanceStatus, Port, StartFailure};
use crate::service::{DblService, ServiceEvent, UriFormat};

use super::created;
use super::harness::{FailAt, Harness, test_port};

fn start(h: &Harness, id: &crate::model::InstanceId) -> Vec<ServiceEvent> {
    let op = h.service.start_instance(id).expect("start");
    h.run(op)
}

fn stop(h: &Harness, id: &crate::model::InstanceId) -> Vec<ServiceEvent> {
    let op = h.service.stop_instance(id).expect("stop");
    h.run(op)
}

fn delete(h: &Harness, id: &crate::model::InstanceId, mode: DeleteMode) -> Vec<ServiceEvent> {
    let op = h.service.delete_instance(id, mode).expect("delete");
    h.run(op)
}

#[test]
fn happy_path_create_start_stop_delete() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();

    let events = start(&h, &id);
    let finished = super::harness::finished_result(&events, &last_op(&events));
    assert!(
        matches!(finished, Some(Ok(()))),
        "start must succeed: {finished:?}"
    );
    assert_eq!(
        h.wait_for_status(&id, InstanceStatus::Running).status(),
        InstanceStatus::Running
    );
    assert!(
        h.instance(&id).expect("row").pid().is_some(),
        "an identity must be recorded"
    );

    let events = stop(&h, &id);
    assert!(matches!(
        super::harness::finished_result(&events, &last_op(&events)),
        Some(Ok(()))
    ));
    let stopped = h.wait_for_status(&id, InstanceStatus::Stopped);
    assert!(stopped.pid().is_none(), "identity is cleared on stop");

    let events = delete(&h, &id, DeleteMode::PurgeData);
    assert!(matches!(
        super::harness::finished_result(&events, &last_op(&events)),
        Some(Ok(()))
    ));
    assert!(h.instance(&id).is_none(), "row is gone");
    assert!(h.service.list_instances().expect("list").is_empty());
}

fn last_op(events: &[ServiceEvent]) -> crate::service::OperationId {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            ServiceEvent::OperationFinished { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("an operation finished")
}

#[test]
fn create_emits_stages_in_order_and_one_operation_finished() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("stage-check", EngineKey::Postgresql, port);
    // Subscribe first so the `queued` stage is not missed.
    let mut receiver = h.receiver();
    let op = h.service.create_instance(spec).expect("create");
    let events = h.run_on(&mut receiver, op.clone());
    let stages = super::harness::stages(&events);
    // The instance is created from a runtime that is "already installed" in the
    // harness, so the expected stage order is initializing then done.
    assert_eq!(stages.first(), Some(&"queued"), "{stages:?}");
    assert!(stages.contains(&"initializing"), "{stages:?}");
    assert_eq!(stages.last(), Some(&"done"), "{stages:?}");
    let finished = events
        .iter()
        .filter(|event| matches!(event, ServiceEvent::OperationFinished { .. }))
        .count();
    assert_eq!(finished, 1, "exactly one OperationFinished: {events:#?}");
}

#[test]
fn create_rejects_a_duplicate_name() {
    let h = Harness::new(EngineKey::Postgresql);
    created(&h, "app-dev");
    let port = test_port();
    let spec = h.spec("app-dev", EngineKey::Postgresql, port);
    let err = h.service.create_instance(spec).expect_err("duplicate name");
    assert_eq!(err.code(), crate::error::CODE_ALREADY_EXISTS);
}

#[test]
fn create_failure_at_every_stage_leaves_nothing_behind() {
    // Table-driven so a new injected stage is one line; today the only stage
    // create itself owns is `initialize` (download failures are covered by the
    // download tests). `cases` stays a slice so the loop is real, not a
    // disguised single call.
    let cases: &[FailAt] = &[FailAt::Initialize];
    for &fail_at in cases {
        let adapter = std::sync::Arc::new(super::harness::ScriptedAdapter::failing(
            EngineKey::Postgresql,
            fail_at,
        ));
        let h = Harness::with_adapter(adapter, EngineKey::Postgresql);
        let port = test_port();
        let spec = h.spec("doomed", EngineKey::Postgresql, port);
        let (op, events) = h.run_call(|| h.service.create_instance(spec));
        let finished = super::harness::finished_result(&events, &op);
        assert!(
            matches!(finished, Some(Err(_))),
            "create must fail at {fail_at:?}: {finished:?}"
        );
        // Directory, row and credential must all be gone.
        assert!(
            !h.paths
                .instance_dir(&crate::model::InstanceName::new("doomed").unwrap())
                .exists(),
            "instance directory must be removed"
        );
        assert!(h.instances().is_empty(), "no row may remain");
        assert_eq!(h.secrets.file.len(), 0, "no file credential may remain");
        assert_eq!(
            h.secrets.keychain.len(),
            0,
            "no keychain credential may remain"
        );
    }
}

#[test]
fn start_on_a_port_taken_after_create_fails_with_port_in_use() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();
    h.busy_ports.lock().unwrap().push(instance.port().get());

    let events = start(&h, &id);
    let finished = super::harness::finished_result(&events, &last_op(&events));
    match finished {
        Some(Err(error)) => assert_eq!(error.code, crate::error::CODE_PORT_IN_USE),
        other => panic!("expected PortInUse, got {other:?}"),
    }
    let failed = h.wait_for_status(&id, InstanceStatus::Failed);
    assert!(failed.pid().is_none(), "no identity after a refused start");
}

#[test]
fn a_failed_start_can_be_retried_after_the_port_frees() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();
    h.busy_ports.lock().unwrap().push(instance.port().get());
    let _ = start(&h, &id);
    h.wait_for_status(&id, InstanceStatus::Failed);

    h.busy_ports.lock().unwrap().clear();
    let events = start(&h, &id);
    assert!(matches!(
        super::harness::finished_result(&events, &last_op(&events)),
        Some(Ok(()))
    ));
    h.wait_for_status(&id, InstanceStatus::Running);
}

#[test]
fn an_unhealthy_engine_produces_a_diagnosed_failure_and_can_be_retried() {
    let h = Harness::new(EngineKey::Postgresql);
    *h.adapter.fail_at.lock().unwrap() = FailAt::Health;
    *h.adapter.marker.lock().unwrap() = Some("port_in_use".to_owned());

    let instance = created(&h, "app-dev");
    let id = instance.id().clone();

    let events = start(&h, &id);
    let finished = super::harness::finished_result(&events, &last_op(&events));
    match finished {
        Some(Err(error)) => {
            assert_eq!(error.code, crate::error::CODE_ENGINE);
            assert_eq!(error.diagnosis, Some(StartFailure::PortInUse));
        }
        other => panic!("expected a diagnosed failure, got {other:?}"),
    }

    let views = h.service.list_instances().expect("list");
    let failure = views[0].failure.as_ref().expect("failure info on the view");
    assert_eq!(failure.diagnosis, StartFailure::PortInUse);
    assert_eq!(failure.code, crate::error::CODE_ENGINE);

    // Retry after the fault clears.
    *h.adapter.fail_at.lock().unwrap() = FailAt::None;
    let events = start(&h, &id);
    assert!(matches!(
        super::harness::finished_result(&events, &last_op(&events)),
        Some(Ok(()))
    ));
    h.wait_for_status(&id, InstanceStatus::Running);
}

#[test]
fn a_forced_stop_is_reported_as_an_event_and_a_graceful_one_is_not() {
    // `F-INS-03` AC3: the user must be told a stop was a forced kill. The event
    // is the observable report; asserting the flag the test itself set proved
    // nothing.
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();

    // Graceful first: no `StopForced`.
    start(&h, &id);
    h.wait_for_status(&id, InstanceStatus::Running);
    let mut receiver = h.receiver();
    let op = h.service.stop_instance(&id).expect("stop");
    let events = h.run_on(&mut receiver, op);
    assert!(
        matches!(
            super::harness::finished_result(&events, &last_op(&events)),
            Some(Ok(()))
        ),
        "graceful stop: {:#?}",
        super::harness::describe(&events)
    );
    assert!(
        !super::harness::stop_forced_for(&events, &id),
        "a graceful stop must not emit StopForced: {:#?}",
        super::harness::describe(&events)
    );

    // Now a stop that needs escalation: exactly one `StopForced`, before the
    // final change.
    start(&h, &id);
    h.wait_for_status(&id, InstanceStatus::Running);
    h.terminator
        .forced
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut receiver = h.receiver();
    let op = h.service.stop_instance(&id).expect("stop");
    let events = h.run_on(&mut receiver, op);
    assert!(
        super::harness::stop_forced_for(&events, &id),
        "a forced stop must emit StopForced: {:#?}",
        super::harness::describe(&events)
    );
    let forced_count = events
        .iter()
        .filter(|event| matches!(event, crate::service::ServiceEvent::StopForced { .. }))
        .count();
    assert_eq!(forced_count, 1, "StopForced is emitted exactly once");

    // It must come before the `Stopped` change, so a UI sees the reason first.
    let positions: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            crate::service::ServiceEvent::StopForced { .. } => Some("forced"),
            crate::service::ServiceEvent::InstanceChanged { view }
                if view.status == InstanceStatus::Stopped =>
            {
                Some("stopped")
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        positions,
        vec!["forced", "stopped"],
        "StopForced precedes the Stopped view"
    );
    let stopped = h.wait_for_status(&id, InstanceStatus::Stopped);
    assert!(stopped.pid().is_none());
}

#[test]
fn start_from_a_running_instance_is_rejected() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();
    start(&h, &id);
    h.wait_for_status(&id, InstanceStatus::Running);
    let err = h.service.start_instance(&id).expect_err("already running");
    assert_eq!(err.code(), crate::error::CODE_INVALID_TRANSITION);
}

#[test]
fn delete_in_archive_mode_records_the_data() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();
    let data_dir = h.paths.data_dir(instance.name());
    assert!(
        data_dir.is_dir(),
        "the harness adapter creates the data dir"
    );

    let events = delete(&h, &id, DeleteMode::ArchiveData);
    assert!(matches!(
        super::harness::finished_result(&events, &last_op(&events)),
        Some(Ok(()))
    ));
    assert!(h.instance(&id).is_none(), "row removed");
    assert!(
        !data_dir.exists(),
        "data moved out of the instance directory"
    );
    let archives = h.service.list_archives().expect("archives");
    assert_eq!(archives.len(), 1, "one archive recorded");
    assert!(archives[0].path().exists(), "archive directory exists");
    assert!(
        archives[0].path().starts_with(h.paths.archive_dir()),
        "archive lives under archive/"
    );
}

#[test]
fn delete_while_running_stops_first() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();
    start(&h, &id);
    h.wait_for_status(&id, InstanceStatus::Running);

    let events = delete(&h, &id, DeleteMode::PurgeData);
    let result = super::harness::finished_result(&events, &last_op(&events));
    assert!(matches!(result, Some(Ok(()))), "delete failed: {result:?}");
    assert!(h.instance(&id).is_none());
    assert!(
        h.terminator.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the process was stopped before deletion"
    );
}

#[test]
fn connection_uri_carries_the_recorded_pointer_not_the_secret_in_sqlite() {
    let h = Harness::new(EngineKey::Postgresql);
    let instance = created(&h, "app-dev");
    let id = instance.id().clone();
    let uri = h
        .service
        .connection_uri(&id, UriFormat::Uri)
        .expect("connection uri");
    assert!(
        uri.expose().starts_with("postgresql://postgres:"),
        "{}",
        uri.expose()
    );

    // The stored password must be in exactly one fake backend and never in the
    // database file.
    let db = std::fs::read(h.paths.state_db()).expect("db bytes");
    let text = String::from_utf8_lossy(&db);
    use crate::secrets::SecretBackend;
    let reference = h
        .store()
        .get_credential(&id)
        .expect("credential")
        .expect("recorded");
    let stored = h.secrets.file.get(&reference).expect("file secret");
    assert!(
        !text.contains(stored.expose()),
        "the password must never reach SQLite"
    );
}

#[test]
fn no_auth_instances_store_no_credential() {
    let h = Harness::new(EngineKey::Valkey);
    let port = test_port();
    let spec = h.spec("cache", EngineKey::Valkey, port);
    let spec = crate::model::InstanceSpec::new(
        spec.name().clone(),
        EngineKey::Valkey,
        spec.version(),
        Some(Port::new(port).expect("port")),
        AuthMode::None,
    )
    .expect("spec");
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("created");
    assert!(
        h.store()
            .get_credential(instance.id())
            .expect("credential lookup")
            .is_none(),
        "no-auth instances have no credential pointer"
    );
}
