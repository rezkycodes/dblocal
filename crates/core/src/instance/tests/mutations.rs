//! Small mutations, event sequences and early-exit handling.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use crate::model::{DeleteMode, EngineKey, InstanceStatus};
use crate::service::{DblService, ServiceEvent};

use super::harness::{FailAt, FakeProcs, Harness, test_port};

fn start_and_wait(h: &Harness, id: &crate::model::InstanceId) {
    let op = h.service.start_instance(id).expect("start");
    let _ = h.run(op);
    h.wait_for_status(id, InstanceStatus::Running);
}

#[test]
fn an_engine_that_exits_immediately_is_diagnosed_and_failed() {
    // Spawn returns an identity, but the process is gone by the time the health
    // poll looks: that is an early exit, which must be diagnosed and failed.
    let procs = Arc::new(FakeProcs::new());
    let h = Harness::with_options(EngineKey::Postgresql, {
        let procs = Arc::clone(&procs);
        move |options| {
            options.probe =
                Some(Arc::clone(&procs)
                    as Arc<dyn crate::process::probe::ProcessProbe + Send + Sync>);
            options.spawn = Some(Arc::new(move |_command, _log| {
                // Return a live identity, then kill it so the poll sees it gone.
                let identity = procs.spawn("/fake/engine/bin/postgres");
                procs.kill(identity.pid());
                Ok(identity)
            }));
        }
    });
    let port = test_port();
    let spec = h.spec("early-exit", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let op = h.service.start_instance(&id).expect("start");
    let events = h.run(op.clone());
    let finished = super::harness::finished_result(&events, &op);
    assert!(
        matches!(finished, Some(Err(_))),
        "an immediate exit must fail: {finished:?}"
    );
    let failed = h.wait_for_status(&id, InstanceStatus::Failed);
    assert!(
        failed.pid().is_none(),
        "no identity survives a failed start"
    );
    let views = h.service.list_instances().expect("list");
    assert!(
        views[0].failure.is_some(),
        "the failure is carried on the view"
    );
}

#[test]
fn create_start_stop_delete_emit_the_expected_instance_changes() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("sequences", EngineKey::Postgresql, port);
    let mut receiver = h.service.subscribe();
    let op = h.service.create_instance(spec).expect("create");
    let create_events = h.run_on(&mut receiver, op);
    let created_changes = statuses(&create_events);
    assert!(
        created_changes.contains(&InstanceStatus::Initializing)
            && created_changes.contains(&InstanceStatus::Stopped),
        "create moves through initializing to stopped: {created_changes:?}"
    );

    let id = h
        .instances()
        .into_iter()
        .next()
        .expect("instance")
        .id()
        .clone();
    let op = h.service.start_instance(&id).expect("start");
    let start_changes = statuses(&h.run_on(&mut receiver, op));
    assert_eq!(
        start_changes.first().copied(),
        Some(InstanceStatus::Starting),
        "Starting is persisted before the spawn: {start_changes:?}"
    );
    assert_eq!(start_changes.last().copied(), Some(InstanceStatus::Running));

    let op = h.service.stop_instance(&id).expect("stop");
    let stop_changes = statuses(&h.run_on(&mut receiver, op));
    assert_eq!(
        stop_changes.first().copied(),
        Some(InstanceStatus::Stopping),
        "Stopping is persisted before the signal: {stop_changes:?}"
    );
    assert_eq!(stop_changes.last().copied(), Some(InstanceStatus::Stopped));

    let op = h
        .service
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("delete");
    let delete_changes = statuses(&h.run_on(&mut receiver, op));
    assert!(
        delete_changes.contains(&InstanceStatus::Deleting),
        "Deleting is persisted before the removal: {delete_changes:?}"
    );
}

fn statuses(events: &[ServiceEvent]) -> Vec<InstanceStatus> {
    events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::InstanceChanged { view } => Some(view.status),
            _ => None,
        })
        .collect()
}

#[test]
fn change_port_moves_a_stopped_instance_and_refuses_a_taken_one() {
    let h = Harness::new(EngineKey::Postgresql);
    let first = h.spec("first", EngineKey::Postgresql, test_port());
    h.create(first);
    let second = h.spec("second", EngineKey::Postgresql, test_port());
    h.create(second);

    let instances = h.instances();
    let one = instances
        .iter()
        .find(|i| i.name().as_str() == "first")
        .expect("first");
    let two = instances
        .iter()
        .find(|i| i.name().as_str() == "second")
        .expect("second");

    // Moving onto another instance's port is refused.
    let err = h
        .service
        .change_port(one.id(), two.port())
        .expect_err("port taken by another instance");
    assert_eq!(err.code(), crate::error::CODE_PORT_IN_USE);

    // Moving onto a free port works and releases the old allocation.
    let free = crate::model::Port::new(test_port()).expect("port");
    h.service.change_port(one.id(), free).expect("move");
    let moved = h.instance(one.id()).expect("moved");
    assert_eq!(moved.port(), free);
    let suggestion = h
        .service
        .suggest_port(EngineKey::Postgresql)
        .expect("suggest");
    assert_ne!(suggestion, free, "the new allocation is held");
}

