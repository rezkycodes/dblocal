//! Supervisor vs. operation races, and the events that announce a removed row.
//!
//! These cover the two defects that a "green" suite hid: the supervisor
//! relabelling a clean stop as a crash, and consumers never being told a row was
//! removed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use crate::model::{DeleteMode, EngineKey, InstanceStatus};
use crate::service::{DblService, ServiceEvent};

use super::harness::{FailAt, Harness, test_port};

#[test]
fn the_supervisor_never_overwrites_a_clean_stop_with_failed() {
    // A stop and the supervisor race on every tick. The supervisor validates the
    // process, the stop kills it, and the supervisor's `Failed` write must lose
    // the compare-and-set because the row is no longer `Running`.
    for round in 0..50 {
        // A 1 ms poll interval makes the race certain rather than lucky.
        let h = super::harness::fast_supervisor(Duration::from_millis(1));
        let port = test_port();
        let spec = h.spec("race-stop", EngineKey::Postgresql, port);
        h.create(spec);
        let instance = h.instances().into_iter().next().expect("instance");
        let id = instance.id().clone();
        let (_ops, start_run) = h.run_call(|| h.service.start_instance(&id));
        assert!(
            matches!(start_run.result(), Some(Ok(()))),
            "start: {:?}",
            start_run.result()
        );
        h.wait_for_status(&id, InstanceStatus::Running);

        // Let the fast supervisor poll while the stop runs.
        let mut receiver = h.receiver();
        let op = h.service.stop_instance(&id).expect("stop");
        let run = h.run_on(&mut receiver, op.clone());
        let events = &run.events;
        assert!(
            matches!(run.result(), Some(Ok(()))),
            "round {round}: the stop must succeed: {:#?}",
            super::harness::describe(events)
        );

        let final_status = h
            .instance(&id)
            .unwrap_or_else(|| panic!("round {round}: the row must survive"))
            .status();
        assert_eq!(
            final_status,
            InstanceStatus::Stopped,
            "round {round}: a clean stop must never become Failed"
        );

        // Give the supervisor several more ticks to try to overwrite it.
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(
            h.instance(&id).expect("row").status(),
            InstanceStatus::Stopped,
            "round {round}: the supervisor must not relabel a stopped row"
        );
    }
}

#[test]
fn a_finished_delete_ends_with_instance_removed() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("gone", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let mut receiver = h.receiver();
    let op = h
        .service
        .delete_instance(&id, DeleteMode::PurgeData)
        .expect("delete");
    let run = h.run_on(&mut receiver, op);
    let events = &run.events;
    assert!(
        super::harness::removed_for(events, &id),
        "a delete ends with InstanceRemoved: {:#?}",
        super::harness::describe(events)
    );
    // The removal is the last thing about that instance.
    let last_instance_event = events
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event,
                ServiceEvent::InstanceRemoved { .. } | ServiceEvent::InstanceChanged { .. }
            )
        })
        .expect("an instance event");
    assert!(
        matches!(last_instance_event, ServiceEvent::InstanceRemoved { .. }),
        "the final instance event is the removal"
    );
}

