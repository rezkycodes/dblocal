//! Recovery clears the in-flight marker of a crashed delete, and only then.
//!
//! The marker now lives beside the instance directories (see
//! `delete::marker_path`). A delete removes its own marker on completion, but a
//! delete that crashed leaves it behind; reconciliation is the only code that
//! observes such a row afterwards, so it is also the only place that can remove
//! the stale file. Both recovery branches in `reconcile::finish_delete` clear it
//! (the abort in branch (a) and the removal in branch (c)), and neither may clear
//! a marker that still names a live delete.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::Arc;

use crate::model::{EngineKey, InstanceId, InstanceStatus};

use super::harness::{Harness, test_port};

/// Waits until a restarted service has finished its startup reconciliation.
///
/// The row changing is not enough: `finish_delete` clears the marker *after* the
/// store write, exactly like it emits afterwards, so observing the row first
/// would race the cleanup this module tests. `on_reconcile_done` fires after
/// every decision, so the assertions run after the last write.
fn restarted_after_reconciliation(
    paths: &crate::paths::DblPaths,
    procs: Arc<super::harness::FakeProcs>,
) -> Harness {
    let done = super::super::reconcile::on_reconcile_done(paths.root());
    let restarted = super::harness::restart_paused(paths, procs);
    restarted.service.start_background();
    done.recv_timeout(std::time::Duration::from_secs(30))
        .expect("reconciliation must finish");
    restarted
}

/// A marker naming a process that is gone, so `delete_in_flight` reads it as a
/// crash rather than a live delete.
fn write_stale_marker(h: &Harness, id: &InstanceId) {
    let marker = super::super::delete::marker_path(&h.paths, id);
    std::fs::write(
        &marker,
        "pid=999999\nstarted=1.0\nexe=/usr/bin/gone\nnonce=stale\n",
    )
    .expect("stale marker");
}

/// Branch (a): a delete crashed with its data still present. The safe answer is
/// to abort back to `Stopped`, and the crashed delete's marker must not linger.
#[test]
fn an_aborted_interrupted_delete_removes_its_marker() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("aborted-marker", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();

    // Data present is what makes reconciliation abort instead of purge.
    std::fs::write(h.paths.data_dir(&name).join("keep.txt"), b"precious").expect("data");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");
    write_stale_marker(&h, &id);
    let marker = super::super::delete::marker_path(&h.paths, &id);
    assert!(marker.exists(), "the fixture must start with a marker");

    let restarted = restarted_after_reconciliation(&h.paths, Arc::clone(&h.procs));

    let row = restarted
        .instance(&id)
        .expect("an aborted delete keeps the row");
    assert_eq!(
        row.status(),
        InstanceStatus::Stopped,
        "a crashed delete with its data present aborts to Stopped"
    );
    assert!(
        !marker.exists(),
        "reconciliation must remove the marker of a delete it has declared over: {}",
        marker.display()
    );
}

/// Branch (c): a delete crashed after its data was gone, so reconciliation
/// removes the row. The marker must go with it.
#[test]
fn a_finished_interrupted_delete_removes_its_marker() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("finished-marker", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();

    // Data gone and no archive recorded: the purge finished, only the row and the
    // marker are left.
    std::fs::remove_dir_all(h.paths.data_dir(&name)).expect("purge data");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");
    write_stale_marker(&h, &id);
    let marker = super::super::delete::marker_path(&h.paths, &id);
    assert!(marker.exists(), "the fixture must start with a marker");

    let restarted = restarted_after_reconciliation(&h.paths, Arc::clone(&h.procs));

    assert!(
        restarted.instance(&id).is_none(),
        "a crashed delete whose data is gone removes the row"
    );
    assert!(
        !marker.exists(),
        "reconciliation must remove the marker of the delete it just finished: {}",
        marker.display()
    );
}

/// The negative: a marker that names a live owner belongs to a real delete that
/// is still running, so reconciliation must leave both the row and the marker
/// alone.
#[test]
fn a_live_delete_marker_survives_reconciliation() {
    let h = Harness::new(EngineKey::Postgresql);
    let port = test_port();
    let spec = h.spec("live-marker", EngineKey::Postgresql, port);
    h.create(spec);
    let instance = h.instances().into_iter().next().expect("instance");
    let id = instance.id().clone();
    let name = instance.name().clone();

    std::fs::write(h.paths.data_dir(&name).join("keep.txt"), b"x").expect("data");
    h.store()
        .transition_status(&id, InstanceStatus::Stopped, InstanceStatus::Deleting)
        .expect("deleting");

    // A live process that is not ours owns the delete, exactly like the other
    // service sharing the home, so the marker records its full identity.
    let owner = h.procs.spawn("/some/other/dblocal");
    let started = owner
        .started_at()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}.0", d.as_secs()))
        .unwrap_or_else(|_| "0.0".to_owned());
    let marker = super::super::delete::marker_path(&h.paths, &id);
    std::fs::write(
        &marker,
        format!(
            "pid={}\nstarted={started}\nexe={}\nnonce=live\n",
            owner.pid(),
            owner.exe().display()
        ),
    )
    .expect("marker");

    let restarted = restarted_after_reconciliation(&h.paths, Arc::clone(&h.procs));

    let row = restarted
        .instance(&id)
        .expect("a live delete keeps its row");
    assert_eq!(
        row.status(),
        InstanceStatus::Deleting,
        "reconciliation must leave a delete that is still running alone"
    );
    assert!(
        marker.exists(),
        "reconciliation must never remove a live delete's marker: {}",
        marker.display()
    );
}