#[test]
fn change_port_is_refused_while_running() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("running", EngineKey::Postgresql, port);
    h.create(spec);
    let id = h
        .instances()
        .into_iter()
        .next()
        .expect("instance")
        .id()
        .clone();
    start_and_wait(&h, &id);

    let free = crate::model::Port::new(test_port()).expect("port");
    let err = h.service.change_port(&id, free).expect_err("must refuse");
    assert_eq!(err.code(), crate::error::CODE_INVALID_TRANSITION);
}

#[test]
fn data_size_and_autostart_read_and_write_state() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("sizes", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // The adapter wrote PG_VERSION into the data directory.
    let size = h.service.data_size(&id).expect("size");
    assert!(size > 0, "the data directory has content");

    assert!(!instance.autostart());
    h.service.set_autostart(&id, true).expect("autostart on");
    assert!(h.instance(&id).expect("row").autostart());
    h.service.set_autostart(&id, false).expect("autostart off");
    assert!(!h.instance(&id).expect("row").autostart());
}

#[test]
fn tail_logs_returns_what_the_engine_wrote() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("logs", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let log = h.paths.logs_dir(instance.name()).join("postgres.log");
    std::fs::create_dir_all(log.parent().unwrap()).expect("logs dir");
    std::fs::write(&log, "line-1\nline-2\nline-3\n").expect("write log");

    let lines = h.service.tail_logs(&id, 2).expect("tail");
    assert_eq!(lines, vec!["line-2", "line-3"]);
}

#[test]
fn cancelling_an_unknown_operation_is_a_typed_not_found() {
    let h = Harness::new(EngineKey::Postgresql);
    let err = h
        .service
        .cancel_operation(&crate::service::OperationId::new("op-9999"))
        .expect_err("unknown operation");
    assert_eq!(err.code(), crate::error::CODE_NOT_FOUND);
}

#[test]
fn a_missing_instance_is_reported_not_found() {
    let h = Harness::new(EngineKey::Postgresql);
    let id = crate::model::InstanceId::new();
    assert_eq!(
        h.service.start_instance(&id).expect_err("missing").code(),
        crate::error::CODE_NOT_FOUND
    );
    assert_eq!(
        h.service.data_size(&id).expect_err("missing").code(),
        crate::error::CODE_NOT_FOUND
    );
}

#[test]
fn a_stop_whose_shutdown_and_kill_both_fail_still_ends_stopped() {
    // The graceful request fails and the terminator refuses (it reports a
    // mismatched identity), so the operation reports the failure but the row
    // must not be left in `Stopping`.
    struct Refusing;
    impl crate::instance::Terminator for Refusing {
        fn terminate(
            &self,
            _identity: &crate::process::identity::ProcessIdentity,
            _grace: std::time::Duration,
        ) -> crate::error::Result<crate::process::terminate::TerminateOutcome> {
            Err(crate::DblError::Engine {
                reason: "identity refused on purpose".to_owned(),
            })
        }
    }
    let h = Harness::with_options(EngineKey::Postgresql, |options| {
        options.terminator = Some(Arc::new(Refusing));
    });
    let port = test_port();
    let spec = h.spec("stuck", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // A live identity is needed for the terminator to be consulted at all.
    let identity = h.procs.spawn("/fake/engine/bin/postgres");
    h.store()
        .update_process_identity(
            &id,
            Some(identity.pid()),
            Some(identity.started_at()),
            Some(identity.exe()),
        )
        .expect("identity");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Running)
        .expect("running");
    *h.adapter.fail_at.lock().unwrap() = FailAt::Shutdown;

    let op = h.service.stop_instance(&id).expect("stop");
    let events = h.run(op.clone());
    assert!(
        matches!(super::harness::finished_result(&events, &op), Some(Err(_))),
        "the refusal surfaces as a failure"
    );
    let settled = h.instance(&id).expect("row");
    assert_eq!(
        settled.status(),
        InstanceStatus::Stopped,
        "a failed stop must not strand the row in Stopping"
    );
    assert!(settled.pid().is_none(), "the stale identity is cleared");
}