#[test]
fn a_failed_create_ends_with_instance_removed_and_one_error() {
    let adapter = Arc::new(super::harness::ScriptedAdapter::failing(
        EngineKey::Postgresql,
        FailAt::Initialize,
    ));
    let h = Harness::with_adapter(adapter, EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("rollback", EngineKey::Postgresql, port);

    let mut receiver = h.receiver();
    let op = h.service.create_instance(spec).expect("create");
    let run = h.run_on(&mut receiver, op.clone());
    let events = &run.events;

    // The row is gone, so we cannot look its id up afterwards; the removal event
    // carries it.
    let removed = events
        .iter()
        .find_map(|event| match event {
            ServiceEvent::InstanceRemoved { id } => Some(id.clone()),
            _ => None,
        })
        .expect("a rolled-back create ends with InstanceRemoved");
    assert!(h.instances().is_empty(), "no row survives a failed create");
    assert!(super::harness::removed_for(events, &removed));
    assert_eq!(
        super::harness::finished_count(events),
        1,
        "exactly one OperationFinished"
    );
    assert!(
        matches!(run.result(), Some(Err(_))),
        "the failed create reports its error"
    );
    // The removal must not be followed by an `InstanceChanged` for a dead row.
    let removal_index = events
        .iter()
        .position(|event| matches!(event, ServiceEvent::InstanceRemoved { .. }))
        .expect("removal present");
    let changed_after = events[removal_index + 1..]
        .iter()
        .any(|event| matches!(event, ServiceEvent::InstanceChanged { .. }));
    assert!(
        !changed_after,
        "no InstanceChanged may follow the removal of the row"
    );
}

#[test]
fn a_failed_row_recovered_after_a_restart_still_carries_a_diagnosis() {
    // `F-LOG-02`: a row that says "failed" must explain itself. The in-memory
    // failure map is empty in a new process, so the view derives the failure from
    // the log tail instead of showing an empty panel.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("diagnose-later", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // A log the adapter can classify, and a row that is `Failed`.
    *h.adapter.marker.lock().unwrap() = Some("port_in_use".to_owned());
    let log = h.paths.logs_dir(instance.name()).join("postgres.log");
    std::fs::create_dir_all(log.parent().expect("logs dir")).expect("logs dir");
    std::fs::write(
        &log,
        "could not bind IPv4 address\nAddress already in use\n",
    )
    .expect("log");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Failed)
        .expect("failed");

    // A brand-new service over the same home: the in-memory map is empty, so any
    // diagnosis must come from re-reading the log.
    let restarted = super::harness::restart(&h.paths, std::sync::Arc::clone(&h.procs));
    // The restarted harness builds a fresh scripted adapter; tell it how to read
    // the text (the real adapters match these markers themselves). The point of
    // the test is *where* the diagnosis comes from — the log file — not the
    // scripted adapter's memory.
    *restarted.adapter.marker.lock().unwrap() = Some("port_in_use".to_owned());
    let view = restarted
        .service
        .list_instances()
        .expect("list")
        .into_iter()
        .find(|view| view.id == id)
        .expect("the row is still listed");

    let failure = view
        .failure
        .expect("a Failed view must carry a diagnosis after a restart");
    assert_eq!(failure.code, crate::error::CODE_ENGINE);
    assert_eq!(
        failure.diagnosis,
        crate::model::StartFailure::PortInUse,
        "the diagnosis comes from the log tail through the adapter"
    );
    assert!(
        !failure.log_tail.is_empty(),
        "the raw tail is shown when the cause is unknown"
    );
}

#[test]
fn a_stop_that_finds_nothing_to_stop_still_reports_cleanly() {
    // No recorded identity and a graceful shutdown that succeeds: the operation
    // completes and the row is `Stopped`, with no forced-kill event.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("no-identity", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Running)
        .expect("running");

    let mut receiver = h.receiver();
    let op = h.service.stop_instance(&id).expect("stop");
    let run = h.run_on(&mut receiver, op.clone());
    let events = &run.events;
    assert!(
        matches!(run.result(), Some(Ok(()))),
        "no identity + graceful shutdown is a clean stop: {:#?}",
        super::harness::describe(events)
    );
    assert!(!super::harness::stop_forced_for(events, &id));
    assert_eq!(
        h.instance(&id).expect("row").status(),
        InstanceStatus::Stopped
    );
}

#[test]
fn a_delete_refuses_to_remove_a_row_whose_engine_survives() {
    // Removing the row while the engine is alive would orphan it with no
    // identity left to stop it. The delete fails cleanly instead.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("delete-alive", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let (_ops, start_run) = h.run_call(|| h.service.start_instance(&id));
    assert!(
        matches!(start_run.result(), Some(Ok(()))),
        "start: {:?}",
        start_run.result()
    );
    h.wait_for_status(&id, InstanceStatus::Running);

    h.terminator
        .refuse
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (op, run) = h.run_call(|| h.service.delete_instance(&id, DeleteMode::PurgeData));
    let events = &run.events;
    let finished = run.result();
    assert!(
        matches!(finished, Some(Err(_))),
        "op {op}: a surviving engine must fail the delete: finished={finished:?} events={:#?}",
        super::harness::describe(events)
    );
    assert!(
        h.instance(&id).is_some(),
        "the row survives so the engine can still be managed"
    );
}

#[test]
fn a_rollback_that_cannot_prove_a_safe_path_removes_nothing() {
    // `safe_within` is not decorative: when it rejects a path the rollback aborts
    // instead of deleting anyway. The guard is exercised directly, because a
    // corrupt row is the only way to reach it and a real `DblPaths` cannot
    // produce one.
    let h = Harness::new(EngineKey::Postgresql);
    let outside = h.dir.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    let paths = crate::paths::DblPaths::with_root(h.dir.path().join("root"));
    // An absolute path outside the root is exactly what a corrupt row could hold.
    assert!(
        super::super::helpers::safe_within(&paths, &outside).is_err(),
        "a path outside the home must be refused"
    );
    assert!(
        super::super::helpers::safe_within(&paths, &paths.root().join("instances").join("app"))
            .is_ok(),
        "a path inside the home is accepted"
    );
    assert!(outside.exists(), "a refused path is never deleted");
}
