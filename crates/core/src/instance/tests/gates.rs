//! Deterministic races: a supervisor decision that is stale by the time it is
//! written, and a claim whose expectation another writer already invalidated.
//!
//! These replace "run it a thousand times and hope" with a gate: the code under
//! test is parked at the exact point where it holds an answer but has not acted,
//! and the test moves the world underneath it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::model::{EngineKey, InstanceStatus};
use crate::process::identity::ProcessState;
use crate::service::{DblService, ServiceEvent};

use super::harness::{FakeProcs, FakeSecrets, FakeTerminator, GatedProbe, Harness, test_port};

#[test]
fn a_stale_supervisor_decision_never_overwrites_a_finished_stop() {
    // The supervisor's `validate` call is gated: it signals "I have an answer
    // pending" and parks. The test then runs a complete, honest stop — the row
    // becomes `Stopped` and its identity is cleared — and only then releases the
    // supervisor with the stale answer `Absent`. The supervisor must not write
    // `Failed` over the stop: its compare-and-set from `Running` fails and it
    // records nothing.
    let procs = Arc::new(FakeProcs::new());
    let gated = Arc::new(GatedProbe::new(Arc::clone(&procs)));
    let h = super::harness::with_gated_probe(
        EngineKey::Postgresql,
        Arc::clone(&procs),
        Arc::clone(&gated),
        Duration::from_millis(15),
    );
    let port = test_port();
    let spec = h.spec("gated-stop", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let ops = h.service.start_instance(&id).expect("start");
    h.run(ops);
    h.wait_for_status(&id, InstanceStatus::Running);

    // Arm the gate and wait for the supervisor to be parked inside `validate`
    // with a decision it has not yet written.
    let mut receiver = h.receiver();
    gated.arm();
    assert!(
        gated.wait_reached(Duration::from_secs(5)),
        "the supervisor never reached validate; the gate cannot be exercised"
    );

    // While the supervisor is parked, perform a complete stop. It is a separate
    // operation but the supervisor holds no lock on the row, exactly as in a real
    // race.
    let stop_op = h.service.stop_instance(&id).expect("stop");
    let stop_events = h.run_on(&mut receiver, stop_op.clone());
    assert!(
        matches!(
            super::harness::finished_result(&stop_events, &stop_op),
            Some(Ok(()))
        ),
        "the stop must succeed: {:#?}",
        super::harness::describe(&stop_events)
    );
    assert_eq!(
        h.instance(&id).expect("row").status(),
        InstanceStatus::Stopped,
        "the stop persisted Stopped"
    );

    // Release the supervisor with the answer that would make it mark Failed.
    gated.release_with(ProcessState::Absent);
    // Give it room to (incorrectly) act.
    std::thread::sleep(Duration::from_millis(120));

    // The row is still Stopped; the stale `Failed` lost the compare-and-set.
    assert_eq!(
        h.instance(&id).expect("row").status(),
        InstanceStatus::Stopped,
        "a stale supervisor decision must not relabel a finished stop"
    );

    // No `InstanceChanged` carrying Failed was emitted after the release, and the
    // failure record has no entry for this instance.
    let failed_change = std::iter::from_fn(|| receiver.try_recv().ok()).any(|event| {
        matches!(
            event,
            ServiceEvent::InstanceChanged { view }
                if view.id == id && view.status == InstanceStatus::Failed
        )
    });
    assert!(
        !failed_change,
        "no Failed view may be emitted after the row was stopped"
    );
    assert!(
        !h.has_failure_record(&id),
        "the supervisor must not record a failure it did not write"
    );
}

#[test]
fn a_claim_from_a_stale_status_is_refused_and_changes_nothing() {
    // The compare-and-set is the whole point of the claim: if the caller's
    // expected status is stale (another service already moved the row), the claim
    // must fail as busy and leave the row exactly as the winner left it.
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("stale-claim", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    // Another service "wins" the start: the row is now Starting.
    let ops = h.service.start_instance(&id).expect("start");
    h.run(ops);
    h.wait_for_status(&id, InstanceStatus::Running);
    // Reset to a non-winning shape to claim from a stale expectation: the row is
    // Running, but our stale observation says Stopped.
    let err = h
        .service
        .claim_for_test(
            &id,
            InstanceStatus::Stopped,
            crate::model::InstanceEvent::StartRequested,
        )
        .expect_err("a stale expectation must lose");
    assert_eq!(
        err.code(),
        crate::error::CODE_INVALID_TRANSITION,
        "the loser gets the typed busy/transition error, got {err:?}"
    );
    assert_eq!(
        h.instance(&id).expect("row").status(),
        InstanceStatus::Running,
        "the losing claim must not change the row"
    );

    // The same stale claim against a row that already moved to Starting: the
    // honest case the two-service race reaches.
    h.store()
        .transition_status(&id, InstanceStatus::Running, InstanceStatus::Starting)
        .expect("starting");
    let err = h
        .service
        .claim_for_test(
            &id,
            InstanceStatus::Stopped,
            crate::model::InstanceEvent::StartRequested,
        )
        .expect_err("a stale Stopped expectation must lose to Starting");
    assert_eq!(err.code(), crate::error::CODE_INVALID_TRANSITION);
    assert_eq!(
        h.instance(&id).expect("row").status(),
        InstanceStatus::Starting,
        "the row stays wherever the winner put it"
    );
}

#[test]
fn a_delete_claim_from_a_stale_status_is_refused_and_changes_nothing() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("stale-delete", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    // Another service already claimed Deleting.
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");

    // Our stale expectation says Stopped; the claim must lose and change nothing.
    let won = h
        .service
        .claim_status_for_test(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("claim");
    assert!(!won, "a stale expected status must make the claim lose");
    assert_eq!(
        h.instance(&id).expect("row").status(),
        InstanceStatus::Deleting,
        "the row stays on the winner's status"
    );
}

#[test]
fn the_in_flight_marker_trusts_identity_not_a_bare_pid() {
    use crate::instance::delete::{IN_FLIGHT_MARKER, delete_in_flight};

    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("marker-identity", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    h.store()
        .transition_status(
            instance.id(),
            InstanceStatus::Stopped,
            InstanceStatus::Deleting,
        )
        .expect("deleting");

    let marker = h.paths.instance_dir(instance.name()).join(IN_FLIGHT_MARKER);
    std::fs::create_dir_all(marker.parent().expect("dir")).expect("dir");

    // A live process, but the marker names a *different* start time: a PID reused
    // after a reboot. This must read as stale, or the row could never recover.
    let live = h.procs.spawn("/some/other/dblocal");
    let wrong_start = live
        .started_at()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
        + 10_000;
    std::fs::write(
        &marker,
        format!(
            "pid={}\nstarted={wrong_start}.0\nexe=/some/other/dblocal\nnonce=other\n",
            live.pid()
        ),
    )
    .expect("marker");
    assert!(
        !delete_in_flight(h.service.inner(), &h.instance(instance.id()).expect("row")),
        "a reused PID with a different start time is a stale marker"
    );

    // The matching identity is in flight.
    let started = live
        .started_at()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}.0", d.as_secs()))
        .unwrap_or_else(|_| "0.0".to_owned());
    std::fs::write(
        &marker,
        format!(
            "pid={}\nstarted={started}\nexe={}\nnonce=other\n",
            live.pid(),
            live.exe().display()
        ),
    )
    .expect("marker");
    assert!(
        delete_in_flight(h.service.inner(), &h.instance(instance.id()).expect("row")),
        "a matching identity means the delete is in flight"
    );
}

#[allow(dead_code)]
fn _unused(_: &FakeSecrets, _: &FakeTerminator, _: Ordering) {}

#[test]
fn a_rollback_through_a_symlinked_instance_directory_removes_nothing() {
    // A symlink named `<home>/instances/<name>` pointing outside the home is the
    // case lexical containment alone cannot catch: as a string the path is inside
    // the home. The real `rollback_create` must refuse it, so the sentinel outside
    // survives and the row is untouched.
    use std::os::unix::fs::symlink;

    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("symlinked", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();

    // The instance directory is replaced by a symlink to an outside directory
    // holding a sentinel that must not be deleted.
    let instance_dir = h.paths.instance_dir(&name);
    super::super::helpers::remove_tree(&instance_dir).expect("remove real dir");
    let outside = h.dir.path().join("outside-home");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("sentinel.txt"), b"keep me").expect("sentinel");
    symlink(&outside, &instance_dir).expect("symlink instance dir");

    // `safe_within` must reject a path through the symlink, which is what makes
    // the rollback refuse it.
    assert!(
        super::super::helpers::safe_within(&h.paths, &instance_dir).is_err(),
        "a symlinked instance directory must be rejected"
    );

    let removed = h.service.rollback_for_test(&id, &name);
    assert!(
        !removed,
        "the rollback must refuse a symlinked instance directory"
    );
    assert!(
        outside.join("sentinel.txt").exists(),
        "the sentinel outside the home must survive"
    );
    assert!(
        h.instance(&id).is_some(),
        "the row is left in place when cleanup refused"
    );
}

#[test]
fn dropping_a_clone_does_not_stop_the_service_but_the_last_handle_does() {
    // The supervisor task and the workers hold `Arc<Inner>`, so `strong_count`
    // cannot tell "last handle". A handle counter can: a clone dropped while the
    // original lives must leave the service fully working, and only the final
    // drop sets `stopping`. This is the defect found in review round 2, gated.
    let h = Harness::new(EngineKey::Postgresql);
    let clone = h.service.clone();
    assert!(!h.is_stopping(), "a fresh service is not stopping");

    // Dropping the clone while the harness's own handle lives must not stop it.
    drop(clone);
    assert!(
        !h.is_stopping(),
        "dropping a clone must not tell the service to stop"
    );
    assert!(
        h.supervisor_alive(),
        "the supervisor keeps running while a handle is alive"
    );
    // It still works after the clone is gone.
    let port = test_port();
    let spec = h.spec("after-clone", EngineKey::Postgresql, port);
    h.create(spec);

    // The last drop must set `stopping`. `inner` keeps the state readable after
    // the harness (and its runtime) is gone, so this observes the real flag the
    // supervisor and the workers check.
    let inner = Arc::clone(h.service.inner());
    assert!(!inner.is_stopping(), "still running before the last drop");
    drop(h);
    assert!(
        inner.is_stopping(),
        "the last handle tells the service to stop"
    );
}

#[test]
fn the_supervisor_skips_a_running_row_while_an_operation_holds_it() {
    // A `Running` row is briefly lock-held by the start operation between
    // persisting `Running` and dropping its `InstanceLock`, so the `is_busy`
    // guard is not dead code. With the probe gated, the supervisor would park in
    // `validate` the moment it looked at the row; while the lock is held it must
    // not reach `validate`, and once the lock is released it must.
    //
    // The gate is reset between the two phases and any parked call is released
    // before asserting, so a failure is a clean panic rather than a hang.
    //
    // Mutation check (R1b): remove the `if inner.ops.is_busy(..) { continue; }`
    // guard in `supervisor.rs::tick` and the first phase sees `reached == true`,
    // failing with
    //   assertion failed: the supervisor must not act on a row an operation holds
    let procs = Arc::new(FakeProcs::new());
    let gated = Arc::new(GatedProbe::new(Arc::clone(&procs)));
    let h = super::harness::with_gated_probe(
        EngineKey::Postgresql,
        Arc::clone(&procs),
        Arc::clone(&gated),
        Duration::from_millis(15),
    );
    let port = test_port();
    let spec = h.spec("busy-row", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();

    let ops = h.service.start_instance(&id).expect("start");
    h.run(ops);
    h.wait_for_status(&id, InstanceStatus::Running);

    // Phase 1: hold the instance lock and arm the gate. The supervisor must not
    // validate this `Running` row while the operation owns it.
    let lock = h.lock_instance(&id);
    gated.reset();
    gated.arm();
    let reached_while_busy = gated.wait_reached(Duration::from_millis(200));
    // Whatever happened, release the gate so no parked task outlives this test.
    gated.release_with(ProcessState::Match);
    drop(lock);

    assert!(
        !reached_while_busy,
        "the supervisor must not act on a row an operation holds"
    );

    // Phase 2: the lock is gone, so the supervisor must now reach `validate`.
    gated.reset();
    gated.arm();
    let reached_when_free = gated.wait_reached(Duration::from_secs(5));
    gated.release_with(ProcessState::Match);
    assert!(
        reached_when_free,
        "the supervisor must validate the row once the operation released it"
    );
}
